use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::State;
use ts_rs::TS;

use crate::state::AppState;
use procnote_core::event::types::{CompletionStatus, Event, ExecutionId, Revertibility};
use procnote_core::event::{append_event, read_log, reverted_event_indices};
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

/// Compute the SHA-256 hash of a file, returning a lowercase hex string.
fn compute_sha256(path: &str) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let bytes = std::fs::read(path)?;
    let hash = Sha256::digest(&bytes);
    Ok(hex_encode(hash.as_ref()))
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

/// Format the execution directory name as `{YYYYMMDD}T{HHMMSS}-{uuid_8}`.
fn execution_dir_name(at: &DateTime<Utc>, execution_id: ExecutionId) -> String {
    format!(
        "{}-{}",
        at.format("%Y%m%dT%H%M%S"),
        &execution_id.to_string()[..8]
    )
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
    let events = read_log(&log_path).map_err(|e| e.to_string())?;
    let state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
    Ok((state, events, log_path))
}

/// Start a new execution from a template file.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn start_execution(template_path: String) -> Result<ExecutionSummary, String> {
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

    // Create execution directory under the procedure's .executions/ subdirectory.
    let procedure_dir = Path::new(&template_path)
        .parent()
        .ok_or("template_path has no parent directory")?;
    let exec_dir = procedure_dir
        .join(".executions")
        .join(execution_dir_name(&started_at, execution_id));
    std::fs::create_dir_all(&exec_dir).map_err(|e| e.to_string())?;

    // Copy template snapshot.
    let template_snapshot = exec_dir.join("template.md");
    std::fs::copy(&template_path, &template_snapshot).map_err(|e| e.to_string())?;

    // Write events to log, starting with a log metadata line.
    let log_path = exec_dir.join("events.jsonl");
    let log_meta = Event::LogMeta {
        at: Utc::now(),
        version: 1,
        tool_version: env!("CARGO_PKG_VERSION").to_string(),
    };
    append_event(&log_path, &log_meta).map_err(|e| e.to_string())?;
    for event in &events {
        append_event(&log_path, event).map_err(|e| e.to_string())?;
    }

    // Build full event list for summarize.
    let mut all_events = vec![log_meta];
    all_events.extend(events);
    Ok(summarize(&exec_state, &all_events, &exec_dir))
}

/// Action payload from the frontend for recording events.
#[derive(Debug, Deserialize, TS)]
#[ts(export)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum ExecutionAction {
    SkipStep {
        step_id: String,
        reason: String,
    },
    ToggleCheckbox {
        step_id: String,
        checkbox_id: String,
        checked: bool,
    },
    RecordInput {
        step_id: String,
        input_id: String,
        value: String,
        #[ts(optional)]
        unit: Option<String>,
    },
    AddNote {
        text: String,
        #[ts(optional)]
        step_id: Option<String>,
    },
    AddStep {
        step_id: String,
        heading: String,
        #[serde(default)]
        content: Vec<StepContent>,
        #[ts(optional)]
        after_step_id: Option<String>,
    },
    AddAttachment {
        step_id: String,
        input_id: String,
        filename: String,
        path: String,
        content_type: String,
    },
    Complete {
        status: CompletionStatus,
    },
    Abort {
        reason: String,
    },
    RenameExecution {
        name: String,
    },
    RevertEvent {
        event_index: usize,
        reason: String,
    },
}

/// Record an action on an active execution.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handler with exhaustive action dispatch"
)]
pub fn record_action(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
    action: ExecutionAction,
) -> Result<ExecutionSummary, String> {
    log::debug!("record_action: execution={execution_id}, action={action:?}");
    let (mut exec_state, mut events, log_path) =
        load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let exec_dir = log_path.parent().expect("log_path must have a parent");

    // Revert is a special case: it rebuilds state from events.
    if let ExecutionAction::RevertEvent {
        event_index,
        reason,
    } = action
    {
        let revert_marker = ExecutionState::revert_event(&events, event_index, &reason)
            .map_err(|e| e.to_string())?;

        // Persist the revert marker.
        append_event(&log_path, &revert_marker).map_err(|e| e.to_string())?;
        events.push(revert_marker);

        // Rebuild state from the full event log.
        let exec_state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;

        return Ok(summarize(&exec_state, &events, exec_dir));
    }

    let event: Event = match action {
        ExecutionAction::SkipStep { step_id, reason } => exec_state
            .skip_step(&step_id, &reason)
            .map_err(|e| e.to_string())?,
        ExecutionAction::ToggleCheckbox {
            step_id,
            checkbox_id,
            checked,
        } => exec_state
            .toggle_checkbox(&step_id, &checkbox_id, checked)
            .map_err(|e| e.to_string())?,
        ExecutionAction::RecordInput {
            step_id,
            input_id,
            value,
            unit,
        } => exec_state
            .record_input(&step_id, &input_id, &value, unit.as_deref())
            .map_err(|e| e.to_string())?,
        ExecutionAction::AddNote { text, step_id } => exec_state
            .add_note(&text, step_id.as_deref())
            .map_err(|e| e.to_string())?,
        ExecutionAction::AddStep {
            step_id,
            heading,
            content,
            after_step_id,
        } => exec_state
            .add_step(&step_id, &heading, content, after_step_id.as_deref())
            .map_err(|e| e.to_string())?,
        ExecutionAction::AddAttachment {
            step_id,
            input_id,
            filename,
            path,
            content_type,
        } => {
            let sha256 = compute_sha256(&path).map_err(|e| e.to_string())?;

            // Copy file into <exec_dir>/attachments/<hash7>-<filename>.
            let short_hash = &sha256[..7];
            let stored_name = format!("{short_hash}-{filename}");
            let attachments_dir = exec_dir.join("attachments");
            std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
            let dest = attachments_dir.join(&stored_name);
            std::fs::copy(&path, &dest).map_err(|e| e.to_string())?;
            let relative_path = format!("attachments/{stored_name}");

            exec_state
                .add_attachment(
                    &step_id,
                    &input_id,
                    &filename,
                    &relative_path,
                    &content_type,
                    &sha256,
                )
                .map_err(|e| e.to_string())?
        }
        ExecutionAction::Complete { status } => {
            exec_state.complete(status).map_err(|e| e.to_string())?
        }
        ExecutionAction::Abort { reason } => {
            exec_state.abort(&reason).map_err(|e| e.to_string())?
        }
        ExecutionAction::RenameExecution { name } => {
            exec_state.rename(&name).map_err(|e| e.to_string())?
        }
        ExecutionAction::RevertEvent { .. } => unreachable!("handled above"),
    };

    // Persist event.
    append_event(&log_path, &event).map_err(|e| e.to_string())?;
    events.push(event);

    Ok(summarize(&exec_state, &events, exec_dir))
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
            let events = match read_log(&log_path) {
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
