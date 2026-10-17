use std::path::{Path, PathBuf};

use base64::Engine;
use base64::engine::general_purpose::STANDARD as BASE64_STANDARD;
use serde::Serialize;
use ts_rs::TS;

use crate::action::{ActionMeta, ExecutionAction};
use crate::path_security::resolve_template_path;
use crate::persistence::event_log::EventLog;
use crate::persistence::execution_store::find_execution_dir;
use crate::repository::{self, ExecutionRepository, NewExecution, StoredExecution};
use procnote_core::event::types::{CompletionStatus, ExecutionId};
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
    /// Files received through the upload link and not yet assigned to an input.
    pub inbox: Vec<AttachmentState>,
    /// Absolute path to the execution directory, when stored on a filesystem.
    #[ts(optional)]
    pub execution_dir: Option<String>,
    /// Number of events recorded so far; a larger value is a newer state.
    #[ts(type = "number")]
    pub seq: u64,
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

fn attachment_state(file: &RecordedAttachment) -> AttachmentState {
    AttachmentState {
        filename: file.filename.clone(),
        path: file.path.clone(),
        content_type: file.content_type.clone(),
        sha256: file.sha256.clone(),
        at: Some(file.at.to_rfc3339()),
    }
}

fn datetime_string(value: Option<chrono::DateTime<chrono::Utc>>) -> Option<String> {
    value.map(|at| at.to_rfc3339())
}

pub fn summarize(
    state: &ExecutionState,
    execution_dir: Option<&str>,
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
                                                files.iter().map(attachment_state).collect()
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
        inbox: state.inbox.iter().map(attachment_state).collect(),
        execution_dir: execution_dir.map(str::to_string),
        seq: state.applied_events,
    })
}

/// Load an execution from disk by replaying its event log.
pub fn load_execution_from_disk(
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
        .chain(state.inbox.iter())
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

fn preview_data_url(content_type: &str, bytes: &[u8]) -> String {
    format!(
        "data:{content_type};base64,{}",
        BASE64_STANDARD.encode(bytes)
    )
}

/// Get a data URL that can be used as an image thumbnail for an attachment.
///
/// Returns `None` for attachments whose content type is not a safe image type.
pub fn attachment_preview_data_url(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    path: &str,
) -> Result<Option<String>, String> {
    let stored = repository.load(execution_id)?;
    let Some(attachment) = find_attachment(&stored.state, path) else {
        return Err(format!("Attachment not found: {path}"));
    };
    let Some(content_type) = preview_content_type(&attachment.content_type) else {
        return Ok(None);
    };
    let bytes = repository.read_attachment(execution_id, &attachment.path)?;
    Ok(Some(preview_data_url(&content_type, &bytes)))
}

/// Infer an attachment's content type from its filename extension.
#[must_use]
pub fn infer_attachment_content_type(filename: &str) -> String {
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

fn summary_of(stored: &StoredExecution) -> Result<ExecutionSummary, String> {
    summarize(&stored.state, stored.location.as_deref())
}

/// Start a new execution from a template file under `procedures_dir`.
///
/// `tool_version` is recorded in the event log's `LogMeta`.
pub fn start_execution(
    repository: &dyn ExecutionRepository,
    procedures_dir: &Path,
    template_path: &Path,
    tool_version: String,
    meta: ActionMeta,
) -> Result<ExecutionSummary, String> {
    let template_path = resolve_template_path(procedures_dir, template_path)?;
    let procedure = template_path
        .parent()
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or("template path has no procedure directory")?
        .to_string();
    let template_source = std::fs::read_to_string(&template_path).map_err(|e| e.to_string())?;
    let template = parse_template(&template_source).map_err(|e| e.to_string())?;

    let mut state = ExecutionState::new();
    let initial_events = state.start(&template).map_err(|e| e.to_string())?;

    let stored = repository.create(NewExecution {
        procedure,
        template_source,
        state,
        initial_events,
        tool_version,
        meta: meta.into_event_meta(),
    })?;
    summary_of(&stored)
}

/// Record an action on an active execution.
///
/// Attachment sources in `action` must already be authorized by the caller.
pub fn record_action(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    action: ExecutionAction,
    meta: ActionMeta,
) -> Result<ExecutionSummary, String> {
    log::debug!("record_action: execution={execution_id}, action={action:?}, meta={meta:?}");
    let stored =
        repository::record_action(repository, execution_id, action, &meta.into_event_meta())?;
    summary_of(&stored)
}

/// Receive files into the execution's inbox (see [`repository::receive_files`]).
///
/// Returns the new summary and how many files were received; files whose
/// content the execution already has are skipped.
pub fn receive_files(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    files: &[(PathBuf, String)],
    meta: ActionMeta,
) -> Result<(ExecutionSummary, usize), String> {
    let (stored, received) =
        repository::receive_files(repository, execution_id, files, &meta.into_event_meta())?;
    Ok((summary_of(&stored)?, received))
}

/// Get the current state of an execution.
pub fn get_execution(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
) -> Result<ExecutionSummary, String> {
    summary_of(&repository.load(execution_id)?)
}

/// List all executions.
pub fn list_executions(
    repository: &dyn ExecutionRepository,
) -> Result<Vec<ExecutionSummary>, String> {
    Ok(repository
        .list()?
        .iter()
        .filter_map(|stored| {
            summary_of(stored)
                .inspect_err(|e| log::warn!("Failed to summarize execution: {e}"))
                .ok()
        })
        .collect())
}
