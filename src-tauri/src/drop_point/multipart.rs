use mailparse::{DispositionType, parse_mail};

const ENVELOPE_PART: &str = "envelope";
const PAYLOAD_PART: &str = "payload";
const MULTIPART_MIXED: &str = "multipart/mixed";
const JSON_CONTENT_TYPE: &str = "application/json";
const OCTET_CONTENT_TYPE: &str = "application/octet-stream";
const MAX_ENVELOPE_BYTES: usize = 1 << 20;

#[derive(Debug, thiserror::Error)]
pub enum MultipartError {
    #[error("pickup response Content-Type must be multipart/mixed")]
    NotMultipartMixed,
    #[error("pickup response MIME body is invalid: {0}")]
    Parse(#[from] mailparse::MailParseError),
    #[error("pickup response contains unexpected part {0}")]
    UnexpectedPart(String),
    #[error("pickup response contains duplicate part {0}")]
    DuplicatePart(&'static str),
    #[error("pickup response is missing part {0}")]
    MissingPart(&'static str),
    #[error("pickup response part {part} must use Content-Type {expected}")]
    WrongPartContentType {
        part: &'static str,
        expected: &'static str,
    },
    #[error("pickup response envelope part is too large")]
    EnvelopeTooLarge,
}

pub fn parse_pickup_multipart(
    content_type: &str,
    body: &[u8],
) -> Result<(Vec<u8>, Vec<u8>), MultipartError> {
    let synthetic_message = synthetic_message(content_type, body);
    let parsed = parse_mail(&synthetic_message)?;
    if !parsed.ctype.mimetype.eq_ignore_ascii_case(MULTIPART_MIXED) {
        return Err(MultipartError::NotMultipartMixed);
    }

    let mut envelope = None;
    let mut payload = None;

    for part in &parsed.subparts {
        let disposition = part.get_content_disposition();
        if disposition.disposition != DispositionType::Attachment
            && disposition.disposition != DispositionType::FormData
        {
            return Err(MultipartError::UnexpectedPart(
                disposition
                    .params
                    .get("name")
                    .cloned()
                    .unwrap_or_else(|| "<unnamed>".to_string()),
            ));
        }
        let name = disposition
            .params
            .get("name")
            .ok_or_else(|| MultipartError::UnexpectedPart("<unnamed>".to_string()))?;
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
                let data = part.get_body_raw()?;
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
                payload = Some(part.get_body_raw()?);
            }
            other => return Err(MultipartError::UnexpectedPart(other.to_string())),
        }
    }

    Ok((
        envelope.ok_or(MultipartError::MissingPart(ENVELOPE_PART))?,
        payload.ok_or(MultipartError::MissingPart(PAYLOAD_PART))?,
    ))
}

fn synthetic_message(content_type: &str, body: &[u8]) -> Vec<u8> {
    let mut message = Vec::with_capacity(content_type.len() + body.len() + 40);
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

    #[test]
    fn parses_drop_point_pickup_shape() {
        let content_type = "multipart/mixed; boundary=test-boundary";
        let body = concat!(
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

        let (envelope, payload) = parse_pickup_multipart(content_type, body.as_bytes()).unwrap();
        assert_eq!(envelope, b"{}");
        assert_eq!(payload, b"abc");
    }
}
