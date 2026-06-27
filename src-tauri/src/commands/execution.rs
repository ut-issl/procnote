use std::path::{Path, PathBuf};

use serde::Serialize;
use tauri::State;
use ts_rs::TS;

use crate::action::ExecutionAction;
use crate::persistence::event_log::EventLog;
use crate::persistence::execution_store::ExecutionStore;
use crate::state::AppState;
use procnote_core::event::reverted_event_indices;
use procnote_core::event::types::{CompletionStatus, Event, ExecutionId, Revertibility};
use procnote_core::execution::{ExecutionState, StepStatus};
use procnote_core::template::parse_template;
use procnote_core::template::types::{InputDefinition, StepContent};

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
    pub steps: Vec<StepSummary>,
    pub event_history: Vec<EventHistoryEntry>,
    /// Absolute path to the execution directory on disk.
    pub execution_dir: String,
}

/// A single entry in the event history, exposed to the frontend.
#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct EventHistoryEntry {
    pub index: usize,
    pub event_type: String,
    /// ISO 8601 timestamp string.
    pub at: String,
    pub description: String,
    pub revertible: bool,
    pub reverted: bool,
    /// Step ID for step-scoped events, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub step_id: Option<String>,
    /// Element ID (`checkbox_id` or `input_id`) for element-scoped events, if applicable.
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub element_id: Option<String>,
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
pub struct NoteState {
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
        StepStatus::Skipped => "skipped".to_string(),
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "large match over all event variants to build summary"
)]
fn summarize(state: &ExecutionState, events: &[Event], execution_dir: &Path) -> ExecutionSummary {
    use std::collections::HashMap;

    let reverted_indices = reverted_event_indices(events);

    // Build timestamp lookup maps from non-reverted events.
    // Store as RFC3339 strings to avoid depending on chrono in this crate.
    let mut started_at: Option<String> = None;
    let mut finished_at: Option<String> = None;
    // step_id -> most recent skip timestamp
    let mut step_status_at: HashMap<&str, String> = HashMap::new();
    // checkbox_id -> most recent toggle timestamp
    let mut checkbox_at: HashMap<&str, String> = HashMap::new();
    // input_id -> most recent record timestamp
    let mut input_at: HashMap<&str, String> = HashMap::new();
    // input_id -> full SHA256 hash for attachments
    let mut attachment_sha256: HashMap<&str, String> = HashMap::new();
    // (step_id, note_index_in_step) -> add timestamp
    // We count notes per step to match the index in StepState.notes.
    let mut note_at: HashMap<(&str, usize), String> = HashMap::new();
    let mut note_counts: HashMap<&str, usize> = HashMap::new();

    for (index, event) in events.iter().enumerate() {
        if reverted_indices.contains(&index) {
            continue;
        }
        match event {
            Event::ExecutionStarted { at, .. } => {
                started_at = Some(at.to_rfc3339());
            }
            Event::ExecutionCompleted { at, .. } | Event::ExecutionAborted { at, .. } => {
                finished_at = Some(at.to_rfc3339());
            }
            Event::StepSkipped { at, step_id, .. } => {
                step_status_at.insert(step_id, at.to_rfc3339());
            }
            Event::CheckboxToggled {
                at, checkbox_id, ..
            } => {
                checkbox_at.insert(checkbox_id, at.to_rfc3339());
            }
            Event::InputRecorded { at, input_id, .. } => {
                input_at.insert(input_id, at.to_rfc3339());
            }
            Event::AttachmentAdded {
                at,
                input_id,
                sha256,
                ..
            } => {
                input_at.insert(input_id, at.to_rfc3339());
                attachment_sha256.insert(input_id, sha256.clone());
            }
            Event::NoteAdded {
                at,
                step_id: Some(id),
                ..
            } => {
                let count = note_counts.entry(id).or_insert(0);
                note_at.insert((id, *count), at.to_rfc3339());
                *count += 1;
            }
            _ => {}
        }
    }

    let steps = state
        .step_order
        .iter()
        .filter_map(|step_id| {
            state.steps.get(step_id).map(|step| {
                let content = step
                    .content
                    .iter()
                    .map(|item| match item {
                        StepContent::Prose { text } => {
                            StepContentSummary::Prose { text: text.clone() }
                        }
                        StepContent::Checkbox { id, text, checked } => {
                            StepContentSummary::Checkbox {
                                id: id.clone(),
                                text: text.clone(),
                                checked: *checked,
                                at: id
                                    .as_ref()
                                    .and_then(|cb_id| checkbox_at.get(cb_id.as_str()).cloned()),
                            }
                        }
                        StepContent::InputBlock { inputs } => StepContentSummary::InputBlock {
                            inputs: inputs
                                .iter()
                                .map(|def| {
                                    let recorded =
                                        step.inputs.get(&def.id).map(|input| InputState {
                                            label: input.label.clone(),
                                            value: input.value.clone(),
                                            unit: input.unit.clone(),
                                            at: input_at.get(def.id.as_str()).cloned(),
                                            sha256: attachment_sha256.get(def.id.as_str()).cloned(),
                                        });
                                    InputDefinitionSummary {
                                        definition: def.clone(),
                                        recorded,
                                    }
                                })
                                .collect(),
                        },
                    })
                    .collect();
                let notes = step
                    .notes
                    .iter()
                    .enumerate()
                    .map(|(i, text)| NoteState {
                        text: text.clone(),
                        at: note_at.get(&(step_id.as_str(), i)).cloned(),
                    })
                    .collect();
                StepSummary {
                    id: step_id.clone(),
                    heading: step.heading.clone(),
                    status: step_status_string(&step.status),
                    status_at: step_status_at.get(step_id.as_str()).cloned(),
                    content,
                    notes,
                }
            })
        })
        .collect();

    let event_history = build_event_history(events, &reverted_indices);

    ExecutionSummary {
        execution_id: state.execution_id.unwrap_or_default(),
        name: state.name.clone(),
        procedure_id: state.procedure_id.clone().unwrap_or_default(),
        procedure_title: state.procedure_title.clone().unwrap_or_default(),
        procedure_version: state.procedure_version.clone().unwrap_or_default(),
        status: status_string(&state.status),
        started_at,
        finished_at,
        steps,
        event_history,
        execution_dir: execution_dir.display().to_string(),
    }
}

fn build_event_history(
    events: &[Event],
    reverted_indices: &std::collections::HashSet<usize>,
) -> Vec<EventHistoryEntry> {
    events
        .iter()
        .enumerate()
        .map(|(index, event)| {
            let revertible = event.revertibility() == Revertibility::Revertible
                && !reverted_indices.contains(&index);
            let (step_id, element_id) = event_step_and_label(event);
            EventHistoryEntry {
                index,
                event_type: event_type_string(event),
                at: event_at(event),
                description: event.description(),
                revertible,
                reverted: reverted_indices.contains(&index),
                step_id,
                element_id,
            }
        })
        .collect()
}

/// Extract optional `step_id` and `element_id` from an event.
fn event_step_and_label(event: &Event) -> (Option<String>, Option<String>) {
    match event {
        Event::StepSkipped { step_id, .. } => (Some(step_id.clone()), None),
        Event::CheckboxToggled {
            step_id,
            checkbox_id,
            ..
        } => (Some(step_id.clone()), Some(checkbox_id.clone())),
        Event::InputRecorded {
            step_id, input_id, ..
        }
        | Event::AttachmentAdded {
            step_id, input_id, ..
        } => (Some(step_id.clone()), Some(input_id.clone())),
        Event::NoteAdded { step_id, .. } => (step_id.clone(), None),
        _ => (None, None),
    }
}

fn event_type_string(event: &Event) -> String {
    match event {
        Event::ExecutionStarted { .. } => "execution_started",
        Event::ExecutionCompleted { .. } => "execution_completed",
        Event::ExecutionAborted { .. } => "execution_aborted",
        Event::StepAdded { .. } => "step_added",
        Event::StepSkipped { .. } => "step_skipped",
        Event::CheckboxToggled { .. } => "checkbox_toggled",
        Event::InputRecorded { .. } => "input_recorded",
        Event::NoteAdded { .. } => "note_added",
        Event::AttachmentAdded { .. } => "attachment_added",
        Event::ExecutionRenamed { .. } => "execution_renamed",
        Event::EventReverted { .. } => "event_reverted",
        Event::LogMeta { .. } => "log_meta",
    }
    .to_string()
}

fn event_at(event: &Event) -> String {
    match event {
        Event::ExecutionStarted { at, .. }
        | Event::ExecutionCompleted { at, .. }
        | Event::ExecutionAborted { at, .. }
        | Event::StepAdded { at, .. }
        | Event::StepSkipped { at, .. }
        | Event::CheckboxToggled { at, .. }
        | Event::InputRecorded { at, .. }
        | Event::NoteAdded { at, .. }
        | Event::AttachmentAdded { at, .. }
        | Event::ExecutionRenamed { at, .. }
        | Event::EventReverted { at, .. }
        | Event::LogMeta { at, .. } => at.to_rfc3339(),
    }
}

/// Find the execution directory by scanning all procedure subdirectories.
fn find_execution_dir(procedures_dir: &Path, execution_id: ExecutionId) -> Option<PathBuf> {
    let suffix = format!("-{}", &execution_id.to_string()[..8]);
    let proc_entries = std::fs::read_dir(procedures_dir).ok()?;
    for proc_entry in proc_entries.flatten() {
        let exec_base = proc_entry.path().join(".executions");
        if !exec_base.is_dir() {
            continue;
        }
        let entries = std::fs::read_dir(&exec_base).ok()?;
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir()
                && let Some(name) = path.file_name().and_then(|n| n.to_str())
                && name.ends_with(&suffix)
            {
                return Some(path);
            }
        }
    }
    None
}

/// Load an execution from disk by replaying its event log.
fn load_execution_from_disk(
    procedures_dir: &Path,
    execution_id: ExecutionId,
) -> Result<(ExecutionState, Vec<Event>, PathBuf), String> {
    let exec_dir = find_execution_dir(procedures_dir, execution_id)
        .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
    let log_path = exec_dir.join("events.jsonl");
    if !log_path.exists() {
        return Err(format!("Execution not found: {execution_id}"));
    }
    let events = EventLog::new(log_path.clone())
        .read()
        .map_err(|e| e.to_string())?;
    let state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
    Ok((state, events, log_path))
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
    let source = std::fs::read_to_string(&template_path).map_err(|e| e.to_string())?;
    let template = parse_template(&source).map_err(|e| e.to_string())?;

    let mut exec_state = ExecutionState::new();
    let events = exec_state.start(&template).map_err(|e| e.to_string())?;

    let execution_id = exec_state
        .execution_id
        .expect("start() must set execution_id");

    // Extract the timestamp from the ExecutionStarted event.
    let started_at = events
        .iter()
        .find_map(|e| match e {
            Event::ExecutionStarted { at, .. } => Some(*at),
            _ => None,
        })
        .expect("start() must produce an ExecutionStarted event");

    let recorded = ExecutionStore::new(state.procedures_dir.clone()).create_execution(
        Path::new(&template_path),
        exec_state,
        events,
        started_at,
        execution_id,
        env!("CARGO_PKG_VERSION").to_string(),
    )?;

    Ok(summarize(
        &recorded.state,
        &recorded.events,
        &recorded.execution_dir,
    ))
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
    action: ExecutionAction,
) -> Result<ExecutionSummary, String> {
    log::debug!("record_action: execution={execution_id}, action={action:?}");
    let recorded =
        ExecutionStore::new(state.procedures_dir.clone()).record_action(execution_id, action)?;
    Ok(summarize(
        &recorded.state,
        &recorded.events,
        &recorded.execution_dir,
    ))
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
    let (exec_state, events, log_path) =
        load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let exec_dir = log_path.parent().expect("log_path must have a parent");
    Ok(summarize(&exec_state, &events, exec_dir))
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
        let entries = std::fs::read_dir(&exec_base).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let dir_path = entry.path();
            if !dir_path.is_dir() {
                continue;
            }
            let log_path = dir_path.join("events.jsonl");
            if !log_path.exists() {
                continue;
            }
            let events = match EventLog::new(log_path.clone()).read() {
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
            summaries.push(summarize(&exec_state, &events, &dir_path));
        }
    }

    Ok(summaries)
}
