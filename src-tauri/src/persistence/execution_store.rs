use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use procnote_core::event::SUPPORTED_VERSION;
use procnote_core::event::types::{AttachmentRecord, Event, ExecutionId};
use procnote_core::execution::ExecutionState;
use procnote_core::snapshot::render_execution_markdown;

use crate::action::{AttachmentSource, ExecutionAction};
use crate::persistence::attachment_store::{AttachmentStore, PendingStoredAttachment};
use crate::persistence::event_log::{EventLog, sync_dir};

pub struct ExecutionStore {
    procedures_dir: PathBuf,
}

pub struct RecordedExecution {
    pub state: ExecutionState,
    pub execution_dir: PathBuf,
}

pub struct AttachmentBytesSource {
    pub filename: String,
    pub bytes: Vec<u8>,
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
        write_execution_snapshot_durable(&temp_dir, &state)?;

        sync_dir(&temp_dir).map_err(|e| e.to_string())?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;

        Ok(RecordedExecution {
            state,
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
        self.with_log_transaction(execution_id, |state, execution_dir| {
            build_event_for_action(state, execution_dir, action)
        })
    }

    pub fn record_attachment_bytes_batch(
        &self,
        execution_id: ExecutionId,
        step_id: &str,
        input_id: &str,
        files: Vec<AttachmentBytesSource>,
    ) -> Result<RecordedExecution, String> {
        self.with_log_transaction(execution_id, |state, execution_dir| {
            let pending_attachments = prepare_attachment_bytes(execution_dir, files)?;
            let attachments = pending_attachments.iter().map(attachment_record).collect();
            let event = state
                .add_attachments_event(step_id, input_id, attachments)
                .map_err(|e| e.to_string())?;
            Ok(BuiltAction {
                event,
                pending_attachments,
            })
        })
    }

    fn with_log_transaction(
        &self,
        execution_id: ExecutionId,
        build_action: impl FnOnce(&ExecutionState, &Path) -> Result<BuiltAction, String>,
    ) -> Result<RecordedExecution, String> {
        let execution_dir = find_execution_dir(&self.procedures_dir, execution_id)?
            .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
        let log_path = execution_dir.join("events.jsonl");
        if !log_path.exists() {
            return Err(format!("Execution not found: {execution_id}"));
        }
        let event_log = EventLog::new(log_path);
        let transaction_log = event_log.clone();

        event_log
            .with_exclusive_lock(|| {
                let events = transaction_log.read().map_err(|e| e.to_string())?;
                let mut state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;

                let built = build_action(&state, &execution_dir)?;
                state.apply(&built.event).map_err(|e| e.to_string())?;
                commit_pending_attachments(&execution_dir, built.pending_attachments)?;
                transaction_log
                    .append_durable(&built.event)
                    .map_err(|e| e.to_string())?;
                if let Err(e) = write_execution_snapshot_durable(&execution_dir, &state) {
                    log::warn!(
                        "failed to update execution snapshot for {execution_id} after appending event: {e}"
                    );
                }
                Ok(RecordedExecution {
                    state,
                    execution_dir,
                })
            })
            .map_err(|e| e.to_string())?
    }
}

struct BuiltAction {
    event: Event,
    pending_attachments: Vec<PendingStoredAttachment>,
}

impl BuiltAction {
    const fn without_attachments(event: Event) -> Self {
        Self {
            event,
            pending_attachments: Vec::new(),
        }
    }
}

#[expect(
    clippy::too_many_lines,
    reason = "central dispatch over all frontend execution actions"
)]
fn build_event_for_action(
    state: &ExecutionState,
    execution_dir: &Path,
    action: ExecutionAction,
) -> Result<BuiltAction, String> {
    let event = match action {
        ExecutionAction::SkipStep { step_id, reason } => state
            .skip_step_event(&step_id, &reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::UnskipStep { step_id, reason } => state
            .unskip_step_event(&step_id, &reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::ToggleCheckbox {
            step_id,
            checkbox_id,
            checked,
        } => state
            .toggle_checkbox_event(&step_id, &checkbox_id, checked)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::RecordInput {
            step_id,
            input_id,
            value,
            unit,
        } => state
            .record_input_event(&step_id, &input_id, &value, unit.as_deref())
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::ClearInput {
            step_id,
            input_id,
            reason,
        } => state
            .clear_input_event(&step_id, &input_id, &reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::AddNote { text, step_id } => state
            .add_note_event(&text, step_id.as_deref())
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::RemoveNote { note_id, reason } => state
            .remove_note_event(&note_id, &reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::AddStep {
            step_id,
            heading,
            content,
            after_step_id,
        } => state
            .add_step_event(&step_id, &heading, content, after_step_id.as_deref())
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::AddAttachment {
            step_id,
            input_id,
            filename,
            path,
            content_type,
        } => {
            let prepared = prepare_attachment_sources(
                execution_dir,
                vec![AttachmentSource {
                    filename,
                    path,
                    content_type,
                }],
            )?;
            let attachments = prepared.iter().map(attachment_record).collect();
            state
                .add_attachments_event(&step_id, &input_id, attachments)
                .map(|event| BuiltAction {
                    event,
                    pending_attachments: prepared,
                })
                .map_err(|e| e.to_string())
        }
        ExecutionAction::AddAttachments {
            step_id,
            input_id,
            files,
        } => {
            let prepared = prepare_attachment_sources(execution_dir, files)?;
            let attachments = prepared.iter().map(attachment_record).collect();
            state
                .add_attachments_event(&step_id, &input_id, attachments)
                .map(|event| BuiltAction {
                    event,
                    pending_attachments: prepared,
                })
                .map_err(|e| e.to_string())
        }
        ExecutionAction::RemoveAttachmentFile {
            step_id,
            input_id,
            path,
        } => state
            .remove_attachment_file_event(&step_id, &input_id, &path)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::ClearAttachments { step_id, input_id } => state
            .clear_attachments_event(&step_id, &input_id)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::RemoveAttachment {
            step_id,
            input_id,
            reason,
        } => state
            .remove_attachment_event(&step_id, &input_id, &reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::Complete { status } => state
            .complete_event(status)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::Abort { reason } => state
            .abort_event(&reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::RenameExecution { name } => state
            .rename_event(&name)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
        ExecutionAction::ReopenExecution { reason } => state
            .reopen_event(&reason)
            .map(BuiltAction::without_attachments)
            .map_err(|e| e.to_string()),
    }?;
    Ok(event)
}

fn prepare_attachment_sources(
    execution_dir: &Path,
    sources: Vec<AttachmentSource>,
) -> Result<Vec<PendingStoredAttachment>, String> {
    if sources.is_empty() {
        return Err("at least one attachment file is required".to_string());
    }

    let store = AttachmentStore::new(execution_dir.to_path_buf());
    sources
        .into_iter()
        .map(|source| store.prepare_copy(Path::new(&source.path), &source.filename))
        .collect()
}

fn prepare_attachment_bytes(
    execution_dir: &Path,
    sources: Vec<AttachmentBytesSource>,
) -> Result<Vec<PendingStoredAttachment>, String> {
    if sources.is_empty() {
        return Err("at least one attachment file is required".to_string());
    }

    let store = AttachmentStore::new(execution_dir.to_path_buf());
    sources
        .into_iter()
        .map(|source| store.prepare_bytes(&source.filename, source.bytes))
        .collect()
}

fn attachment_record(pending: &PendingStoredAttachment) -> AttachmentRecord {
    AttachmentRecord {
        filename: pending.stored.filename.clone(),
        path: pending.stored.relative_path.clone(),
        content_type: attachment_content_type(&pending.stored.filename),
        sha256: pending.stored.sha256.clone(),
    }
}

fn attachment_content_type(filename: &str) -> String {
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

fn commit_pending_attachments(
    execution_dir: &Path,
    pending: Vec<PendingStoredAttachment>,
) -> Result<(), String> {
    let store = AttachmentStore::new(execution_dir.to_path_buf());
    pending
        .into_iter()
        .try_for_each(|attachment| store.commit_prepared(attachment).map(|_| ()))
}

/// Find the execution directory by scanning all procedure subdirectories.
pub fn find_execution_dir(
    procedures_dir: &Path,
    execution_id: ExecutionId,
) -> Result<Option<PathBuf>, String> {
    let mut matches = Vec::new();
    let proc_entries = std::fs::read_dir(procedures_dir).map_err(|e| e.to_string())?;
    for proc_entry in proc_entries.flatten() {
        let exec_base = proc_entry.path().join(".executions");
        if !exec_base.is_dir() {
            continue;
        }
        let entries = match std::fs::read_dir(&exec_base) {
            Ok(entries) => entries,
            Err(e) => {
                log::warn!(
                    "skipping unreadable executions directory {}: {e}",
                    exec_base.display()
                );
                continue;
            }
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_dir() || is_temp_execution_dir(&path) {
                continue;
            }
            let log_path = path.join("events.jsonl");
            if !log_path.exists() {
                continue;
            }
            let events = match EventLog::new(log_path.clone()).read_locked() {
                Ok(events) => events,
                Err(e) => {
                    log::warn!("skipping unreadable event log {}: {e}", log_path.display());
                    continue;
                }
            };
            if events.iter().any(|event| {
                matches!(event, Event::ExecutionStarted { execution_id: id, .. } if *id == execution_id)
            }) {
                matches.push(path);
            }
        }
    }

    match matches.len() {
        0 => Ok(None),
        1 => Ok(matches.pop()),
        _ => Err(format!(
            "multiple execution directories contain execution id {execution_id}"
        )),
    }
}

pub fn is_temp_execution_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') || name.contains(".tmp-"))
}

fn write_execution_snapshot_durable(
    execution_dir: &Path,
    state: &ExecutionState,
) -> Result<(), String> {
    let snapshot_path = execution_dir.join("README.md");
    let temp_path = execution_dir.join(format!(".README.md.tmp-{}", uuid::Uuid::new_v4()));
    let markdown = render_execution_markdown(state);

    let write_result = write_file_durable(&temp_path, markdown.as_bytes())
        .and_then(|()| std::fs::rename(&temp_path, &snapshot_path))
        .and_then(|()| sync_dir(execution_dir));

    if let Err(error) = write_result {
        if let Err(remove_error) = std::fs::remove_file(&temp_path)
            && remove_error.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!(
                "failed to remove temporary execution snapshot {}: {remove_error}",
                temp_path.display()
            );
        }
        return Err(error.to_string());
    }

    Ok(())
}

fn write_file_durable(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(path)?;
    file.write_all(bytes)?;
    file.flush()?;
    file.sync_all()
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

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_core::template::parse_template;

    const TEMPLATE: &str = r"---
id: snapshot-test
title: Snapshot Test
version: 1.0.0
---

## Prepare

- [ ] Ready

## Measure

```inputs
- id: voltage
  label: Voltage
  type: measurement
  unit: V
```
";

    #[test]
    fn create_and_update_execution_snapshot_readme() {
        let temp = tempfile::tempdir().unwrap();
        let procedure_dir = temp.path().join("snapshot-test");
        std::fs::create_dir(&procedure_dir).unwrap();
        let template_path = procedure_dir.join("template.md");
        std::fs::write(&template_path, TEMPLATE).unwrap();

        let template = parse_template(TEMPLATE).unwrap();
        let mut state = ExecutionState::new();
        let events = state.start(&template).unwrap();
        let execution_id = state.execution_id.unwrap();
        let started_at = events
            .iter()
            .find_map(|event| match event {
                Event::ExecutionStarted { at, .. } => Some(*at),
                _ => None,
            })
            .unwrap();

        let store = ExecutionStore::new(temp.path().to_path_buf());
        let recorded = store
            .create_execution(
                &template_path,
                state,
                events,
                started_at,
                execution_id,
                "test".to_string(),
            )
            .unwrap();

        let readme_path = recorded.execution_dir.join("README.md");
        let initial_readme = std::fs::read_to_string(&readme_path).unwrap();
        assert!(initial_readme.contains("# Snapshot Test"));
        assert!(initial_readme.contains("- [ ] Ready"));
        assert!(!initial_readme.contains("Event history"));

        store
            .record_action(
                execution_id,
                ExecutionAction::ToggleCheckbox {
                    step_id: "step-0".to_string(),
                    checkbox_id: "step-0/cb-0".to_string(),
                    checked: true,
                },
            )
            .unwrap();

        let updated_readme = std::fs::read_to_string(readme_path).unwrap();
        assert!(updated_readme.contains("- [x] Ready"));
        assert!(updated_readme.contains("Toggled at:"));
    }
}
