use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use sha2::{Digest, Sha256};

use crate::attachment::compute_sha256;
use crate::attachment::hex_encode;
use crate::persistence::event_log::sync_dir;
use crate::repository::StagedFile;

pub struct AttachmentStore {
    execution_dir: PathBuf,
}

impl AttachmentStore {
    #[must_use]
    pub const fn new(execution_dir: PathBuf) -> Self {
        Self { execution_dir }
    }

    /// Durably copy a staged file to its stored path. Storing the same content
    /// again is a no-op.
    pub fn store(&self, file: &StagedFile) -> Result<(), String> {
        let attachments_dir = self.execution_dir.join("attachments");
        std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;

        let destination = self.execution_dir.join(&file.record.path);
        if destination.exists() {
            let existing_hash = compute_sha256(&destination).map_err(|e| e.to_string())?;
            if existing_hash == file.record.sha256 {
                return Ok(());
            }
            return Err(format!(
                "attachment hash-prefix collision at {}",
                destination.display()
            ));
        }

        let mut output = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&destination)
            .map_err(|e| e.to_string())?;
        write_pending_source(&file.source, &mut output, &file.record.sha256)?;
        output.flush().map_err(|e| e.to_string())?;
        output.sync_all().map_err(|e| e.to_string())?;
        sync_dir(&attachments_dir).map_err(|e| e.to_string())?;
        Ok(())
    }
}

fn write_pending_source(
    path: &Path,
    destination: &mut std::fs::File,
    expected_sha256: &str,
) -> Result<(), String> {
    let mut source = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];
    loop {
        let read = source.read(&mut buffer).map_err(|e| e.to_string())?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        destination
            .write_all(&buffer[..read])
            .map_err(|e| e.to_string())?;
    }
    let copied_hash = hex_encode(hasher.finalize().as_ref());
    if copied_hash == expected_sha256 {
        Ok(())
    } else {
        Err(format!(
            "attachment hash mismatch while copying: expected {expected_sha256}, got {copied_hash}"
        ))
    }
}
