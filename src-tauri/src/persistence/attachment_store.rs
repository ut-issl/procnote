use std::io::Write;
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::persistence::event_log::sync_dir;

pub struct AttachmentStore {
    execution_dir: PathBuf,
}

pub struct StoredAttachment {
    pub filename: String,
    pub relative_path: String,
    pub sha256: String,
}

impl AttachmentStore {
    #[must_use]
    pub const fn new(execution_dir: PathBuf) -> Self {
        Self { execution_dir }
    }

    /// Copy an attachment into the execution directory and sync it before the
    /// caller commits the corresponding `AttachmentAdded` event.
    pub fn copy_verify_sync(
        &self,
        source: &Path,
        filename: &str,
    ) -> Result<StoredAttachment, String> {
        let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
        self.write_bytes_verify_sync(filename, &bytes)
    }

    /// Store attachment bytes in the execution directory and sync them before
    /// the caller commits the corresponding event.
    pub fn write_bytes_verify_sync(
        &self,
        filename: &str,
        bytes: &[u8],
    ) -> Result<StoredAttachment, String> {
        let sha256 = hex_encode(Sha256::digest(bytes).as_ref());
        let short_hash = &sha256[..7];
        let stored_name = format!("{short_hash}-{filename}");
        let attachments_dir = self.execution_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;

        let destination = attachments_dir.join(&stored_name);
        if destination.exists() {
            let existing_hash = compute_sha256(&destination).map_err(|e| e.to_string())?;
            if existing_hash == sha256 {
                return Ok(StoredAttachment {
                    filename: filename.to_string(),
                    relative_path: format!("attachments/{stored_name}"),
                    sha256,
                });
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
        file.write_all(bytes).map_err(|e| e.to_string())?;
        file.flush().map_err(|e| e.to_string())?;
        file.sync_all().map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;

        let copied_hash = compute_sha256(&destination).map_err(|e| e.to_string())?;
        if copied_hash != sha256 {
            return Err(format!(
                "attachment hash mismatch after copy: expected {sha256}, got {copied_hash}"
            ));
        }

        Ok(StoredAttachment {
            filename: filename.to_string(),
            relative_path: format!("attachments/{stored_name}"),
            sha256,
        })
    }
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
