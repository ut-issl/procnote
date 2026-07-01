use std::path::PathBuf;

use crate::drop_point::DropPointConfig;

/// Application state managed by Tauri.
pub struct AppState {
    /// Root directory containing procedure subdirectories.
    /// Each procedure is a subdirectory with `template.md` and `.executions/`.
    pub procedures_dir: PathBuf,
    /// Optional `DropPoint` receiver configuration loaded from environment variables.
    pub drop_point_config: Option<DropPointConfig>,
}
