use std::collections::HashSet;

use chrono::DateTime;
use serde::Deserialize;
use unicode_general_category::{GeneralCategory, get_general_category};
use unicode_normalization::{UnicodeNormalization, is_nfc};
use zeroize::Zeroize;

const PROTOCOL_VERSION: u32 = 2;
const DEFAULT_MIME_TYPE: &str = "application/octet-stream";
const MAX_MANIFEST_FILES: usize = 1000;
const MAX_FILENAME_UTF8_BYTES: usize = 240;
const MAX_MIME_TYPE_UTF8_BYTES: usize = 255;
const RESERVED_RECEIPT_NAME: &str = ".droppoint-receipt.json";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredFile {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
}

impl Drop for RecoveredFile {
    fn drop(&mut self) {
        self.filename.zeroize();
        self.content_type.zeroize();
        self.data.zeroize();
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    protocol_version: u32,
    files: Vec<ManifestFile>,
    created_at: String,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct ManifestFile {
    name: String,
    #[serde(rename = "type")]
    mime_type: String,
    size: u64,
}

impl Drop for ManifestFile {
    fn drop(&mut self) {
        self.name.zeroize();
        self.mime_type.zeroize();
    }
}

impl Drop for Manifest {
    fn drop(&mut self) {
        self.created_at.zeroize();
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest JSON is invalid")]
    Json,
    #[error("manifest protocol_version = {actual}, want {expected}")]
    UnsupportedProtocol { actual: u32, expected: u32 },
    #[error("manifest created_at is invalid: {0}")]
    InvalidCreatedAt(String),
    #[error("manifest must contain between 1 and {MAX_MANIFEST_FILES} files")]
    InvalidFileCount,
    #[error("manifest filename at index {index} is invalid: {reason}")]
    InvalidFilename { index: usize, reason: String },
    #[error("manifest contains colliding filenames at index {index}")]
    FilenameCollision { index: usize },
    #[error("manifest file {index} MIME type is invalid: {reason}")]
    InvalidMimeType { index: usize, reason: String },
    #[error("manifest file {index} size cannot be represented by this platform")]
    SizeNotRepresentable { index: usize },
    #[error("manifest file {index} size exceeds the remaining authenticated payload")]
    SizeExceedsPayload { index: usize },
    #[error("manifest sizes leave {remaining} unclaimed authenticated payload bytes")]
    UnclaimedPayload { remaining: usize },
    #[error("manifest payload bounds changed while splitting file {index}")]
    InvalidSliceBounds { index: usize },
}

pub fn split_payload(
    manifest_json: &[u8],
    payload_plaintext: &[u8],
) -> Result<Vec<RecoveredFile>, ManifestError> {
    let manifest: Manifest =
        serde_json::from_slice(manifest_json).map_err(|_| ManifestError::Json)?;
    validate_manifest_header(&manifest)?;

    let mut remaining = payload_plaintext.len();
    let mut comparison_keys = HashSet::with_capacity(manifest.files.len());
    let entries = manifest
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| {
            validate_filename(&file.name)
                .map_err(|reason| ManifestError::InvalidFilename { index, reason })?;
            if !comparison_keys.insert(filename_collision_key(&file.name)) {
                return Err(ManifestError::FilenameCollision { index });
            }
            let content_type = sanitize_mime_type(&file.mime_type)
                .map_err(|reason| ManifestError::InvalidMimeType { index, reason })?;
            let size = usize::try_from(file.size)
                .map_err(|_| ManifestError::SizeNotRepresentable { index })?;
            if size > remaining {
                return Err(ManifestError::SizeExceedsPayload { index });
            }
            remaining -= size;
            Ok((file.name.clone(), content_type, size))
        })
        .collect::<Result<Vec<_>, ManifestError>>()?;

    if remaining != 0 {
        return Err(ManifestError::UnclaimedPayload { remaining });
    }

    entries
        .into_iter()
        .enumerate()
        .try_fold(
            (Vec::with_capacity(manifest.files.len()), 0usize),
            |(mut files, offset), (index, (filename, content_type, size))| {
                let end = offset
                    .checked_add(size)
                    .ok_or(ManifestError::InvalidSliceBounds { index })?;
                let data = payload_plaintext
                    .get(offset..end)
                    .ok_or(ManifestError::InvalidSliceBounds { index })?
                    .to_vec();
                files.push(RecoveredFile {
                    filename,
                    content_type,
                    data,
                });
                Ok((files, end))
            },
        )
        .map(|(files, _)| files)
}

fn validate_manifest_header(manifest: &Manifest) -> Result<(), ManifestError> {
    if manifest.protocol_version != PROTOCOL_VERSION {
        return Err(ManifestError::UnsupportedProtocol {
            actual: manifest.protocol_version,
            expected: PROTOCOL_VERSION,
        });
    }
    if !(1..=MAX_MANIFEST_FILES).contains(&manifest.files.len()) {
        return Err(ManifestError::InvalidFileCount);
    }
    validate_rfc3339_timestamp(&manifest.created_at).map_err(ManifestError::InvalidCreatedAt)
}

pub fn validate_rfc3339_timestamp(value: &str) -> Result<(), String> {
    let bytes = value.as_bytes();
    let fixed_digits = [0, 1, 2, 3, 5, 6, 8, 9, 11, 12, 14, 15, 17, 18];
    if bytes.len() < 20
        || fixed_digits
            .into_iter()
            .any(|index| bytes.get(index).is_none_or(|byte| !byte.is_ascii_digit()))
        || bytes.get(4) != Some(&b'-')
        || bytes.get(7) != Some(&b'-')
        || bytes.get(10) != Some(&b'T')
        || bytes.get(13) != Some(&b':')
        || bytes.get(16) != Some(&b':')
    {
        return Err("timestamp does not have canonical RFC3339 syntax".to_string());
    }

    let mut zone_start = 19;
    if bytes.get(zone_start) == Some(&b'.') {
        zone_start += 1;
        let fraction_start = zone_start;
        while bytes.get(zone_start).is_some_and(u8::is_ascii_digit)
            && zone_start - fraction_start < 9
        {
            zone_start += 1;
        }
        if zone_start == fraction_start || bytes.get(zone_start).is_some_and(u8::is_ascii_digit) {
            return Err("timestamp has an invalid fractional second".to_string());
        }
    }

    let zone = bytes.get(zone_start..).unwrap_or_default();
    let valid_zone = zone == b"Z"
        || (zone.len() == 6
            && matches!(zone.first(), Some(b'+' | b'-'))
            && zone.get(1).is_some_and(u8::is_ascii_digit)
            && zone.get(2).is_some_and(u8::is_ascii_digit)
            && zone.get(3) == Some(&b':')
            && zone.get(4).is_some_and(u8::is_ascii_digit)
            && zone.get(5).is_some_and(u8::is_ascii_digit));
    if !valid_zone {
        return Err("timestamp must include an RFC3339 timezone".to_string());
    }

    DateTime::parse_from_rfc3339(value)
        .map(|_| ())
        .map_err(|_| "timestamp contains an invalid date or time".to_string())
}

pub fn validate_filename(name: &str) -> Result<(), String> {
    let encoded_len = name.len();
    if !(1..=MAX_FILENAME_UTF8_BYTES).contains(&encoded_len) {
        return Err(format!(
            "filename must contain between 1 and {MAX_FILENAME_UTF8_BYTES} UTF-8 bytes"
        ));
    }
    if !is_nfc(name) {
        return Err("filename must use Unicode NFC".to_string());
    }
    if name.chars().any(is_forbidden_filename_character) {
        return Err("filename contains a forbidden character".to_string());
    }
    if name.ends_with([' ', '.']) {
        return Err("filename must not end in an ASCII space or dot".to_string());
    }
    if matches!(name, "." | "..") || name.chars().all(char::is_whitespace) {
        return Err("filename is blank or reserved".to_string());
    }
    if name.eq_ignore_ascii_case(RESERVED_RECEIPT_NAME) {
        return Err("filename is reserved for the installation receipt".to_string());
    }
    if is_windows_reserved_name(name) {
        return Err("filename has a Windows-reserved stem".to_string());
    }
    Ok(())
}

fn is_forbidden_filename_character(character: char) -> bool {
    matches!(
        character,
        '/' | '\\' | '\0' | '<' | '>' | ':' | '"' | '|' | '?' | '*'
    ) || matches!(
        get_general_category(character),
        GeneralCategory::Control | GeneralCategory::Format
    )
}

fn is_windows_reserved_name(filename: &str) -> bool {
    let stem = filename
        .split_once('.')
        .map_or(filename, |(stem, _)| stem)
        .trim_end_matches(' ')
        .to_uppercase();
    matches!(
        stem.as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "COM¹"
            | "COM²"
            | "COM³"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
            | "LPT¹"
            | "LPT²"
            | "LPT³"
    )
}

pub fn filename_collision_key(filename: &str) -> String {
    filename.nfc().collect::<String>().to_lowercase()
}

pub fn sanitize_mime_type(value: &str) -> Result<String, String> {
    if value.is_empty() {
        return Ok(DEFAULT_MIME_TYPE.to_string());
    }
    if value.len() > MAX_MIME_TYPE_UTF8_BYTES {
        return Err(format!(
            "MIME type exceeds {MAX_MIME_TYPE_UTF8_BYTES} UTF-8 bytes"
        ));
    }

    let lowered = value.to_ascii_lowercase();
    let Some((top, subtype)) = lowered.split_once('/') else {
        return Err("MIME type must contain exactly one slash".to_string());
    };
    if subtype.contains('/') || !is_mime_token(top) || !is_mime_token(subtype) {
        return Err("MIME type contains invalid token characters".to_string());
    }
    Ok(lowered)
}

fn is_mime_token(value: &str) -> bool {
    let mut bytes = value.bytes();
    matches!(bytes.next(), Some(first) if first.is_ascii_alphanumeric())
        && bytes.all(|byte| {
            byte.is_ascii_alphanumeric()
                || matches!(
                    byte,
                    b'!' | b'#' | b'$' | b'&' | b'^' | b'_' | b'.' | b'+' | b'-'
                )
        })
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use proptest::prelude::*;
    use serde::Deserialize;

    use super::*;

    #[derive(Deserialize)]
    struct FilenameFixture {
        max_filename_utf8_bytes: usize,
        max_manifest_files: usize,
        canonicalization_bundles: Vec<CanonicalizationBundle>,
        validation: Vec<FilenameValidation>,
        collisions: Vec<FilenameCollision>,
    }

    #[derive(Deserialize)]
    struct CanonicalizationBundle {
        output: Vec<String>,
    }

    #[derive(Deserialize)]
    struct FilenameValidation {
        name: String,
        valid: bool,
    }

    #[derive(Deserialize)]
    struct FilenameCollision {
        names: Vec<String>,
        collides: bool,
    }

    #[derive(Deserialize)]
    struct ParsingFixture {
        timestamps: Vec<TimestampFixture>,
        mime_types: Vec<MimeFixture>,
    }

    #[derive(Deserialize)]
    struct TimestampFixture {
        value: String,
        valid: bool,
    }

    #[derive(Deserialize)]
    struct MimeFixture {
        value: String,
        valid: bool,
        canonical: Option<String>,
    }

    #[test]
    fn accepts_every_normative_sender_canonicalization_output() {
        let fixture: FilenameFixture = serde_json::from_str(include_str!(
            "../../testdata/drop_point/filename-policy.json"
        ))
        .unwrap();
        assert_eq!(fixture.max_filename_utf8_bytes, MAX_FILENAME_UTF8_BYTES);
        assert_eq!(fixture.max_manifest_files, MAX_MANIFEST_FILES);
        for bundle in fixture.canonicalization_bundles {
            let keys = bundle
                .output
                .iter()
                .map(|name| {
                    validate_filename(name).unwrap();
                    filename_collision_key(name)
                })
                .collect::<HashSet<_>>();
            assert_eq!(keys.len(), bundle.output.len());
        }
    }

    #[test]
    fn matches_normative_filename_validation_fixture() {
        let fixture: FilenameFixture = serde_json::from_str(include_str!(
            "../../testdata/drop_point/filename-policy.json"
        ))
        .unwrap();
        for case in fixture.validation {
            assert_eq!(
                validate_filename(&case.name).is_ok(),
                case.valid,
                "unexpected result for {:?}",
                case.name
            );
        }
    }

    #[test]
    fn matches_normative_filename_collision_fixture() {
        let fixture: FilenameFixture = serde_json::from_str(include_str!(
            "../../testdata/drop_point/filename-policy.json"
        ))
        .unwrap();
        for case in fixture.collisions {
            let keys = case
                .names
                .iter()
                .map(|name| filename_collision_key(name))
                .collect::<HashSet<_>>();
            assert_eq!(
                keys.len() != case.names.len(),
                case.collides,
                "unexpected collision result for {:?}",
                case.names
            );
        }
    }

    #[test]
    fn matches_normative_timestamp_and_mime_fixtures() {
        let fixture: ParsingFixture = serde_json::from_str(include_str!(
            "../../testdata/drop_point/protocol-parsing-policy.json"
        ))
        .unwrap();
        for case in fixture.timestamps {
            assert_eq!(
                validate_rfc3339_timestamp(&case.value).is_ok(),
                case.valid,
                "unexpected timestamp result for {:?}",
                case.value
            );
        }
        for case in fixture.mime_types {
            let actual = sanitize_mime_type(&case.value);
            assert_eq!(
                actual.is_ok(),
                case.valid,
                "unexpected MIME result for {:?}",
                case.value
            );
            if let Some(canonical) = case.canonical {
                assert_eq!(actual.unwrap(), canonical);
            }
        }
    }

    #[test]
    fn rejects_noncanonical_rfc3339_spellings() {
        for invalid in [
            "2026-06-30 12:00:00Z",
            "2026-06-30t12:00:00Z",
            "2026-06-30T12:00:00z",
            "2026-06-30T12:00:00.1234567890Z",
            "2026-06-30T12:00:00+0900",
        ] {
            assert!(
                validate_rfc3339_timestamp(invalid).is_err(),
                "accepted {invalid}"
            );
        }
    }

    #[test]
    fn rejects_noncanonical_and_colliding_filenames_instead_of_rewriting() {
        let traversal = br#"{"protocol_version":2,"files":[{"name":"../a.txt","type":"text/plain","size":0}],"created_at":"2026-06-30T12:00:00Z"}"#;
        assert!(matches!(
            split_payload(traversal, b""),
            Err(ManifestError::InvalidFilename { .. })
        ));

        let collision = br#"{"protocol_version":2,"files":[{"name":"Report.txt","type":"text/plain","size":1},{"name":"report.TXT","type":"text/plain","size":1}],"created_at":"2026-06-30T12:00:00Z"}"#;
        assert!(matches!(
            split_payload(collision, b"ab"),
            Err(ManifestError::FilenameCollision { index: 1 })
        ));
    }

    #[test]
    fn rejects_duplicate_unknown_and_wrongly_typed_json_fields() {
        let cases: &[&[u8]] = &[
            br#"{"protocol_version":2,"protocol_version":2,"files":[],"created_at":"2026-06-30T12:00:00Z"}"#,
            br#"{"protocol_version":2,"files":[],"created_at":"2026-06-30T12:00:00Z","extra":1}"#,
            br#"{"protocol_version":true,"files":[],"created_at":"2026-06-30T12:00:00Z"}"#,
            br#"{"protocol_version":2,"files":[{"name":"a","type":"","size":true}],"created_at":"2026-06-30T12:00:00Z"}"#,
        ];
        for case in cases {
            assert!(split_payload(case, b"").is_err());
        }
    }

    #[test]
    fn enforces_the_manifest_file_count_bound() {
        let manifest = |count: usize| {
            serde_json::to_vec(&serde_json::json!({
                "protocol_version": 2,
                "files": (0..count)
                    .map(|index| serde_json::json!({
                        "name": format!("file-{index}"),
                        "type": "",
                        "size": 0,
                    }))
                    .collect::<Vec<_>>(),
                "created_at": "2026-06-30T12:00:00Z",
            }))
            .unwrap()
        };
        assert_eq!(
            split_payload(&manifest(MAX_MANIFEST_FILES), b"")
                .unwrap()
                .len(),
            MAX_MANIFEST_FILES
        );
        assert!(matches!(
            split_payload(&manifest(MAX_MANIFEST_FILES + 1), b""),
            Err(ManifestError::InvalidFileCount)
        ));
    }

    #[test]
    fn checks_each_size_against_remaining_payload() {
        let manifest = br#"{"protocol_version":2,"files":[{"name":"a","type":"","size":18446744073709551615}],"created_at":"2026-06-30T12:00:00Z"}"#;
        assert!(matches!(
            split_payload(manifest, b""),
            Err(ManifestError::SizeExceedsPayload { .. }
                | ManifestError::SizeNotRepresentable { .. })
        ));
    }

    proptest! {
        #[test]
        fn arbitrary_manifests_return_without_panicking(
            manifest in proptest::collection::vec(any::<u8>(), 0..2048),
            payload in proptest::collection::vec(any::<u8>(), 0..2048),
        ) {
            let _ = split_payload(&manifest, &payload);
        }
    }
}
