use std::path::{Path, PathBuf};

use procnote_core::event::types::{Event, ExecutionId};
use procnote_core::execution::ExecutionState;
use sha2::{Digest, Sha256};

use crate::action::ExecutionAction;
use crate::persistence::event_log::EventLog;

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

                match action {
                    ExecutionAction::RevertEvent {
                        event_index,
                        reason,
                    } => {
                        let revert_marker =
                            ExecutionState::revert_event(&events, event_index, &reason)
                                .map_err(|e| e.to_string())?;
                        EventLog::new(log_path.clone())
                            .append_durable(&revert_marker)
                            .map_err(|e| e.to_string())?;
                        events.push(revert_marker);
                        let state =
                            ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
                        Ok(RecordedExecution {
                            state,
                            events,
                            execution_dir,
                        })
                    }
                    action => {
                        let event = build_event_for_action(&mut state, &execution_dir, action)?;
                        EventLog::new(log_path.clone())
                            .append_durable(&event)
                            .map_err(|e| e.to_string())?;
                        events.push(event);
                        Ok(RecordedExecution {
                            state,
                            events,
                            execution_dir,
                        })
                    }
                }
            })
            .map_err(|e| e.to_string())?
    }
}

fn build_event_for_action(
    state: &mut ExecutionState,
    execution_dir: &Path,
    action: ExecutionAction,
) -> Result<Event, String> {
    match action {
        ExecutionAction::SkipStep { step_id, reason } => state
            .skip_step(&step_id, &reason)
            .map_err(|e| e.to_string()),
        ExecutionAction::ToggleCheckbox {
            step_id,
            checkbox_id,
            checked,
        } => state
            .toggle_checkbox(&step_id, &checkbox_id, checked)
            .map_err(|e| e.to_string()),
        ExecutionAction::RecordInput {
            step_id,
            input_id,
            value,
            unit,
        } => state
            .record_input(&step_id, &input_id, &value, unit.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::AddNote { text, step_id } => state
            .add_note(&text, step_id.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::AddStep {
            step_id,
            heading,
            content,
            after_step_id,
        } => state
            .add_step(&step_id, &heading, content, after_step_id.as_deref())
            .map_err(|e| e.to_string()),
        ExecutionAction::AddAttachment {
            step_id,
            input_id,
            filename,
            path,
            content_type,
        } => {
            let sha256 = compute_sha256(Path::new(&path)).map_err(|e| e.to_string())?;

            // Copy file into <exec_dir>/attachments/<hash7>-<filename>.
            let short_hash = &sha256[..7];
            let stored_name = format!("{short_hash}-{filename}");
            let attachments_dir = execution_dir.join("attachments");
            std::fs::create_dir_all(&attachments_dir).map_err(|e| e.to_string())?;
            let dest = attachments_dir.join(&stored_name);
            std::fs::copy(&path, &dest).map_err(|e| e.to_string())?;
            let relative_path = format!("attachments/{stored_name}");

            state
                .add_attachment(
                    &step_id,
                    &input_id,
                    &filename,
                    &relative_path,
                    &content_type,
                    &sha256,
                )
                .map_err(|e| e.to_string())
        }
        ExecutionAction::Complete { status } => state.complete(status).map_err(|e| e.to_string()),
        ExecutionAction::Abort { reason } => state.abort(&reason).map_err(|e| e.to_string()),
        ExecutionAction::RenameExecution { name } => state.rename(&name).map_err(|e| e.to_string()),
        ExecutionAction::RevertEvent { .. } => {
            unreachable!("handled before build_event_for_action")
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

/// Compute the SHA-256 hash of a file, returning a lowercase hex string.
fn compute_sha256(path: &Path) -> std::io::Result<String> {
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
