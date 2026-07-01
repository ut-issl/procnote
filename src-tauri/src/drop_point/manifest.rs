use std::collections::HashSet;

use chrono::DateTime;
use serde::Deserialize;

const PROTOCOL_VERSION: u32 = 2;
const DEFAULT_MIME_TYPE: &str = "application/octet-stream";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecoveredFile {
    pub filename: String,
    pub content_type: String,
    pub data: Vec<u8>,
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

#[derive(Debug, thiserror::Error)]
pub enum ManifestError {
    #[error("manifest JSON is invalid: {0}")]
    Json(#[from] serde_json::Error),
    #[error("manifest protocol_version = {actual}, want {expected}")]
    UnsupportedProtocol { actual: u32, expected: u32 },
    #[error("manifest created_at is invalid: {0}")]
    InvalidCreatedAt(String),
    #[error("manifest must contain at least one file")]
    EmptyFiles,
    #[error("manifest file {index} name is invalid: {reason}")]
    InvalidFilename { index: usize, reason: String },
    #[error("manifest file {index} MIME type is invalid: {reason}")]
    InvalidMimeType { index: usize, reason: String },
    #[error("manifest filename {filename} has too many duplicates")]
    FilenameSuffixOverflow { filename: String },
    #[error("manifest size sum overflows usize")]
    SizeOverflow,
    #[error("manifest size sum {expected} does not match payload length {actual}")]
    SizeMismatch { expected: usize, actual: usize },
}

pub fn split_payload(
    manifest_json: &[u8],
    payload_plaintext: &[u8],
) -> Result<Vec<RecoveredFile>, ManifestError> {
    let manifest: Manifest = serde_json::from_slice(manifest_json)?;
    validate_manifest_header(&manifest)?;

    let mut total = 0usize;
    let mut used_names = HashSet::new();
    let mut sanitized_entries = Vec::with_capacity(manifest.files.len());

    for (index, file) in manifest.files.iter().enumerate() {
        let sanitized_filename = sanitize_filename(&file.name)
            .map_err(|reason| ManifestError::InvalidFilename { index, reason })?;
        let filename = unique_filename(&sanitized_filename, &mut used_names)?;
        let content_type = sanitize_mime_type(&file.mime_type)
            .map_err(|reason| ManifestError::InvalidMimeType { index, reason })?;
        let size = usize::try_from(file.size).map_err(|_| ManifestError::SizeOverflow)?;
        total = total.checked_add(size).ok_or(ManifestError::SizeOverflow)?;
        sanitized_entries.push((filename, content_type, size));
    }

    if total != payload_plaintext.len() {
        return Err(ManifestError::SizeMismatch {
            expected: total,
            actual: payload_plaintext.len(),
        });
    }

    sanitized_entries
        .into_iter()
        .try_fold(
            (Vec::new(), 0usize),
            |(mut files, offset), (filename, content_type, size)| {
                let next = offset
                    .checked_add(size)
                    .ok_or(ManifestError::SizeOverflow)?;
                files.push(RecoveredFile {
                    filename,
                    content_type,
                    data: payload_plaintext[offset..next].to_vec(),
                });
                Ok((files, next))
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
    if manifest.files.is_empty() {
        return Err(ManifestError::EmptyFiles);
    }
    DateTime::parse_from_rfc3339(&manifest.created_at)
        .map(|_| ())
        .map_err(|e| ManifestError::InvalidCreatedAt(e.to_string()))
}

fn unique_filename(
    filename: &str,
    used_names: &mut HashSet<String>,
) -> Result<String, ManifestError> {
    if used_names.insert(fold_filename(filename)) {
        return Ok(filename.to_string());
    }

    let (stem, extension) = split_filename_extension(filename);
    std::iter::successors(Some(1usize), |suffix| suffix.checked_add(1))
        .find_map(|suffix| {
            let candidate = format!("{stem} ({suffix}){extension}");
            used_names
                .insert(fold_filename(&candidate))
                .then_some(candidate)
        })
        .ok_or_else(|| ManifestError::FilenameSuffixOverflow {
            filename: filename.to_string(),
        })
}

fn split_filename_extension(filename: &str) -> (&str, &str) {
    match filename.rsplit_once('.') {
        Some((stem, _extension)) if !stem.is_empty() => (stem, &filename[stem.len()..]),
        Some(_) | None => (filename, ""),
    }
}

fn fold_filename(filename: &str) -> String {
    filename.to_lowercase()
}

fn sanitize_filename(name: &str) -> Result<String, String> {
    if name.is_empty() {
        return Err("filename must not be empty".to_string());
    }
    if name.contains('/') || name.contains('\\') {
        return Err("filename must be a base name".to_string());
    }
    if name.chars().any(|ch| ch == '\0' || ch.is_control()) {
        return Err("filename contains control characters".to_string());
    }
    if name
        .chars()
        .any(|ch| matches!(ch, '<' | '>' | ':' | '"' | '|' | '?' | '*'))
    {
        return Err("filename contains platform-reserved characters".to_string());
    }
    let trimmed = name.trim();
    match trimmed {
        "" => Err("filename must not be blank".to_string()),
        "." | ".." => Err("filename is reserved".to_string()),
        _ if is_windows_reserved_name(trimmed) => Err("filename is platform-reserved".to_string()),
        _ => Ok(trimmed.to_string()),
    }
}

fn is_windows_reserved_name(name: &str) -> bool {
    let stem = name.rsplit_once('.').map_or(name, |(base, _)| base);
    matches!(
        stem.to_ascii_uppercase().as_str(),
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
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn sanitize_mime_type(value: &str) -> Result<String, String> {
    let trimmed = value.trim();
    if trimmed.is_empty() {
        return Ok(DEFAULT_MIME_TYPE.to_string());
    }
    if trimmed.chars().any(char::is_whitespace) || trimmed.chars().any(char::is_control) {
        return Err("MIME type contains whitespace or control characters".to_string());
    }
    let lowered = trimmed.to_ascii_lowercase();
    let Some((top, sub)) = lowered.split_once('/') else {
        return Err("MIME type must contain one slash".to_string());
    };
    if sub.contains('/') {
        return Err("MIME type must contain one slash".to_string());
    }
    if !is_mime_token(top) || !is_mime_token(sub) {
        return Err("MIME type contains invalid token characters".to_string());
    }
    Ok(lowered)
}

fn is_mime_token(value: &str) -> bool {
    let mut chars = value.chars();
    match chars.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => return false,
    }
    chars.all(|ch| {
        ch.is_ascii_alphanumeric()
            || matches!(ch, '!' | '#' | '$' | '&' | '^' | '_' | '.' | '+' | '-')
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
    fn renames_duplicate_filenames_case_insensitively() {
        let manifest = br#"{"protocol_version":2,"files":[{"name":"image.jpg","type":"text/plain","size":1},{"name":"image (1).jpg","type":"text/plain","size":1},{"name":"IMAGE.JPG","type":"text/plain","size":1}],"created_at":"2026-06-30T12:00:00Z"}"#;

        let files = split_payload(manifest, b"abc").unwrap();

        assert_eq!(
            files
                .iter()
                .map(|file| file.filename.as_str())
                .collect::<Vec<_>>(),
            vec!["image.jpg", "image (1).jpg", "IMAGE (2).JPG"]
        );
        assert_eq!(files[0].data, b"a");
        assert_eq!(files[1].data, b"b");
        assert_eq!(files[2].data, b"c");
    }

    #[test]
    fn rejects_path_filenames() {
        let manifest = br#"{"protocol_version":2,"files":[{"name":"../a.txt","type":"text/plain","size":0}],"created_at":"2026-06-30T12:00:00Z"}"#;
        assert!(matches!(
            split_payload(manifest, b""),
            Err(ManifestError::InvalidFilename { .. })
        ));
    }
}
