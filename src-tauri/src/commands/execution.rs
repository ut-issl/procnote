use std::path::Path;

use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;

use crate::state::AppState;
use procnote_app::action::{ActionMeta, AttachmentSource, ExecutionAction};
use procnote_app::execution::{ExecutionSummary, infer_attachment_content_type};
use procnote_app::persistence::execution_store::find_execution_dir;
use procnote_core::event::types::ExecutionId;

/// Get a data URL that can be used as an image thumbnail for an attachment.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn get_attachment_preview_data_url(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
    path: String,
) -> Result<Option<String>, String> {
    procnote_app::execution::attachment_preview_data_url(&state.procedures_dir, execution_id, &path)
}

fn authorize_attachment_action(
    state: &AppState,
    action: &mut ExecutionAction,
) -> Result<(), String> {
    action
        .attachment_sources_mut()
        .into_iter()
        .try_for_each(|source| {
            authorize_attachment_source(state, source.filename, source.path, source.content_type)
        })
}

fn authorize_attachment_source(
    state: &AppState,
    filename: &mut String,
    path: &mut String,
    content_type: &mut String,
) -> Result<(), String> {
    let canonical = Path::new(path)
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize attachment path: {e}"))?;
    if !canonical.is_file() {
        return Err("attachment path is not a file".to_string());
    }
    state.consume_attachment_path(&canonical)?;
    *filename = canonical
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| "attachment path has no filename".to_string())?
        .to_string();
    *content_type = infer_attachment_content_type(filename);
    *path = canonical.to_string_lossy().to_string();
    Ok(())
}

/// Open the native file picker and grant the selected files for one attachment action.
///
/// This must be an async command: synchronous commands run on the main thread,
/// and the blocking dialog would wait there for the main thread to show it.
#[tauri::command]
pub async fn pick_attachment_sources(
    app: AppHandle,
    state: State<'_, AppState>,
    title: String,
) -> Result<Vec<AttachmentSource>, String> {
    let selected = app
        .dialog()
        .file()
        .set_title(title)
        .blocking_pick_files()
        .unwrap_or_default();

    selected
        .into_iter()
        .map(|file_path| {
            let path = file_path.into_path().map_err(|e| e.to_string())?;
            let canonical = path
                .canonicalize()
                .map_err(|e| format!("failed to canonicalize selected file: {e}"))?;
            if !canonical.is_file() {
                return Err("selected attachment is not a file".to_string());
            }
            state.grant_attachment_path(canonical.clone())?;
            let filename = canonical
                .file_name()
                .and_then(|name| name.to_str())
                .ok_or_else(|| "selected attachment has no filename".to_string())?
                .to_string();
            Ok(AttachmentSource {
                content_type: infer_attachment_content_type(&filename),
                filename,
                path: canonical.to_string_lossy().to_string(),
            })
        })
        .collect()
}

/// Reveal an execution directory after resolving the execution ID server-side.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn reveal_execution_dir(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
) -> Result<(), String> {
    let exec_dir = find_execution_dir(&state.procedures_dir, execution_id)?
        .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
    tauri_plugin_opener::reveal_item_in_dir(exec_dir).map_err(|e| e.to_string())
}

/// Start a new execution from a template file.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn start_execution(
    state: State<'_, AppState>,
    template_path: String,
    meta: Option<ActionMeta>,
) -> Result<ExecutionSummary, String> {
    procnote_app::execution::start_execution(
        &state.procedures_dir,
        Path::new(&template_path),
        env!("CARGO_PKG_VERSION").to_string(),
        meta.unwrap_or_default(),
    )
}

/// Record an action on an active execution.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn record_action(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
    mut action: ExecutionAction,
    meta: Option<ActionMeta>,
) -> Result<ExecutionSummary, String> {
    authorize_attachment_action(&state, &mut action)?;
    procnote_app::execution::record_action(
        &state.procedures_dir,
        execution_id,
        action,
        meta.unwrap_or_default(),
    )
}

/// Get the current state of an execution.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn get_execution_state(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
) -> Result<ExecutionSummary, String> {
    procnote_app::execution::get_execution(&state.procedures_dir, execution_id)
}

/// List all executions by scanning each procedure's `.executions/` subdirectory.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_executions(state: State<'_, AppState>) -> Result<Vec<ExecutionSummary>, String> {
    procnote_app::execution::list_executions(&state.procedures_dir)
}
