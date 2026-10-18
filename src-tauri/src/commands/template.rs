use std::path::Path;

use tauri::State;

use crate::state::AppState;
use procnote_app::template::TemplateSummary;
use procnote_core::template::types::ProcedureTemplate;

/// List all procedure templates found in the procedures directory.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_templates(state: State<'_, AppState>) -> Result<Vec<TemplateSummary>, String> {
    procnote_app::template::list_templates(&state.procedures_dir)
}

/// Read one procedure template as structured data.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn get_template(
    state: State<'_, AppState>,
    template_path: String,
) -> Result<ProcedureTemplate, String> {
    procnote_app::template::get_template(&state.procedures_dir, Path::new(&template_path))
}
