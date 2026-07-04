use std::io::Read;
use std::path::{Component, Path, PathBuf};

use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use tauri::{AppHandle, State};
use tauri_plugin_dialog::DialogExt;
use ts_rs::TS;

use crate::action::{AttachmentSource, ExecutionAction};
use crate::path_security::resolve_template_path;
use crate::persistence::event_log::EventLog;
use crate::persistence::execution_store::{
    ExecutionStore, find_execution_dir, is_temp_execution_dir,
};
use crate::state::AppState;
use procnote_core::event::types::{CompletionStatus, Event, ExecutionId};
use procnote_core::execution::{
    ExecutionState, ExecutionStepContent, RecordedAttachment, StepStatus,
};
use procnote_core::template::parse_template;
use procnote_core::template::types::InputDefinition;

/// Serializable execution state summary for the frontend.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct ExecutionSummary {
    pub execution_id: ExecutionId,
    #[ts(optional)]
    pub name: Option<String>,
    pub procedure_id: String,
    pub procedure_title: String,
    pub procedure_version: String,
    pub status: String,
    /// ISO 8601 timestamp of when the execution was started.
    #[ts(optional)]
    pub started_at: Option<String>,
    /// ISO 8601 timestamp of when the execution was finished (completed/aborted).
    #[ts(optional)]
    pub finished_at: Option<String>,
    /// ISO 8601 timestamp of the last state transition applied to this execution.
    #[ts(optional)]
    pub updated_at: Option<String>,
    pub steps: Vec<StepSummary>,
    /// Absolute path to the execution directory on disk.
    pub execution_dir: String,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct StepSummary {
    pub id: String,
    pub heading: String,
    pub status: String,
    /// ISO 8601 timestamp of when the step was skipped, if applicable.
    #[ts(optional)]
    pub status_at: Option<String>,
    /// Ordered content items preserving template source order.
    pub content: Vec<StepContentSummary>,
    pub notes: Vec<NoteState>,
}

/// A single content item within a step, merging template structure with runtime state.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
#[serde(tag = "type")]
pub enum StepContentSummary {
    Prose {
        text: String,
    },
    Checkbox {
        #[ts(optional)]
        id: Option<String>,
        text: String,
        checked: bool,
        /// Zero-based nesting level within a Markdown task list.
        nesting_level: u32,
        /// ISO 8601 timestamp of the last toggle, if any.
        #[ts(optional)]
        at: Option<String>,
    },
    InputBlock {
        inputs: Vec<InputDefinitionSummary>,
    },
}

/// An input definition paired with its optional recorded value.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct InputDefinitionSummary {
    pub definition: InputDefinition,
    #[ts(optional)]
    pub recorded: Option<InputState>,
    pub attachments: Vec<AttachmentState>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct InputState {
    pub label: String,
    pub value: String,
    #[ts(optional)]
    pub unit: Option<String>,
    /// ISO 8601 timestamp of when the input was recorded.
    #[ts(optional)]
    pub at: Option<String>,
    /// Full SHA256 hash of the attached file, if this is an attachment.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub sha256: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AttachmentState {
    pub filename: String,
    pub path: String,
    pub content_type: String,
    pub sha256: String,
    /// ISO 8601 timestamp of when the attachment was recorded.
    #[ts(optional)]
    pub at: Option<String>,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct NoteState {
    pub id: String,
    pub text: String,
    /// ISO 8601 timestamp of when the note was added.
    #[ts(optional)]
    pub at: Option<String>,
}

fn status_string(status: &procnote_core::execution::ExecutionStatus) -> String {
    match status {
        procnote_core::execution::ExecutionStatus::Pending => "pending".to_string(),
        procnote_core::execution::ExecutionStatus::Active => "active".to_string(),
        procnote_core::execution::ExecutionStatus::Finished(s) => match s {
            CompletionStatus::Pass => "pass".to_string(),
            CompletionStatus::Fail => "fail".to_string(),
            CompletionStatus::Aborted => "aborted".to_string(),
        },
    }
}

fn step_status_string(status: &StepStatus) -> String {
    match status {
        StepStatus::Present => "present".to_string(),
        StepStatus::Skipped { .. } => "skipped".to_string(),
    }
}

fn datetime_string(value: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value.map(|at| at.to_rfc3339())
}

#[expect(
    clippy::too_many_lines,
    reason = "DTO conversion handles all execution content variants"
)]
pub(super) fn summarize(
    state: &ExecutionState,
    execution_dir: &Path,
) -> Result<ExecutionSummary, String> {
    let steps = state
        .step_order
        .iter()
        .filter_map(|step_id| {
            state.steps.get(step_id).map(|step| {
                let content = step
                    .content
                    .iter()
                    .map(|item| match item {
                        ExecutionStepContent::Prose { text } => {
                            StepContentSummary::Prose { text: text.clone() }
                        }
                        ExecutionStepContent::Checkbox(checkbox) => StepContentSummary::Checkbox {
                            id: Some(checkbox.id.clone()),
                            text: checkbox.text.clone(),
                            checked: checkbox.checked,
                            nesting_level: checkbox.nesting_level,
                            at: datetime_string(checkbox.toggled_at),
                        },
                        ExecutionStepContent::InputBlock { inputs } => {
                            StepContentSummary::InputBlock {
                                inputs: inputs
                                    .iter()
                                    .map(|def| {
                                        let recorded =
                                            step.inputs.get(&def.id).map(|input| InputState {
                                                label: input.label.clone(),
                                                value: input.value.clone(),
                                                unit: input.unit.clone(),
                                                at: Some(input.at.to_rfc3339()),
                                                sha256: None,
                                            });
                                        let attachments = step
                                            .attachments
                                            .get(&def.id)
                                            .map(|files| {
                                                files
                                                    .iter()
                                                    .map(|file| AttachmentState {
                                                        filename: file.filename.clone(),
                                                        path: file.path.clone(),
                                                        content_type: file.content_type.clone(),
                                                        sha256: file.sha256.clone(),
                                                        at: Some(file.at.to_rfc3339()),
                                                    })
                                                    .collect()
                                            })
                                            .unwrap_or_default();
                                        InputDefinitionSummary {
                                            definition: def.clone(),
                                            recorded,
                                            attachments,
                                        }
                                    })
                                    .collect(),
                            }
                        }
                    })
                    .collect();
                let notes = step
                    .notes
                    .iter()
                    .map(|note| NoteState {
                        id: note.id.clone(),
                        text: note.text.clone(),
                        at: Some(note.at.to_rfc3339()),
                    })
                    .collect();
                StepSummary {
                    id: step_id.clone(),
                    heading: step.heading.clone(),
                    status: step_status_string(&step.status),
                    status_at: match &step.status {
                        StepStatus::Present => None,
                        StepStatus::Skipped { at, .. } => Some(at.to_rfc3339()),
                    },
                    content,
                    notes,
                }
            })
        })
        .collect();

    Ok(ExecutionSummary {
        execution_id: state
            .execution_id
            .ok_or_else(|| "execution log is missing ExecutionStarted".to_string())?,
        name: state.name.clone(),
        procedure_id: state
            .procedure_id
            .clone()
            .ok_or_else(|| "execution log is missing procedure id".to_string())?,
        procedure_title: state
            .procedure_title
            .clone()
            .ok_or_else(|| "execution log is missing procedure title".to_string())?,
        procedure_version: state
            .procedure_version
            .clone()
            .ok_or_else(|| "execution log is missing procedure version".to_string())?,
        status: status_string(&state.status),
        started_at: datetime_string(state.started_at),
        finished_at: datetime_string(state.finished_at),
        updated_at: datetime_string(state.updated_at),
        steps,
        execution_dir: execution_dir.display().to_string(),
    })
}

/// Load an execution from disk by replaying its event log.
pub(super) fn load_execution_from_disk(
    procedures_dir: &Path,
    execution_id: ExecutionId,
) -> Result<(ExecutionState, PathBuf), String> {
    let exec_dir = find_execution_dir(procedures_dir, execution_id)?
        .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
    let log_path = exec_dir.join("events.jsonl");
    if !log_path.exists() {
        return Err(format!("Execution not found: {execution_id}"));
    }
    let events = EventLog::new(log_path.clone())
        .read_locked()
        .map_err(|e| e.to_string())?;
    let state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
    Ok((state, log_path))
}

fn find_attachment<'a>(
    state: &'a ExecutionState,
    attachment_path: &str,
) -> Option<&'a RecordedAttachment> {
    state
        .step_order
        .iter()
        .filter_map(|step_id| state.steps.get(step_id))
        .flat_map(|step| step.attachments.values())
        .flat_map(|files| files.iter())
        .find(|file| file.path == attachment_path)
}

fn preview_content_type(content_type: &str) -> Option<String> {
    let media_type = content_type
        .split_once(';')
        .map_or(content_type, |(media_type, _)| media_type)
        .trim()
        .to_ascii_lowercase();
    let (top_level, subtype) = media_type.split_once('/')?;
    let subtype_is_safe = subtype
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '+' | '.'));

    (top_level == "image" && subtype != "svg+xml" && !subtype.is_empty() && subtype_is_safe)
        .then_some(media_type)
}

fn resolve_attachment_file_path(
    execution_dir: &Path,
    relative_path: &str,
) -> Result<PathBuf, String> {
    let relative = Path::new(relative_path);
    if relative.is_absolute()
        || relative
            .components()
            .any(|component| !matches!(component, Component::Normal(_)))
    {
        return Err(format!("invalid attachment path: {relative_path}"));
    }

    let attachments_dir = execution_dir
        .join("attachments")
        .canonicalize()
        .map_err(|e| e.to_string())?;
    let file_path = execution_dir
        .join(relative)
        .canonicalize()
        .map_err(|e| e.to_string())?;

    if !file_path.starts_with(&attachments_dir) {
        return Err("attachment path escapes the attachments directory".to_string());
    }

    Ok(file_path)
}

fn attachment_preview_data_url(
    execution_dir: &Path,
    attachment: &RecordedAttachment,
) -> Result<Option<String>, String> {
    let Some(content_type) = preview_content_type(&attachment.content_type) else {
        return Ok(None);
    };
    let file_path = resolve_attachment_file_path(execution_dir, &attachment.path)?;
    let mut input = std::fs::File::open(file_path).map_err(|e| e.to_string())?;
    let mut base64_output = Vec::new();
    {
        let mut base64_writer =
            base64::write::EncoderWriter::new(&mut base64_output, &BASE64_STANDARD);
        let mut buffer = vec![0u8; 64 * 1024];
        loop {
            let read = input.read(&mut buffer).map_err(|e| e.to_string())?;
            if read == 0 {
                break;
            }
            std::io::Write::write_all(&mut base64_writer, &buffer[..read])
                .map_err(|e| e.to_string())?;
        }
        base64_writer.finish().map_err(|e| e.to_string())?;
    }
    let encoded = String::from_utf8(base64_output).map_err(|e| e.to_string())?;
    Ok(Some(format!("data:{content_type};base64,{encoded}")))
}

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
    let (exec_state, log_path) = load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let exec_dir = log_path
        .parent()
        .ok_or_else(|| "event log path has no parent".to_string())?;
    let Some(attachment) = find_attachment(&exec_state, &path) else {
        return Err(format!("Attachment not found: {path}"));
    };

    attachment_preview_data_url(exec_dir, attachment)
}

fn authorize_attachment_action(
    state: &AppState,
    action: &mut ExecutionAction,
) -> Result<(), String> {
    match action {
        ExecutionAction::AddAttachment {
            filename,
            path,
            content_type,
            ..
        } => authorize_attachment_source(state, filename, path, content_type),
        ExecutionAction::AddAttachments { files, .. } => files.iter_mut().try_for_each(|source| {
            authorize_attachment_source(
                state,
                &mut source.filename,
                &mut source.path,
                &mut source.content_type,
            )
        }),
        _ => Ok(()),
    }
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

fn infer_attachment_content_type(filename: &str) -> String {
    let extension = Path::new(filename)
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase);
    match extension.as_deref() {
        Some("jpg" | "jpeg") => "image/jpeg",
        Some("png") => "image/png",
        Some("gif") => "image/gif",
        Some("webp") => "image/webp",
        Some("bmp") => "image/bmp",
        Some("txt" | "log") => "text/plain",
        Some("csv") => "text/csv",
        Some("json") => "application/json",
        Some("pdf") => "application/pdf",
        _ => "application/octet-stream",
    }
    .to_string()
}

/// Open the native file picker and grant the selected files for one attachment action.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn pick_attachment_sources(
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
) -> Result<ExecutionSummary, String> {
    let template_path = resolve_template_path(&state.procedures_dir, Path::new(&template_path))?;
    let source = std::fs::read_to_string(&template_path).map_err(|e| e.to_string())?;
    let template = parse_template(&source).map_err(|e| e.to_string())?;

    let mut exec_state = ExecutionState::new();
    let events = exec_state.start(&template).map_err(|e| e.to_string())?;

    let execution_id = exec_state
        .execution_id
        .ok_or_else(|| "start() did not set execution_id".to_string())?;

    // Extract the timestamp from the ExecutionStarted event.
    let started_at = events
        .iter()
        .find_map(|e| match e {
            Event::ExecutionStarted { at, .. } => Some(*at),
            _ => None,
        })
        .ok_or_else(|| "start() did not produce an ExecutionStarted event".to_string())?;

    let recorded = ExecutionStore::new(state.procedures_dir.clone()).create_execution(
        &template_path,
        exec_state,
        events,
        started_at,
        execution_id,
        env!("CARGO_PKG_VERSION").to_string(),
    )?;

    summarize(&recorded.state, &recorded.execution_dir)
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
) -> Result<ExecutionSummary, String> {
    authorize_attachment_action(&state, &mut action)?;
    log::debug!("record_action: execution={execution_id}, action={action:?}");
    let recorded =
        ExecutionStore::new(state.procedures_dir.clone()).record_action(execution_id, action)?;
    summarize(&recorded.state, &recorded.execution_dir)
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
    let (exec_state, log_path) = load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let exec_dir = log_path
        .parent()
        .ok_or_else(|| "event log path has no parent".to_string())?;
    summarize(&exec_state, exec_dir)
}

/// List all executions by scanning each procedure's `.executions/` subdirectory.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_executions(state: State<'_, AppState>) -> Result<Vec<ExecutionSummary>, String> {
    let mut summaries = Vec::new();

    if !state.procedures_dir.exists() {
        return Ok(summaries);
    }

    let proc_entries = std::fs::read_dir(&state.procedures_dir).map_err(|e| e.to_string())?;
    for proc_entry in proc_entries {
        let proc_entry = proc_entry.map_err(|e| e.to_string())?;
        let exec_base = proc_entry.path().join(".executions");
        if !exec_base.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&exec_base) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!(
                    "Skipping unreadable executions directory {}: {e}",
                    exec_base.display()
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            let dir_path = entry.path();
            if !dir_path.is_dir() || is_temp_execution_dir(&dir_path) {
                continue;
            }
            let log_path = dir_path.join("events.jsonl");
            if !log_path.exists() {
                continue;
            }
            let events = match EventLog::new(log_path.clone()).read_locked() {
                Ok(events) => events,
                Err(e) => {
                    log::warn!("Failed to read events from {}: {e}", log_path.display());
                    continue;
                }
            };
            let exec_state = match ExecutionState::from_events(&events) {
                Ok(state) => state,
                Err(e) => {
                    log::warn!("Failed to replay events from {}: {e}", log_path.display());
                    continue;
                }
            };
            match summarize(&exec_state, &dir_path) {
                Ok(summary) => summaries.push(summary),
                Err(e) => {
                    log::warn!("Failed to summarize execution {}: {e}", dir_path.display());
                }
            }
        }
    }

    Ok(summaries)
}
