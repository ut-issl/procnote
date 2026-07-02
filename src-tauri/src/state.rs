use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::drop_point::DropPointConfig;

/// Application state managed by Tauri.
pub struct AppState {
    /// Root directory containing procedure subdirectories.
    /// Each procedure is a subdirectory with `template.md` and `.executions/`.
    pub procedures_dir: PathBuf,
    /// Optional `DropPoint` receiver configuration loaded from environment variables.
    pub drop_point_config: Option<DropPointConfig>,
    /// Canonical file paths that were returned by the trusted native file picker
    /// and may be consumed by one attachment-recording IPC call.
    pub attachment_grants: Mutex<HashSet<PathBuf>>,
}

impl AppState {
    pub fn grant_attachment_path(&self, path: PathBuf) -> Result<(), String> {
        self.attachment_grants
            .lock()
            .map_err(|_| "attachment grant lock poisoned".to_string())?
            .insert(path);
        Ok(())
    }

    pub fn consume_attachment_path(&self, path: &Path) -> Result<(), String> {
        let removed = {
            let mut grants = self
                .attachment_grants
                .lock()
                .map_err(|_| "attachment grant lock poisoned".to_string())?;
            grants.remove(path)
        };
        if removed {
            Ok(())
        } else {
            Err("attachment path was not selected with the trusted file picker".to_string())
        }
    }
}
