use tauri::State;

use crate::state::AppState;
use procnote_app::template::TemplateSummary;

/// List all procedure templates found in the procedures directory.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_templates(state: State<'_, AppState>) -> Result<Vec<TemplateSummary>, String> {
    procnote_app::template::list_templates(&state.procedures_dir)
}
