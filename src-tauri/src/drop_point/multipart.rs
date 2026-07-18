use mailparse::{DispositionType, parse_content_type, parse_mail};

const ENVELOPE_PART: &str = "envelope";
const PAYLOAD_PART: &str = "payload";
const MULTIPART_MIXED: &str = "multipart/mixed";
const JSON_CONTENT_TYPE: &str = "application/json";
const OCTET_CONTENT_TYPE: &str = "application/octet-stream";
const MAX_ENVELOPE_BYTES: usize = 1 << 20;
const MAX_CONTENT_TYPE_BYTES: usize = 1024;
const MAX_BOUNDARY_BYTES: usize = 70;

#[derive(Debug, thiserror::Error)]
pub enum MultipartError {
    #[error("pickup response Content-Type is invalid")]
    InvalidContentType,
    #[error("pickup response Content-Type must be multipart/mixed")]
    NotMultipartMixed,
    #[error("pickup response multipart boundary is invalid")]
    InvalidBoundary,
    #[error("pickup response is missing its final multipart boundary")]
    Truncated,
    #[error("pickup response MIME body is invalid")]
    Parse,
    #[error("pickup response contains an unexpected part")]
    UnexpectedPart,
    #[error("pickup response part headers are ambiguous or incomplete")]
    InvalidPartHeaders,
    #[error("pickup response contains duplicate part {0}")]
    DuplicatePart(&'static str),
    #[error("pickup response is missing part {0}")]
    MissingPart(&'static str),
    #[error("pickup response part {part} must use Content-Type {expected}")]
    WrongPartContentType {
        part: &'static str,
        expected: &'static str,
    },
    #[error("pickup response parts must not use Content-Transfer-Encoding")]
    TransferEncoding,
    #[error("pickup response envelope part exceeds 1 MiB")]
    EnvelopeTooLarge,
    #[error("pickup response encrypted payload exceeds the persisted drop-point limit")]
    PayloadTooLarge,
}

#[expect(
    clippy::too_many_lines,
    reason = "the strict multipart shape is validated in one linear pass"
)]
pub fn parse_pickup_multipart(
    content_type: &str,
    body: &[u8],
    max_payload_bytes: u64,
) -> Result<(Vec<u8>, Vec<u8>), MultipartError> {
    if content_type.is_empty()
        || content_type.len() > MAX_CONTENT_TYPE_BYTES
        || content_type.bytes().any(|byte| byte.is_ascii_control())
    {
        return Err(MultipartError::InvalidContentType);
    }

    let parsed_content_type = parse_content_type(content_type);
    if parsed_content_type.mimetype != MULTIPART_MIXED {
        return Err(MultipartError::NotMultipartMixed);
    }
    if parameter_count(content_type, "boundary") != 1 {
        return Err(MultipartError::InvalidBoundary);
    }
    let boundary = parsed_content_type
        .params
        .get("boundary")
        .filter(|boundary| valid_boundary(boundary))
        .ok_or(MultipartError::InvalidBoundary)?;
    validate_complete_body(body, boundary)?;

    let synthetic_message = synthetic_message(content_type, body);
    let parsed = parse_mail(&synthetic_message).map_err(|_| MultipartError::Parse)?;
    if !parsed.ctype.mimetype.eq_ignore_ascii_case(MULTIPART_MIXED) {
        return Err(MultipartError::NotMultipartMixed);
    }

    let mut envelope = None;
    let mut payload = None;

    for part in &parsed.subparts {
        let disposition_headers = part
            .headers
            .iter()
            .filter(|header| {
                header
                    .get_key_ref()
                    .eq_ignore_ascii_case("Content-Disposition")
            })
            .collect::<Vec<_>>();
        let content_type_count = part
            .headers
            .iter()
            .filter(|header| header.get_key_ref().eq_ignore_ascii_case("Content-Type"))
            .count();
        if disposition_headers.len() != 1
            || content_type_count != 1
            || parameter_count(&disposition_headers[0].get_value(), "name") != 1
        {
            return Err(MultipartError::InvalidPartHeaders);
        }
        if part.headers.iter().any(|header| {
            header
                .get_key_ref()
                .eq_ignore_ascii_case("Content-Transfer-Encoding")
        }) {
            return Err(MultipartError::TransferEncoding);
        }
        let disposition = part.get_content_disposition();
        if disposition.disposition != DispositionType::Attachment
            && disposition.disposition != DispositionType::FormData
        {
            return Err(MultipartError::UnexpectedPart);
        }
        let name = disposition
            .params
            .get("name")
            .ok_or(MultipartError::UnexpectedPart)?;
        match name.as_str() {
            ENVELOPE_PART => {
                if envelope.is_some() {
                    return Err(MultipartError::DuplicatePart(ENVELOPE_PART));
                }
                if !part.ctype.mimetype.eq_ignore_ascii_case(JSON_CONTENT_TYPE) {
                    return Err(MultipartError::WrongPartContentType {
                        part: ENVELOPE_PART,
                        expected: JSON_CONTENT_TYPE,
                    });
                }
                let data = part.get_body_raw().map_err(|_| MultipartError::Parse)?;
                if data.len() > MAX_ENVELOPE_BYTES {
                    return Err(MultipartError::EnvelopeTooLarge);
                }
                envelope = Some(data);
            }
            PAYLOAD_PART => {
                if payload.is_some() {
                    return Err(MultipartError::DuplicatePart(PAYLOAD_PART));
                }
                if !part.ctype.mimetype.eq_ignore_ascii_case(OCTET_CONTENT_TYPE) {
                    return Err(MultipartError::WrongPartContentType {
                        part: PAYLOAD_PART,
                        expected: OCTET_CONTENT_TYPE,
                    });
                }
                let data = part.get_body_raw().map_err(|_| MultipartError::Parse)?;
                if u64::try_from(data.len()).map_or(true, |length| length > max_payload_bytes) {
                    return Err(MultipartError::PayloadTooLarge);
                }
                payload = Some(data);
            }
            _ => return Err(MultipartError::UnexpectedPart),
        }
    }

    Ok((
        envelope.ok_or(MultipartError::MissingPart(ENVELOPE_PART))?,
        payload.ok_or(MultipartError::MissingPart(PAYLOAD_PART))?,
    ))
}

fn parameter_count(header: &str, wanted_name: &str) -> usize {
    let mut quoted = false;
    let mut escaped = false;
    let mut segment_start = 0;
    let mut count = 0;
    for (index, character) in header.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match character {
            '\\' if quoted => escaped = true,
            '"' => quoted = !quoted,
            ';' if !quoted => {
                count += usize::from(
                    parameter_name(&header[segment_start..index])
                        .is_some_and(|name| name.eq_ignore_ascii_case(wanted_name)),
                );
                segment_start = index + 1;
            }
            _ => {}
        }
    }
    count
        + usize::from(
            parameter_name(&header[segment_start..])
                .is_some_and(|name| name.eq_ignore_ascii_case(wanted_name)),
        )
}

fn parameter_name(segment: &str) -> Option<&str> {
    segment.trim().split_once('=').map(|(name, _)| name.trim())
}

fn valid_boundary(boundary: &str) -> bool {
    let bytes = boundary.as_bytes();
    !bytes.is_empty()
        && bytes.len() <= MAX_BOUNDARY_BYTES
        && !bytes.ends_with(b" ")
        && bytes.iter().all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'\''
                        | b'('
                        | b')'
                        | b'+'
                        | b'_'
                        | b','
                        | b'-'
                        | b'.'
                        | b'/'
                        | b':'
                        | b'='
                        | b'?'
                        | b' '
                )
        })
}

fn validate_complete_body(body: &[u8], boundary: &str) -> Result<(), MultipartError> {
    let opening = [b"--".as_slice(), boundary.as_bytes(), b"\r\n"].concat();
    if !body.starts_with(&opening) {
        return Err(MultipartError::Truncated);
    }

    let closing = [b"--".as_slice(), boundary.as_bytes(), b"--"].concat();
    let without_optional_crlf = body.strip_suffix(b"\r\n").unwrap_or(body);
    if !without_optional_crlf.ends_with(&closing) {
        return Err(MultipartError::Truncated);
    }
    Ok(())
}

fn synthetic_message(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(
        content_type
            .len()
            .saturating_add(body.len())
            .saturating_add(40),
    );
    message.extend_from_slice(b"Content-Type: ");
    message.extend_from_slice(content_type.as_bytes());
    message.extend_from_slice(b"\r\nMIME-Version: 1.0\r\n\r\n");
    message.extend_from_slice(body);
    message
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    const CONTENT_TYPE: &str = "multipart/mixed; boundary=test-boundary";
    const BODY: &str = concat!(
        "--test-boundary\r\n",
        "Content-Disposition: attachment; name=\"envelope\"\r\n",
        "Content-Type: application/json\r\n\r\n",
        "{}\r\n",
        "--test-boundary\r\n",
        "Content-Disposition: attachment; name=\"payload\"\r\n",
        "Content-Type: application/octet-stream\r\n\r\n",
        "abc\r\n",
        "--test-boundary--\r\n"
    );

    #[test]
    fn parses_drop_point_pickup_shape() {
        let (envelope, payload) = parse_pickup_multipart(CONTENT_TYPE, BODY.as_bytes(), 3).unwrap();
        assert_eq!(envelope, b"{}");
        assert_eq!(payload, b"abc");
    }

    #[test]
    fn rejects_truncated_closing_boundary() {
        let truncated = BODY.trim_end_matches("--test-boundary--\r\n");
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, truncated.as_bytes(), 3),
            Err(MultipartError::Truncated)
        ));
    }

    #[test]
    fn rejects_duplicate_and_additional_parts() {
        let duplicate = BODY.replace(
            "--test-boundary--\r\n",
            concat!(
                "--test-boundary\r\n",
                "Content-Disposition: attachment; name=\"payload\"\r\n",
                "Content-Type: application/octet-stream\r\n\r\n",
                "x\r\n",
                "--test-boundary--\r\n"
            ),
        );
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, duplicate.as_bytes(), 3),
            Err(MultipartError::DuplicatePart(PAYLOAD_PART))
        ));

        let additional = BODY.replace("name=\"payload\"", "name=\"other\"");
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, additional.as_bytes(), 3),
            Err(MultipartError::UnexpectedPart)
        ));
    }

    #[test]
    fn rejects_duplicate_or_missing_required_part_headers() {
        let duplicate_name =
            BODY.replace("name=\"envelope\"", "name=\"envelope\"; name=\"payload\"");
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, duplicate_name.as_bytes(), 3),
            Err(MultipartError::InvalidPartHeaders)
        ));

        let duplicate_content_type = BODY.replacen(
            "Content-Type: application/json\r\n",
            "Content-Type: application/json\r\nContent-Type: application/json\r\n",
            1,
        );
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, duplicate_content_type.as_bytes(), 3),
            Err(MultipartError::InvalidPartHeaders)
        ));
    }

    #[test]
    fn enforces_persisted_payload_bound() {
        assert!(matches!(
            parse_pickup_multipart(CONTENT_TYPE, BODY.as_bytes(), 2),
            Err(MultipartError::PayloadTooLarge)
        ));
    }

    #[test]
    fn rejects_content_type_header_injection_and_missing_boundary() {
        assert!(matches!(
            parse_pickup_multipart("multipart/mixed\r\nX-Evil: yes", BODY.as_bytes(), 3),
            Err(MultipartError::InvalidContentType)
        ));
        assert!(matches!(
            parse_pickup_multipart("multipart/mixed", BODY.as_bytes(), 3),
            Err(MultipartError::InvalidBoundary)
        ));
        assert!(matches!(
            parse_pickup_multipart(
                "multipart/mixed; boundary=test-boundary; boundary=other",
                BODY.as_bytes(),
                3
            ),
            Err(MultipartError::InvalidBoundary)
        ));
    }
}
