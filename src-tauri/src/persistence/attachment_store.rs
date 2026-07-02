use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::persistence::event_log::sync_dir;

pub struct AttachmentStore {
    execution_dir: PathBuf,
}

#[derive(Debug, Clone)]
pub struct StoredAttachment {
    pub filename: String,
    pub relative_path: String,
    pub sha256: String,
}

pub struct PendingStoredAttachment {
    pub stored: StoredAttachment,
    bytes: Vec<u8>,
}

impl AttachmentStore {
    #[must_use]
    pub const fn new(execution_dir: PathBuf) -> Self {
        Self { execution_dir }
    }

    pub fn prepare_copy(
        &self,
        source: &Path,
        filename: &str,
    ) -> Result<PendingStoredAttachment, String> {
        let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
        self.prepare_bytes(filename, bytes)
    }

    #[expect(
        clippy::unused_self,
        clippy::unnecessary_wraps,
        reason = "keeps the attachment preparation API uniform with fallible file preparation"
    )]
    pub fn prepare_bytes(
        &self,
        filename: &str,
        bytes: Vec<u8>,
    ) -> Result<PendingStoredAttachment, String> {
        let filename = sanitize_attachment_filename(filename);
        let sha256 = hex_encode(Sha256::digest(&bytes).as_ref());
        let short_hash = &sha256[..7];
        let stored_name = format!("{short_hash}-{filename}");
        Ok(PendingStoredAttachment {
            stored: StoredAttachment {
                filename,
                relative_path: format!("attachments/{stored_name}"),
                sha256,
            },
            bytes,
        })
    }

    pub fn commit_prepared(
        &self,
        pending: PendingStoredAttachment,
    ) -> Result<StoredAttachment, String> {
        let attachments_dir = self.execution_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;

        let destination = self.execution_dir.join(&pending.stored.relative_path);
        if destination.exists() {
            let existing_hash = compute_sha256(&destination).map_err(|e| e.to_string())?;
            if existing_hash == pending.stored.sha256 {
                return Ok(pending.stored);
            }
            return Err(format!(
                "attachment hash-prefix collision at {}",
                destination.display()
            ));
        }

        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        file.write_all(&pending.bytes).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;

        Ok(pending.stored)
    }
}

pub fn sanitize_attachment_filename(filename: &str) -> String {
    let mut sanitized: String = filename
        .trim()
        .chars()
        .map(|c| if is_safe_filename_char(c) { c } else { '_' })
        .collect();

    while sanitized.ends_with(['.', ' ']) {
        sanitized.pop();
    }

    if sanitized.is_empty() || sanitized == "." || sanitized == ".." {
        sanitized = "attachment".to_string();
    }

    if is_windows_reserved_name(&sanitized) {
        sanitized.insert(0, '_');
    }

    sanitized
}

fn is_safe_filename_char(c: char) -> bool {
    !c.is_control()
        && !is_format_or_bidi_control(c)
        && !matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
}

const fn is_format_or_bidi_control(c: char) -> bool {
    matches!(
        c,
        '\u{00AD}'
            | '\u{034F}'
            | '\u{061C}'
            | '\u{115F}'..='\u{1160}'
            | '\u{17B4}'..='\u{17B5}'
            | '\u{180B}'..='\u{180F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2060}'..='\u{206F}'
            | '\u{3164}'
            | '\u{FE00}'..='\u{FE0F}'
            | '\u{FEFF}'
            | '\u{FFA0}'
            | '\u{1BCA0}'..='\u{1BCA3}'
            | '\u{1D173}'..='\u{1D17A}'
            | '\u{E0000}'..='\u{E0FFF}'
    )
}

fn is_windows_reserved_name(filename: &str) -> bool {
    let stem = filename
        .split_once('.')
        .map_or(filename, |(stem, _)| stem)
        .to_ascii_uppercase();
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

fn compute_sha256(path: &Path) -> std::io::Result<String> {
    let bytes = std::fs::read(path)?;
    Ok(hex_encode(Sha256::digest(&bytes).as_ref()))
}

fn hex_encode(bytes: &[u8]) -> String {
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut output, b| {
            std::fmt::Write::write_fmt(&mut output, format_args!("{b:02x}"))
                .expect("writing to a String should never fail");
            output
        })
}

#[cfg(test)]
mod tests {
    use super::sanitize_attachment_filename;

    #[test]
    fn sanitizes_to_single_safe_component() {
        assert_eq!(
            sanitize_attachment_filename("../secret.txt"),
            ".._secret.txt"
        );
        assert_eq!(sanitize_attachment_filename("a/b\\c?.txt"), "a_b_c_.txt");
        assert_eq!(sanitize_attachment_filename("photo.jpg."), "photo.jpg");
        assert_eq!(sanitize_attachment_filename("\u{202E}gpj.exe"), "_gpj.exe");
        assert_eq!(sanitize_attachment_filename("CON.txt"), "_CON.txt");
    }
}
