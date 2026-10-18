use std::io::Write;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, Utc};
use procnote_core::event::SUPPORTED_VERSION;
use procnote_core::event::types::{AttachmentRecord, Event, EventMeta, EventRecord, ExecutionId};
use procnote_core::execution::ExecutionState;
use procnote_core::snapshot::render_execution_markdown;
use uuid::Uuid;

use crate::persistence::attachment_store::AttachmentStore;
use crate::persistence::conflict_log;
use crate::persistence::event_log::{EventLog, sync_dir};
use crate::persistence::outbox::{self, Rejected, SyncState};
use crate::repository::{
    Decide, ExecutionRepository, NewExecution, StoredExecution, TemplateSnapshot, apply_decision,
};
use crate::sync::{Conflict, Resolution};

/// Executions stored as directories under each procedure's `.executions/`.
pub struct ExecutionStore {
    procedures_dir: PathBuf,
}

pub struct RecordedExecution {
    pub state: ExecutionState,
    pub execution_dir: PathBuf,
}

pub struct InstalledAttachmentSource {
    pub filename: String,
    pub relative_path: String,
    pub content_type: String,
    pub sha256: String,
}

impl ExecutionRepository for ExecutionStore {
    fn list(&self) -> Result<Vec<StoredExecution>, String> {
        Ok(execution_dirs(&self.procedures_dir)?
            .into_iter()
            .filter_map(|dir| match read_state(&dir) {
                Ok(state) => Some(stored(state, &dir)),
                Err(e) => {
                    log::warn!("skipping unreadable execution {}: {e}", dir.display());
                    None
                }
            })
            .collect())
    }

    fn load(&self, execution_id: ExecutionId) -> Result<StoredExecution, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        Ok(stored(read_state(&execution_dir)?, &execution_dir))
    }

    /// Create an execution directory and publish it atomically after its core
    /// files have been durably written.
    fn create(&self, new: NewExecution) -> Result<StoredExecution, String> {
        let (Some(execution_id), Some(started_at)) = (new.state.execution_id, new.state.started_at)
        else {
            return Err("execution state has not been started".to_string());
        };
        if Path::new(&new.procedure)
            .components()
            .ne([Component::Normal(new.procedure.as_ref())])
        {
            return Err(format!("invalid procedure name: {}", new.procedure));
        }
        let executions_dir = self.procedures_dir.join(&new.procedure).join(".executions");
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

        write_file_durable(
            &temp_dir.join(new.template.file_name()),
            new.template.text().as_bytes(),
        )
        .map_err(|e| e.to_string())?;

        let log_meta = Event::LogMeta {
            at: Utc::now(),
            version: SUPPORTED_VERSION,
            tool_version: new.tool_version,
        };
        let mut events = vec![EventRecord::from(log_meta)];
        events.extend(new.initial_events.into_iter().map(|event| EventRecord {
            event,
            meta: new.meta.clone(),
        }));
        EventLog::new(temp_dir.join("events.jsonl"))
            .create_with_events_durable(&events)
            .map_err(|e| e.to_string())?;
        write_execution_snapshot_durable(&temp_dir, &new.state)?;

        sync_dir(&temp_dir).map_err(|e| e.to_string())?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;

        Ok(stored(new.state, &final_dir))
    }

    /// The full read/replay/validate/append sequence runs under the event
    /// log's exclusive lock; the durable append is the commit point.
    fn record(
        &self,
        execution_id: ExecutionId,
        meta: &EventMeta,
        decide: &mut Decide<'_>,
    ) -> Result<StoredExecution, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let event_log = EventLog::new(execution_dir.join("events.jsonl"));
        let transaction_log = event_log.clone();

        event_log
            .with_exclusive_lock(|| {
                let linked = outbox::read_state(&execution_dir)?;
                let mut records = transaction_log.read_records().map_err(|e| e.to_string())?;
                if let Some(sync) = linked {
                    records.extend(unhandled(&execution_dir, sync)?);
                }
                let events: Vec<Event> = records.iter().map(|r| r.event.clone()).collect();
                let mut state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;

                if is_retry(&records, meta) {
                    return Ok(stored(state, &execution_dir));
                }

                let decision = decide(&state)?;
                apply_decision(&mut state, &decision)?;
                let attachments = AttachmentStore::new(execution_dir.clone());
                decision
                    .files
                    .iter()
                    .try_for_each(|file| attachments.store(file))?;
                if decision.events.is_empty() {
                    return Ok(stored(state, &execution_dir));
                }
                let appended: Vec<EventRecord> = decision
                    .events
                    .into_iter()
                    .map(|event| EventRecord {
                        event,
                        meta: meta.clone(),
                    })
                    .collect();
                // A linked execution's log is the server's; what's recorded
                // here waits in pending.jsonl until the server confirms it.
                if linked.is_some() {
                    outbox::append_pending(&execution_dir, &appended)?;
                } else {
                    transaction_log
                        .append_batch_durable(&appended)
                        .map_err(|e| e.to_string())?;
                }
                if let Err(e) = write_execution_snapshot_durable(&execution_dir, &state) {
                    log::warn!(
                        "failed to update execution snapshot for {execution_id} after appending events: {e}"
                    );
                }
                Ok(stored(state, &execution_dir))
            })
            .map_err(|e| e.to_string())?
    }

    fn read_attachment(&self, execution_id: ExecutionId, path: &str) -> Result<Vec<u8>, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        std::fs::read(resolve_attachment_file_path(&execution_dir, path)?)
            .map_err(|e| e.to_string())
    }

    fn records(&self, execution_id: ExecutionId) -> Result<Vec<EventRecord>, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let event_log = EventLog::new(execution_dir.join("events.jsonl"));
        event_log
            .with_shared_lock(|| event_log.read_records())
            .map_err(|e| e.to_string())?
            .map_err(|e| e.to_string())
    }

    fn conflicts(&self, execution_id: ExecutionId) -> Result<Vec<Conflict>, String> {
        conflict_log::read(&self.existing_dir(execution_id)?)
    }

    fn template_snapshot(&self, execution_id: ExecutionId) -> Result<TemplateSnapshot, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let read = |name: &str| std::fs::read_to_string(execution_dir.join(name));
        match read(TemplateSnapshot::JSON_FILE) {
            Ok(text) => Ok(TemplateSnapshot::Json(text)),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                read(TemplateSnapshot::MARKDOWN_FILE)
                    .map(TemplateSnapshot::Markdown)
                    .map_err(|e| e.to_string())
            }
            Err(e) => Err(e.to_string()),
        }
    }

    fn add_conflict(&self, execution_id: ExecutionId, conflict: &Conflict) -> Result<(), String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let event_id = conflict.record.meta.event_id;
        // Under the log's lock, so a conflict is kept once however it's retried.
        EventLog::new(execution_dir.join("events.jsonl"))
            .with_exclusive_lock(|| {
                let kept = conflict_log::read(&execution_dir)?
                    .iter()
                    .any(|known| event_id.is_some() && known.record.meta.event_id == event_id);
                if kept {
                    Ok(())
                } else {
                    conflict_log::append_conflict(&execution_dir, conflict)
                }
            })
            .map_err(|e| e.to_string())?
    }

    fn resolve_conflict(
        &self,
        execution_id: ExecutionId,
        event_id: Uuid,
        resolution: &Resolution,
    ) -> Result<(), String> {
        let execution_dir = self.existing_dir(execution_id)?;
        EventLog::new(execution_dir.join("events.jsonl"))
            .with_exclusive_lock(|| {
                conflict_log::append_resolution(&execution_dir, event_id, resolution)
            })
            .map_err(|e| e.to_string())?
    }
}

impl ExecutionStore {
    #[must_use]
    pub const fn new(procedures_dir: PathBuf) -> Self {
        Self { procedures_dir }
    }

    /// Durably append the application record for an already-installed atomic
    /// bundle. Replaying an identical committed batch is idempotent, which
    /// allows `DropPoint` close recovery after a process restart.
    pub fn record_installed_attachment_batch(
        &self,
        execution_id: ExecutionId,
        step_id: &str,
        input_id: &str,
        files: Vec<InstalledAttachmentSource>,
    ) -> Result<RecordedExecution, String> {
        let attachments = validate_installed_attachment_sources(files)?;
        let execution_dir = find_execution_dir(&self.procedures_dir, execution_id)?
            .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
        let event_log = EventLog::new(execution_dir.join("events.jsonl"));
        let transaction_log = event_log.clone();

        event_log
            .with_exclusive_lock(|| {
                let events = transaction_log.read().map_err(|error| error.to_string())?;
                let mut state =
                    ExecutionState::from_events(&events).map_err(|error| error.to_string())?;

                let already_recorded = events.iter().any(|event| {
                    matches!(
                        event,
                        Event::AttachmentsAdded {
                            execution_id: event_execution_id,
                            step_id: event_step_id,
                            input_id: event_input_id,
                            attachments: event_attachments,
                            ..
                        } if *event_execution_id == execution_id
                            && event_step_id == step_id
                            && event_input_id == input_id
                            && event_attachments == &attachments
                    )
                });
                if already_recorded {
                    return Ok(RecordedExecution {
                        state,
                        execution_dir: execution_dir.clone(),
                    });
                }

                let event = state
                    .add_attachments_event(step_id, input_id, attachments)
                    .map_err(|error| error.to_string())?;
                state.apply(&event).map_err(|error| error.to_string())?;
                transaction_log
                    .append_durable(&event.clone().into())
                    .map_err(|error| error.to_string())?;
                if let Err(error) = write_execution_snapshot_durable(&execution_dir, &state) {
                    log::warn!(
                        "failed to update execution snapshot for {execution_id} after appending installed attachment event: {error}"
                    );
                }
                Ok(RecordedExecution {
                    state,
                    execution_dir: execution_dir.clone(),
                })
            })
            .map_err(|error| error.to_string())?
    }

    /// The directory of an execution whose event log exists.
    fn existing_dir(&self, execution_id: ExecutionId) -> Result<PathBuf, String> {
        find_execution_dir(&self.procedures_dir, execution_id)?
            .filter(|dir| dir.join("events.jsonl").exists())
            .ok_or_else(|| format!("Execution not found: {execution_id}"))
    }
}

/// What a linked execution has to send, read under the log's lock.
#[derive(Debug)]
pub struct Outbox {
    pub sync: SyncState,
    /// The log as it is here (for registering an execution started here).
    pub records: Vec<EventRecord>,
    /// Pending lines not dealt with yet, in order.
    pub pending: Vec<EventRecord>,
    /// The pending line count once these are dealt with.
    pub pending_upto: usize,
    /// Files the pending events attach, with where their content is.
    pub files: Vec<(AttachmentRecord, PathBuf)>,
}

/// The outcome of one sync, applied by [`ExecutionStore::settle`].
#[derive(Debug, Default)]
pub struct Settlement {
    /// The server now has the execution.
    pub registered: bool,
    /// Pending lines up to here were dealt with by the server.
    pub pending_upto: usize,
    /// Pushed events the server refused.
    pub rejected: Vec<Rejected>,
    /// Log lines the server has after the ones here, in its order.
    pub pulled: Vec<EventRecord>,
    /// How many log lines the server had before `pulled`.
    pub pulled_after: usize,
}

impl ExecutionStore {
    fn log_of(execution_dir: &Path) -> EventLog {
        EventLog::new(execution_dir.join("events.jsonl"))
    }

    /// Link an execution to the server: from now on what's recorded here
    /// waits in `pending.jsonl` until the server confirms it. Linking a
    /// linked execution changes nothing.
    pub fn link(&self, execution_id: ExecutionId, registered: bool) -> Result<(), String> {
        let execution_dir = self.existing_dir(execution_id)?;
        Self::log_of(&execution_dir)
            .with_exclusive_lock(|| {
                if outbox::read_state(&execution_dir)?.is_some() {
                    return Ok(());
                }
                outbox::write_state(
                    &execution_dir,
                    SyncState {
                        registered,
                        handled: 0,
                    },
                )
            })
            .map_err(|e| e.to_string())?
    }

    /// The sync state, or `None` when the execution isn't linked.
    pub fn sync_state(&self, execution_id: ExecutionId) -> Result<Option<SyncState>, String> {
        outbox::read_state(&self.existing_dir(execution_id)?)
    }

    /// Linked executions here, with their sync state.
    pub fn linked(&self) -> Result<Vec<(ExecutionId, SyncState)>, String> {
        let mut linked = Vec::new();
        for dir in execution_dirs(&self.procedures_dir)? {
            if let Some(sync) = outbox::read_state(&dir)?
                && let Some(id) = read_state(&dir)?.execution_id
            {
                linked.push((id, sync));
            }
        }
        Ok(linked)
    }

    /// What a linked execution has to send.
    pub fn outbox(&self, execution_id: ExecutionId) -> Result<Outbox, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let event_log = Self::log_of(&execution_dir);
        event_log
            .with_shared_lock(|| {
                let sync = outbox::read_state(&execution_dir)?
                    .ok_or_else(|| format!("execution {execution_id} isn't linked to a server"))?;
                let all = outbox::read_pending(&execution_dir)?;
                let pending_upto = all.len();
                let pending: Vec<EventRecord> = all.into_iter().skip(sync.handled).collect();
                let files = pending
                    .iter()
                    .flat_map(|record| crate::sync::referenced_files(&record.event))
                    .map(|file| {
                        let path = execution_dir.join(&file.path);
                        (file, path)
                    })
                    .collect();
                Ok(Outbox {
                    sync,
                    records: event_log.read_records().map_err(|e| e.to_string())?,
                    pending,
                    pending_upto,
                    files,
                })
            })
            .map_err(|e| e.to_string())?
    }

    /// Apply a sync's outcome: append the pulled lines to the log, keep the
    /// refused events, then mark the pushed pending lines dealt with — in
    /// that order, so a crash midway only causes a harmless re-push.
    pub fn settle(
        &self,
        execution_id: ExecutionId,
        settlement: &Settlement,
    ) -> Result<ExecutionState, String> {
        let execution_dir = self.existing_dir(execution_id)?;
        let event_log = Self::log_of(&execution_dir);
        event_log
            .with_exclusive_lock(|| {
                let sync = outbox::read_state(&execution_dir)?
                    .ok_or_else(|| format!("execution {execution_id} isn't linked to a server"))?;
                let records = event_log.read_records().map_err(|e| e.to_string())?;
                if !settlement.pulled.is_empty() {
                    if settlement.pulled_after != records.len() {
                        return Err(format!(
                            "pulled log lines start at {}, but this copy has {}",
                            settlement.pulled_after,
                            records.len()
                        ));
                    }
                    let events: Vec<Event> = records
                        .iter()
                        .chain(&settlement.pulled)
                        .map(|record| record.event.clone())
                        .collect();
                    ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
                    event_log
                        .append_batch_durable(&settlement.pulled)
                        .map_err(|e| e.to_string())?;
                }
                outbox::append_rejected(&execution_dir, &settlement.rejected)?;
                outbox::write_state(
                    &execution_dir,
                    SyncState {
                        registered: sync.registered || settlement.registered,
                        handled: sync.handled.max(settlement.pending_upto),
                    },
                )?;
                Ok(())
            })
            .map_err(|e| e.to_string())??;
        let state = read_state(&execution_dir)?;
        if let Err(e) = write_execution_snapshot_durable(&execution_dir, &state) {
            log::warn!("failed to update execution snapshot for {execution_id} after syncing: {e}");
        }
        Ok(state)
    }

    /// Keep a copy of an execution the server has: its log as the server has
    /// it, linked and registered. Fails if a copy is already here.
    pub fn import_remote(
        &self,
        procedure: &str,
        template: &TemplateSnapshot,
        records: &[EventRecord],
    ) -> Result<StoredExecution, String> {
        let events: Vec<Event> = records.iter().map(|record| record.event.clone()).collect();
        let state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
        let (Some(execution_id), Some(started_at)) = (state.execution_id, state.started_at) else {
            return Err("the log doesn't start an execution".to_string());
        };
        if find_execution_dir(&self.procedures_dir, execution_id)?.is_some() {
            return Err(format!("execution {execution_id} is already here"));
        }
        if Path::new(procedure)
            .components()
            .ne([Component::Normal(procedure.as_ref())])
        {
            return Err(format!("invalid procedure name: {procedure}"));
        }
        let executions_dir = self.procedures_dir.join(procedure).join(".executions");
        std::fs::create_dir_all(&executions_dir).map_err(|e| e.to_string())?;
        let final_dir = executions_dir.join(execution_dir_name(&started_at, execution_id));
        let temp_dir = executions_dir.join(format!(".import.tmp-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir(&temp_dir).map_err(|e| e.to_string())?;
        write_file_durable(
            &temp_dir.join(template.file_name()),
            template.text().as_bytes(),
        )
        .map_err(|e| e.to_string())?;
        Self::log_of(&temp_dir)
            .create_with_events_durable(records)
            .map_err(|e| e.to_string())?;
        outbox::write_state(
            &temp_dir,
            SyncState {
                registered: true,
                handled: 0,
            },
        )?;
        write_execution_snapshot_durable(&temp_dir, &state)?;
        sync_dir(&temp_dir).map_err(|e| e.to_string())?;
        std::fs::rename(&temp_dir, &final_dir).map_err(|e| e.to_string())?;
        sync_dir(&executions_dir).map_err(|e| e.to_string())?;
        Ok(stored(state, &final_dir))
    }

    /// Store the content of an attachment someone else recorded, checked
    /// against its record. Content already here is left as it is.
    pub fn store_attachment(
        &self,
        execution_id: ExecutionId,
        record: &AttachmentRecord,
        content: &[u8],
    ) -> Result<(), String> {
        if crate::attachment::sha256_hex(content) != record.sha256 {
            return Err(format!(
                "downloaded content of {} doesn't match its hash",
                record.path
            ));
        }
        let execution_dir = self.existing_dir(execution_id)?;
        let relative = Path::new(&record.path);
        if !record.path.starts_with("attachments/")
            || relative
                .components()
                .any(|component| !matches!(component, Component::Normal(_)))
        {
            return Err(format!("invalid attachment path: {}", record.path));
        }
        let path = execution_dir.join(relative);
        if path.exists() {
            return Ok(());
        }
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        write_file_durable(&path, content).map_err(|e| e.to_string())
    }

    /// Pushed events the server refused as malformed.
    pub fn rejected(&self, execution_id: ExecutionId) -> Result<Vec<Rejected>, String> {
        outbox::read_rejected(&self.existing_dir(execution_id)?)
    }
}

/// Every published execution directory under `procedures_dir` that has an
/// event log.
pub fn execution_dirs(procedures_dir: &Path) -> Result<Vec<PathBuf>, String> {
    if !procedures_dir.exists() {
        return Ok(Vec::new());
    }
    let mut dirs = Vec::new();
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
        dirs.extend(entries.flatten().map(|entry| entry.path()).filter(|path| {
            path.is_dir() && !is_temp_execution_dir(path) && path.join("events.jsonl").exists()
        }));
    }
    Ok(dirs)
}

fn stored(state: ExecutionState, execution_dir: &Path) -> StoredExecution {
    StoredExecution {
        state,
        location: Some(execution_dir.display().to_string()),
    }
}

fn read_state(execution_dir: &Path) -> Result<ExecutionState, String> {
    let event_log = EventLog::new(execution_dir.join("events.jsonl"));
    let events = event_log
        .with_shared_lock(|| -> Result<Vec<Event>, String> {
            let mut events = event_log.read().map_err(|e| e.to_string())?;
            if let Some(sync) = outbox::read_state(execution_dir)? {
                events.extend(unhandled(execution_dir, sync)?.into_iter().map(|r| r.event));
            }
            Ok(events)
        })
        .map_err(|e| e.to_string())??;
    ExecutionState::from_events(&events).map_err(|e| e.to_string())
}

/// The pending lines of a linked execution the server hasn't dealt with yet.
fn unhandled(execution_dir: &Path, sync: SyncState) -> Result<Vec<EventRecord>, String> {
    Ok(outbox::read_pending(execution_dir)?
        .into_iter()
        .skip(sync.handled)
        .collect())
}

/// Whether `meta` carries an event ID that is already recorded.
fn is_retry(records: &[EventRecord], meta: &EventMeta) -> bool {
    meta.event_id.is_some_and(|id| {
        records
            .iter()
            .any(|record| record.meta.event_id == Some(id))
    })
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

fn validate_installed_attachment_sources(
    sources: Vec<InstalledAttachmentSource>,
) -> Result<Vec<AttachmentRecord>, String> {
    if sources.is_empty() {
        return Err("at least one installed attachment file is required".to_string());
    }
    sources
        .into_iter()
        .map(|source| {
            let relative = Path::new(&source.relative_path);
            if relative.is_absolute()
                || !source.relative_path.starts_with("attachments/")
                || relative
                    .components()
                    .any(|component| !matches!(component, std::path::Component::Normal(_)))
                || source.filename.is_empty()
                || source.content_type.is_empty()
                || source.sha256.len() != 64
                || !source
                    .sha256
                    .bytes()
                    .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
            {
                return Err("installed attachment metadata is invalid".to_string());
            }
            Ok(AttachmentRecord {
                filename: source.filename,
                path: source.relative_path,
                content_type: source.content_type,
                sha256: source.sha256,
            })
        })
        .collect()
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

#[must_use]
pub fn is_temp_execution_dir(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with('.') || name.contains(".tmp-"))
}

/// Write the human-readable `README.md` snapshot of an execution.
pub fn write_execution_snapshot_durable(
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

/// Format the execution directory name as `{YYYYMMDD}T{HHMMSS}-{uuid_8}`.
#[must_use]
pub fn execution_dir_name(at: &DateTime<Utc>, execution_id: ExecutionId) -> String {
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
    use crate::action::ExecutionAction;
    use crate::repository::{TemplateSnapshot, receive_files, record_action};
    use procnote_core::template::parse_template;

    /// Start an execution of `source` in `procedure` (created under `root`).
    fn create(
        root: &Path,
        procedure: &str,
        source: &str,
        meta: &EventMeta,
    ) -> (ExecutionStore, ExecutionId, PathBuf) {
        let procedure_dir = root.join(procedure);
        std::fs::create_dir(&procedure_dir).unwrap();
        std::fs::write(procedure_dir.join("template.md"), source).unwrap();
        let mut state = ExecutionState::new();
        let initial_events = state.start(&parse_template(source).unwrap()).unwrap();
        let execution_id = state.execution_id.unwrap();
        let store = ExecutionStore::new(root.to_path_buf());
        let created = store
            .create(NewExecution {
                procedure: procedure.to_string(),
                template: TemplateSnapshot::Markdown(source.to_string()),
                state,
                initial_events,
                tool_version: "test".to_string(),
                meta: meta.clone(),
            })
            .unwrap();
        (store, execution_id, dir_of(&created))
    }

    fn dir_of(stored: &StoredExecution) -> PathBuf {
        PathBuf::from(stored.location.as_deref().unwrap())
    }

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

    const ATTACHMENT_TEMPLATE: &str = r"---
id: attachment-test
title: Attachment Test
version: 1.0.0
---

## Capture

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";

    #[test]
    fn create_and_update_execution_snapshot_readme() {
        let temp = tempfile::tempdir().unwrap();
        let (store, execution_id, execution_dir) = create(
            temp.path(),
            "snapshot-test",
            TEMPLATE,
            &EventMeta::default(),
        );

        let readme_path = execution_dir.join("README.md");
        let initial_readme = std::fs::read_to_string(&readme_path).unwrap();
        assert!(initial_readme.contains("# Snapshot Test"));
        assert!(initial_readme.contains("- [ ] Ready"));
        assert!(!initial_readme.contains("Event history"));

        record_action(&store, execution_id, toggle(), &EventMeta::default()).unwrap();

        let updated_readme = std::fs::read_to_string(readme_path).unwrap();
        assert!(updated_readme.contains("- [x] Ready"));
        assert!(updated_readme.contains("Toggled at:"));
    }

    fn toggle() -> ExecutionAction {
        ExecutionAction::ToggleCheckbox {
            step_id: "step-0".to_string(),
            checkbox_id: "step-0/cb-0".to_string(),
            checked: true,
        }
    }

    #[test]
    fn records_who_and_where_with_each_event() {
        let temp = tempfile::tempdir().unwrap();
        let starter = EventMeta {
            event_id: None,
            recorded_by: Some("starter".to_string()),
            device: Some("desk".to_string()),
        };
        let (store, execution_id, execution_dir) =
            create(temp.path(), "snapshot-test", TEMPLATE, &starter);
        let toggler = EventMeta {
            event_id: Some(uuid::Uuid::new_v4()),
            recorded_by: Some("toggler".to_string()),
            device: Some("phone".to_string()),
        };

        record_action(&store, execution_id, toggle(), &toggler).unwrap();

        let records = EventLog::new(execution_dir.join("events.jsonl"))
            .read_records()
            .unwrap();
        assert_eq!(
            records[0].meta,
            EventMeta::default(),
            "LogMeta has no author"
        );
        assert_eq!(records[1].meta, starter);
        assert_eq!(records.last().unwrap().meta, toggler);
    }

    #[test]
    fn a_retried_event_id_is_recorded_once() {
        let temp = tempfile::tempdir().unwrap();
        let (store, execution_id, execution_dir) = create(
            temp.path(),
            "snapshot-test",
            TEMPLATE,
            &EventMeta::default(),
        );
        let meta = EventMeta {
            event_id: Some(uuid::Uuid::new_v4()),
            ..EventMeta::default()
        };

        record_action(&store, execution_id, toggle(), &meta).unwrap();
        let retried = record_action(&store, execution_id, toggle(), &meta).unwrap();

        assert!(
            retried.state.steps["step-0"]
                .content
                .iter()
                .any(|c| matches!(
                    c,
                    procnote_core::execution::ExecutionStepContent::Checkbox(cb) if cb.checked
                ))
        );
        let toggles = EventLog::new(execution_dir.join("events.jsonl"))
            .read()
            .unwrap()
            .into_iter()
            .filter(|event| matches!(event, Event::CheckboxToggled { .. }))
            .count();
        assert_eq!(toggles, 1);
    }

    #[test]
    fn received_files_go_to_the_inbox_once_per_content_and_can_be_assigned() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, execution_id, execution_dir) = create(
            temporary.path(),
            "attachment-test",
            ATTACHMENT_TEMPLATE,
            &EventMeta::default(),
        );
        let source = |name: &str, content: &str| {
            let path = temporary.path().join(name);
            std::fs::write(&path, content).unwrap();
            (path, name.to_string())
        };
        let phone = EventMeta {
            device: Some("upload-link".to_string()),
            ..EventMeta::default()
        };

        let (recorded, received) = receive_files(
            &store,
            execution_id,
            &[
                source("a.txt", "same"),
                source("b.txt", "same"),
                source("c.txt", "other"),
            ],
            &phone,
        )
        .unwrap();
        let (_, again) =
            receive_files(&store, execution_id, &[source("a.txt", "same")], &phone).unwrap();

        assert_eq!((received, again), (2, 0));
        let inbox: Vec<&str> = recorded
            .state
            .inbox
            .iter()
            .map(|f| f.filename.as_str())
            .collect();
        assert_eq!(inbox, ["a.txt", "c.txt"]);
        assert!(execution_dir.join(&recorded.state.inbox[0].path).is_file());

        let assigned = record_action(
            &store,
            execution_id,
            ExecutionAction::AssignAttachment {
                step_id: "step-0".to_string(),
                input_id: "evidence".to_string(),
                path: recorded.state.inbox[0].path.clone(),
            },
            &EventMeta::default(),
        )
        .unwrap();
        assert_eq!(assigned.state.inbox.len(), 1);
        assert_eq!(
            assigned.state.steps["step-0"].attachments["evidence"].len(),
            1
        );
    }

    #[test]
    fn installed_attachment_record_is_idempotent_after_restart_boundary() {
        let temporary = tempfile::tempdir().unwrap();
        let (store, execution_id, execution_dir) = create(
            temporary.path(),
            "attachment-test",
            ATTACHMENT_TEMPLATE,
            &EventMeta::default(),
        );

        let source = || InstalledAttachmentSource {
            filename: "scan.txt".to_string(),
            relative_path: "attachments/bundle-dp_example/scan.txt".to_string(),
            content_type: "text/plain".to_string(),
            sha256: "a".repeat(64),
        };
        store
            .record_installed_attachment_batch(execution_id, "step-0", "evidence", vec![source()])
            .unwrap();
        let replayed = store
            .record_installed_attachment_batch(execution_id, "step-0", "evidence", vec![source()])
            .unwrap();

        assert_eq!(
            replayed.state.steps["step-0"].attachments["evidence"].len(),
            1
        );
        let events = EventLog::new(execution_dir.join("events.jsonl"))
            .read()
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::AttachmentsAdded { .. }))
                .count(),
            1
        );
    }

    #[test]
    fn pushed_events_are_recorded_once_or_kept_as_conflicts() {
        use crate::sync::{PushOutcome, create_started, push_event, resolve_conflict};

        let temp = tempfile::tempdir().unwrap();
        std::fs::create_dir(temp.path().join("snapshot-test")).unwrap();
        let store = ExecutionStore::new(temp.path().to_path_buf());
        // A device starts the execution itself and pushes what it recorded.
        let mut device = ExecutionState::new();
        let started = device.start(&parse_template(TEMPLATE).unwrap()).unwrap();
        let id = device.execution_id.unwrap();
        let author = EventMeta {
            event_id: None,
            recorded_by: Some("offline operator".to_string()),
            device: Some("tablet".to_string()),
        };
        let records: Vec<EventRecord> = started
            .into_iter()
            .map(|event| EventRecord {
                event,
                meta: author.clone(),
            })
            .collect();
        create_started(
            &store,
            "snapshot-test".to_string(),
            TemplateSnapshot::Markdown(TEMPLATE.to_string()),
            &records,
            "test".to_string(),
        )
        .unwrap();
        let push = |event: Event| EventRecord {
            event,
            meta: EventMeta {
                event_id: Some(uuid::Uuid::new_v4()),
                ..author.clone()
            },
        };
        let toggle = push(
            device
                .toggle_checkbox_event("step-0", "step-0/cb-0", true)
                .unwrap(),
        );
        let untoggle = push(
            device
                .toggle_checkbox_event("step-0", "step-0/cb-0", false)
                .unwrap(),
        );
        let now = Utc::now();

        let first = push_event(&store, id, &toggle, &[], now).unwrap();
        let retried = push_event(&store, id, &toggle, &[], now).unwrap();
        // Meanwhile someone finishes the execution on the server.
        record_action(
            &store,
            id,
            ExecutionAction::Complete {
                status: procnote_core::event::types::CompletionStatus::Pass,
            },
            &EventMeta::default(),
        )
        .unwrap();
        let late = push_event(&store, id, &untoggle, &[], now).unwrap();
        let late_again = push_event(&store, id, &untoggle, &[], now).unwrap();

        assert_eq!(first, PushOutcome::Recorded);
        assert_eq!(retried, PushOutcome::Duplicate);
        assert!(matches!(late, PushOutcome::Conflict { .. }), "{late:?}");
        assert_eq!(late_again, PushOutcome::Duplicate);
        let records = store.records(id).unwrap();
        assert!(matches!(records[0].event, Event::LogMeta { .. }));
        let toggled = records
            .iter()
            .find(|record| record.meta.event_id == toggle.meta.event_id)
            .unwrap();
        assert_eq!(toggled.event, toggle.event, "kept with the device's time");
        assert_eq!(
            toggled.meta.recorded_by.as_deref(),
            Some("offline operator")
        );
        let conflicts = store.conflicts(id).unwrap();
        assert_eq!(conflicts.len(), 1);
        assert_eq!(conflicts[0].record, untoggle);

        let resolution = crate::sync::Resolution {
            at: Utc::now(),
            by: Some("reviewer".to_string()),
        };
        resolve_conflict(&store, id, untoggle.meta.event_id.unwrap(), &resolution).unwrap();
        assert!(resolve_conflict(&store, id, uuid::Uuid::new_v4(), &resolution).is_err());
        assert_eq!(store.conflicts(id).unwrap()[0].resolved, Some(resolution));
    }

    /// A desktop copy and a server (another store), synced with the same
    /// steps the desktop app takes: push pending lines, pull the rest, settle.
    fn sync_once(desktop: &ExecutionStore, server: &ExecutionStore, id: ExecutionId) {
        let outbox = desktop.outbox(id).unwrap();
        // Registering again is a retry, as the server's PUT treats it.
        if !outbox.sync.registered && server.records(id).is_err() {
            let starting: Vec<EventRecord> = outbox
                .records
                .iter()
                .filter(|record| !matches!(record.event, Event::LogMeta { .. }))
                .cloned()
                .collect();
            crate::sync::create_started(
                server,
                "snapshot-test".to_string(),
                TemplateSnapshot::Markdown(TEMPLATE.to_string()),
                &starting,
                "server".to_string(),
            )
            .unwrap();
        }
        for record in &outbox.pending {
            crate::sync::push_event(server, id, record, &[], Utc::now()).unwrap();
        }
        let server_records = server.records(id).unwrap();
        let after = outbox.records.len();
        desktop
            .settle(
                id,
                &Settlement {
                    registered: true,
                    pending_upto: outbox.pending_upto,
                    rejected: Vec::new(),
                    pulled: server_records[after..].to_vec(),
                    pulled_after: after,
                },
            )
            .unwrap();
    }

    #[test]
    fn a_linked_execution_waits_in_pending_until_the_server_confirms_it() {
        let desktop_dir = tempfile::tempdir().unwrap();
        let server_dir = tempfile::tempdir().unwrap();
        std::fs::create_dir(server_dir.path().join("snapshot-test")).unwrap();
        let server = ExecutionStore::new(server_dir.path().to_path_buf());
        let (desktop, id, execution_dir) = create(
            desktop_dir.path(),
            "snapshot-test",
            TEMPLATE,
            &EventMeta::default(),
        );
        desktop.link(id, false).unwrap();
        let log_before = std::fs::read(execution_dir.join("events.jsonl")).unwrap();
        let meta = EventMeta {
            event_id: Some(uuid::Uuid::new_v4()),
            ..EventMeta::default()
        };

        let toggled = record_action(&desktop, id, toggle(), &meta).unwrap();
        record_action(&desktop, id, toggle(), &meta).unwrap();

        assert_eq!(
            std::fs::read(execution_dir.join("events.jsonl")).unwrap(),
            log_before,
            "the log waits for the server"
        );
        assert_eq!(
            desktop.outbox(id).unwrap().pending.len(),
            1,
            "the retry was deduplicated"
        );
        assert!(matches!(
            &toggled.state.steps["step-0"].content[0],
            procnote_core::execution::ExecutionStepContent::Checkbox(cb) if cb.checked
        ));

        // Not settled yet (as after a crash): the same lines are pushed again.
        let first = desktop.outbox(id).unwrap();
        crate::sync::create_started(
            &server,
            "snapshot-test".to_string(),
            TemplateSnapshot::Markdown(TEMPLATE.to_string()),
            &first
                .records
                .iter()
                .filter(|r| !matches!(r.event, Event::LogMeta { .. }))
                .cloned()
                .collect::<Vec<_>>(),
            "server".to_string(),
        )
        .unwrap();
        crate::sync::push_event(&server, id, &first.pending[0], &[], Utc::now()).unwrap();
        // Someone else records on the server.
        record_action(
            &server,
            id,
            ExecutionAction::RenameExecution {
                name: "from the browser".to_string(),
            },
            &EventMeta::default(),
        )
        .unwrap();
        sync_once(&desktop, &server, id);

        let desktop_log = EventLog::new(execution_dir.join("events.jsonl"))
            .read_records()
            .unwrap();
        let server_log = server.records(id).unwrap();
        assert_eq!(desktop_log.len(), server_log.len());
        assert_eq!(desktop_log[1..], server_log[1..], "same log after LogMeta");
        assert_eq!(desktop.outbox(id).unwrap().pending, []);
        assert_eq!(
            desktop.load(id).unwrap().state.name.as_deref(),
            Some("from the browser")
        );
    }

    #[test]
    fn pulled_lines_must_continue_the_log_here() {
        let temp = tempfile::tempdir().unwrap();
        let (store, id, _) = create(
            temp.path(),
            "snapshot-test",
            TEMPLATE,
            &EventMeta::default(),
        );
        store.link(id, true).unwrap();
        let records = store.outbox(id).unwrap().records;

        let wrong = store.settle(
            id,
            &Settlement {
                pulled: records[1..2].to_vec(),
                pulled_after: 1,
                ..Settlement::default()
            },
        );

        assert!(wrong.is_err());
    }

    #[test]
    fn a_server_execution_is_kept_here_with_its_attachments_checked() {
        let server_dir = tempfile::tempdir().unwrap();
        let (server, id, _) = create(
            server_dir.path(),
            "attachment-test",
            ATTACHMENT_TEMPLATE,
            &EventMeta::default(),
        );
        let desktop_dir = tempfile::tempdir().unwrap();
        let desktop = ExecutionStore::new(desktop_dir.path().to_path_buf());

        let kept = desktop
            .import_remote(
                "attachment-test",
                &TemplateSnapshot::Markdown(ATTACHMENT_TEMPLATE.to_string()),
                &server.records(id).unwrap(),
            )
            .unwrap();
        let again = desktop.import_remote(
            "attachment-test",
            &TemplateSnapshot::Markdown(ATTACHMENT_TEMPLATE.to_string()),
            &server.records(id).unwrap(),
        );
        let content = b"photo";
        let record = AttachmentRecord {
            filename: "p.jpg".to_string(),
            path: format!(
                "attachments/{}-p.jpg",
                &crate::attachment::sha256_hex(content)[..7]
            ),
            content_type: "image/jpeg".to_string(),
            sha256: crate::attachment::sha256_hex(content),
        };

        assert_eq!(kept.state.execution_id, Some(id));
        assert_eq!(
            desktop.sync_state(id).unwrap(),
            Some(SyncState {
                registered: true,
                handled: 0
            })
        );
        assert!(again.is_err());
        assert!(desktop.store_attachment(id, &record, b"tampered").is_err());
        desktop.store_attachment(id, &record, content).unwrap();
        assert_eq!(desktop.read_attachment(id, &record.path).unwrap(), content);
        let escape = AttachmentRecord {
            path: "attachments/../../escape".to_string(),
            ..record
        };
        assert!(desktop.store_attachment(id, &escape, content).is_err());
    }
}
