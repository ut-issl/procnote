use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use procnote_core::event::SUPPORTED_VERSION;
use procnote_core::event::types::{Event, ExecutionId};
use procnote_core::execution::ExecutionState;

use crate::action::ExecutionAction;
use crate::persistence::attachment_store::AttachmentStore;
use crate::persistence::event_log::{EventLog, sync_dir};

pub struct ExecutionStore {
    procedures_dir: PathBuf,
}

pub struct RecordedExecution {
    pub state: ExecutionState,
    pub events: Vec<Event>,
    pub execution_dir: PathBuf,
}

impl ExecutionStore {
    #[must_use]
    pub const fn new(procedures_dir: PathBuf) -> Self {
        Self { procedures_dir }
    }

    /// Create an execution directory and publish it atomically after its core
    /// files have been durably written.
    #[expect(
        clippy::unused_self,
        reason = "ExecutionStore groups filesystem creation operations"
    )]
    pub fn create_execution(
        &self,
        template_path: &Path,
        state: ExecutionState,
        initial_events: Vec<Event>,
        started_at: DateTime<Utc>,
        execution_id: ExecutionId,
        tool_version: String,
    ) -> Result<RecordedExecution, String> {
        let procedure_dir = template_path
            .parent()
            .ok_or("template_path has no parent directory")?;
        let executions_dir = procedure_dir.join(".executions");
        std::fs::create_dir_all(&executions_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;

        let final_dir = executions_dir.join(execution_dir_name(&started_at, execution_id));
        if final_dir.exists() {
            return Err(format!(
                "Execution directory already exists: {}",
                final_dir.display()
            ));
        }
        let temp_dir = executions_dir.join(format!(
            ".{}.tmp-{}",
            final_dir
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("execution"),
            uuid::Uuid::new_v4()
        ));
        std::fs::create_dir(&temp_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;

        let template_snapshot = temp_dir.join("template.md");
        copy_file_durable(template_path, &template_snapshot).map_err(|e| e.to_string())?;

        let log_meta = Event::LogMeta {
            at: Utc::now(),
            version: SUPPORTED_VERSION,
            tool_version,
        };
        let mut events = vec![log_meta];
        events.extend(initial_events);
        EventLog::new(temp_dir.join("events.jsonl"))
            .create_with_events_durable(&events)
            .map_err(|e| e.to_string())?;

        sync_dir(&temp_dir).map_err(|e| e.to_string())?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;

        Ok(RecordedExecution {
            state,
            events,
            execution_dir: final_dir,
        })
    }

    /// Record an action as a single log transaction.
    ///
    /// The full read/replay/validate/append sequence runs under the event log's
    /// exclusive lock. The command layer should treat the durable append inside
    /// this method as the commit point.
    pub fn record_action(
        &self,
        execution_id: ExecutionId,
        action: ExecutionAction,
    ) -> Result<RecordedExecution, String> {
        let execution_dir = find_execution_dir(&self.procedures_dir, execution_id)
            .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
        let log_path = execution_dir.join("events.jsonl");
        if !log_path.exists() {
            return Err(format!("Execution not found: {execution_id}"));
        }
        let event_log = EventLog::new(log_path.clone());

        event_log
            .with_exclusive_lock(|| {
                let mut events = EventLog::new(log_path.clone())
                    .read()
                    .map_err(|e| e.to_string())?;
                let mut state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;

                let event = build_event_for_action(&state, &execution_dir, action)?;
                EventLog::new(log_path.clone())
                    .append_durable(&event)
                    .map_err(|e| e.to_string())?;
                state.apply(&event).map_err(|e| e.to_string())?;
                events.push(event);
                Ok(RecordedExecution {
                    state,
                    events,
                    execution_dir,
                })
            })
            .map_err(|e| e.to_string())?
    }
}

fn build_event_for_action(
    state: &ExecutionState,
    execution_dir: &Path,
    action: ExecutionAction,
) -> Result<Event, String> {
    match action {
        ExecutionAction::SkipStep { step_id, reason } => state
            .skip_step_event(&step_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::UnskipStep { step_id, reason } => state
            .unskip_step_event(&step_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::ToggleCheckbox {
            step_id,
            checkbox_id,
            checked,
        } => state
            .toggle_checkbox_event(&step_id, &checkbox_id, checked)
            .map_err(|e| e.to_string()),
        ExecutionAction::RecordInput {
            step_id,
            input_id,
            value,
            unit,
        } => state
            .record_input_event(&step_id, &input_id, &value, unit.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::ClearInput {
            step_id,
            input_id,
            reason,
        } => state
            .clear_input_event(&step_id, &input_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::AddNote { text, step_id } => state
            .add_note_event(&text, step_id.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::RemoveNote { note_id, reason } => state
            .remove_note_event(&note_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::AddStep {
            step_id,
            heading,
            content,
            after_step_id,
        } => state
            .add_step_event(&step_id, &heading, content, after_step_id.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::AddAttachment {
            step_id,
            input_id,
            filename,
            path,
            content_type,
        } => {
            let stored_attachment = AttachmentStore::new(execution_dir.to_path_buf())
                .copy_verify_sync(Path::new(&path), &filename)?;

            state
                .add_attachment_event(
                    &step_id,
                    &input_id,
                    &stored_attachment.filename,
                    &stored_attachment.relative_path,
                    &content_type,
                    &stored_attachment.sha256,
                )
                .map_err(|e| e.to_string())
        }
        ExecutionAction::RemoveAttachment {
            step_id,
            input_id,
            reason,
        } => state
            .remove_attachment_event(&step_id, &input_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::Complete { status } => {
            state.complete_event(status).map_err(|e| e.to_string())
        }
        ExecutionAction::Abort { reason } => state.abort_event(&reason).map_err(|e| e.to_string()),
        ExecutionAction::RenameExecution { name } => {
            state.rename_event(&name).map_err(|e| e.to_string())
        }
        ExecutionAction::ReopenExecution { reason } => {
            state.reopen_event(&reason).map_err(|e| e.to_string())
        }
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

fn copy_file_durable(source: &Path, destination: &Path) -> std::io::Result<()> {
    let bytes = std::fs::read(source)?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)?;
    file.write_all(&bytes)?;
    file.flush()?;
    file.sync_all()?;
    destination.parent().map_or(Ok(()), sync_dir)
}

/// Format the execution directory name as `{YYYYMMDD}T{HHMMSS}-{uuid_8}`.
fn execution_dir_name(at: &DateTime<Utc>, execution_id: ExecutionId) -> String {
    format!(
        "{}-{}",
        at.format("%Y%m%dT%H%M%S"),
        &execution_id.to_string()[..8]
    )
}
