//! Events recorded on a device, possibly while offline, reaching the server.
//!
//! A device turns actions into events itself (with the WebAssembly build of
//! this crate) and pushes them later. The server records each event that
//! still applies to the execution as it is now. One that doesn't — because
//! someone else changed the execution meanwhile — was still something that
//! happened, so it is kept as a [`Conflict`] for a person to look at, never
//! dropped.

use chrono::{DateTime, Utc};
use procnote_core::event::types::{AttachmentRecord, Event, EventMeta, EventRecord, ExecutionId};
use procnote_core::execution::ExecutionState;
use serde::{Deserialize, Serialize};
use ts_rs::TS;
use uuid::Uuid;

use crate::repository::{
    Decision, ExecutionRepository, NewExecution, StagedFile, StoredExecution, TemplateSnapshot,
    has_content,
};

/// A pushed event that no longer applied to the execution when it arrived.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct Conflict {
    pub record: EventRecord,
    /// Why the execution refused it.
    pub reason: String,
    /// ISO 8601 timestamp of when it reached the server.
    #[ts(type = "string")]
    pub received_at: DateTime<Utc>,
    /// Set once someone has dealt with it (re-recorded it or decided to drop it).
    #[ts(optional)]
    pub resolved: Option<Resolution>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Resolution {
    /// ISO 8601 timestamp.
    #[ts(type = "string")]
    pub at: DateTime<Utc>,
    #[ts(optional)]
    pub by: Option<String>,
}

/// What happened to one pushed event.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, TS)]
#[ts(export)]
#[serde(tag = "outcome", rename_all = "snake_case")]
pub enum PushOutcome {
    /// Appended to the execution's log.
    Recorded,
    /// Already recorded (or already kept as a conflict): a retry.
    Duplicate,
    /// Kept as a conflict for a person to look at.
    Conflict { reason: String },
    /// Not accepted at all (malformed, or attachment content missing); the
    /// device should keep it and fix what's wrong.
    Rejected { reason: String },
}

/// What to do with a pushed event, given the execution as it is now.
#[derive(Debug)]
pub enum Pushed {
    Record(Decision),
    Conflict(String),
}

/// The attachment files an event references.
#[must_use]
pub fn referenced_files(event: &Event) -> Vec<AttachmentRecord> {
    match event {
        Event::AttachmentsAdded { attachments, .. } => attachments.clone(),
        Event::AttachmentReceived { attachment, .. } => vec![attachment.clone()],
        Event::AttachmentAdded {
            filename,
            path,
            content_type,
            sha256,
            ..
        } => vec![AttachmentRecord {
            filename: filename.clone(),
            path: path.clone(),
            content_type: content_type.clone(),
            sha256: sha256.clone(),
        }],
        _ => Vec::new(),
    }
}

const fn event_execution_id(event: &Event) -> Option<ExecutionId> {
    match event {
        Event::LogMeta { .. } => None,
        Event::ExecutionStarted { execution_id, .. }
        | Event::ExecutionCompleted { execution_id, .. }
        | Event::ExecutionAborted { execution_id, .. }
        | Event::ExecutionReopened { execution_id, .. }
        | Event::StepAdded { execution_id, .. }
        | Event::StepSkipped { execution_id, .. }
        | Event::StepUnskipped { execution_id, .. }
        | Event::CheckboxToggled { execution_id, .. }
        | Event::InputRecorded { execution_id, .. }
        | Event::InputCleared { execution_id, .. }
        | Event::NoteAdded { execution_id, .. }
        | Event::NoteRemoved { execution_id, .. }
        | Event::AttachmentAdded { execution_id, .. }
        | Event::AttachmentsAdded { execution_id, .. }
        | Event::AttachmentFileRemoved { execution_id, .. }
        | Event::AttachmentsCleared { execution_id, .. }
        | Event::AttachmentRemoved { execution_id, .. }
        | Event::AttachmentReceived { execution_id, .. }
        | Event::AttachmentAssigned { execution_id, .. }
        | Event::AttachmentUnassigned { execution_id, .. }
        | Event::ExecutionRenamed { execution_id, .. } => Some(*execution_id),
    }
}

/// Decide what to do with `record`, pushed to execution `execution_id` in
/// `state`. `uploaded` are files uploaded with the push; every file the
/// event references must be among them or already in the execution.
///
/// `Err` means the event is malformed or incomplete and is rejected; a
/// [`Pushed::Conflict`] means it is valid but no longer applies.
pub fn decide_pushed(
    execution_id: ExecutionId,
    state: &ExecutionState,
    record: &EventRecord,
    uploaded: &[StagedFile],
) -> Result<Pushed, String> {
    if record.meta.event_id.is_none() {
        return Err("pushed events need an event_id".to_string());
    }
    match event_execution_id(&record.event) {
        None => return Err("log metadata can't be pushed".to_string()),
        Some(id) if id != execution_id => {
            return Err(format!("the event belongs to execution {id}"));
        }
        Some(_) => {}
    }
    if matches!(record.event, Event::ExecutionStarted { .. }) {
        return Err("executions are started by creating them, not by pushing".to_string());
    }

    let mut files = Vec::new();
    for file in referenced_files(&record.event) {
        let upload = uploaded
            .iter()
            .find(|staged| staged.record.sha256 == file.sha256);
        match upload {
            Some(staged) if staged.record != file => {
                return Err(format!(
                    "uploaded file {} doesn't match the attachment record {}",
                    staged.record.path, file.path
                ));
            }
            Some(staged) => files.push(staged.clone()),
            None if has_content(state, &file.sha256) => {}
            None => return Err(format!("the content of {} wasn't uploaded", file.path)),
        }
    }

    let mut next = state.clone();
    Ok(match next.apply(&record.event) {
        Ok(()) => Pushed::Record(Decision {
            events: vec![record.event.clone()],
            files,
        }),
        Err(e) => Pushed::Conflict(e.to_string()),
    })
}

/// Push one event recorded on a device. Retries (by `event_id`) are
/// [`PushOutcome::Duplicate`]; events that no longer apply are kept as conflicts.
pub fn push_event(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    record: &EventRecord,
    uploaded: &[StagedFile],
    now: DateTime<Utc>,
) -> Result<PushOutcome, String> {
    let Some(event_id) = record.meta.event_id else {
        return Ok(PushOutcome::Rejected {
            reason: "pushed events need an event_id".to_string(),
        });
    };
    if repository
        .conflicts(execution_id)?
        .iter()
        .any(|conflict| conflict.record.meta.event_id == Some(event_id))
    {
        return Ok(PushOutcome::Duplicate);
    }

    let mut outcome = PushOutcome::Duplicate;
    repository.record(
        execution_id,
        &record.meta,
        &mut |state| match decide_pushed(execution_id, state, record, uploaded) {
            Ok(Pushed::Record(decision)) => {
                outcome = PushOutcome::Recorded;
                Ok(decision)
            }
            Ok(Pushed::Conflict(reason)) => {
                outcome = PushOutcome::Conflict { reason };
                Ok(Decision::default())
            }
            Err(reason) => {
                outcome = PushOutcome::Rejected { reason };
                Ok(Decision::default())
            }
        },
    )?;
    if let PushOutcome::Conflict { reason } = &outcome {
        repository.add_conflict(
            execution_id,
            &Conflict {
                record: record.clone(),
                reason: reason.clone(),
                received_at: now,
                resolved: None,
            },
        )?;
    }
    Ok(outcome)
}

/// Store an execution a device started (possibly offline) from its first
/// log lines. The lines must start it, as `ExecutionState::start` does.
pub fn create_started(
    repository: &dyn ExecutionRepository,
    procedure: String,
    template: TemplateSnapshot,
    records: &[EventRecord],
    tool_version: String,
) -> Result<StoredExecution, String> {
    let events: Vec<Event> = records.iter().map(|record| record.event.clone()).collect();
    if !matches!(events.first(), Some(Event::ExecutionStarted { .. })) {
        return Err("an execution's log must begin with execution_started".to_string());
    }
    if events.iter().skip(1).any(|event| {
        matches!(
            event,
            Event::ExecutionStarted { .. } | Event::LogMeta { .. }
        )
    }) {
        return Err("an execution can only be started once".to_string());
    }
    let state = ExecutionState::from_events(&events).map_err(|e| e.to_string())?;
    // The starting events share the starter's metadata, without event IDs:
    // creating the execution is what deduplicates them.
    let meta = records
        .first()
        .map(|record| EventMeta {
            event_id: None,
            ..record.meta.clone()
        })
        .unwrap_or_default();
    repository.create(NewExecution {
        procedure,
        template,
        state,
        initial_events: events,
        tool_version,
        meta,
    })
}

/// Mark the conflict of event `event_id` as dealt with.
pub fn resolve_conflict(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    event_id: Uuid,
    resolution: &Resolution,
) -> Result<(), String> {
    let known = repository
        .conflicts(execution_id)?
        .iter()
        .any(|conflict| conflict.record.meta.event_id == Some(event_id));
    if !known {
        return Err(format!("no conflict for event {event_id}"));
    }
    repository.resolve_conflict(execution_id, event_id, resolution)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_core::template::parse_template;

    const TEMPLATE: &str = "---\nid: S\ntitle: Sync\nversion: '1'\n---\n\n## Step\n\n- [ ] Ready\n\n```inputs\n- id: photo\n  label: Photo\n  type: attachment\n```\n";

    fn started() -> (ExecutionId, ExecutionState) {
        let mut state = ExecutionState::new();
        state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
        (state.execution_id.unwrap(), state)
    }

    fn pushed(event: Event) -> EventRecord {
        EventRecord {
            event,
            meta: EventMeta {
                event_id: Some(Uuid::new_v4()),
                ..EventMeta::default()
            },
        }
    }

    fn photo() -> AttachmentRecord {
        AttachmentRecord {
            filename: "p.jpg".to_string(),
            path: "attachments/aaaaaaa-p.jpg".to_string(),
            content_type: "image/jpeg".to_string(),
            sha256: "a".repeat(64),
        }
    }

    #[test]
    fn an_event_that_applies_is_recorded_and_one_that_doesnt_is_a_conflict() {
        let (id, mut state) = started();
        let complete = state
            .complete_event(procnote_core::event::types::CompletionStatus::Pass)
            .unwrap();
        let toggle = state
            .toggle_checkbox_event("step-0", "step-0/cb-0", true)
            .unwrap();

        let before = decide_pushed(id, &state, &pushed(toggle.clone()), &[]).unwrap();
        state.apply(&complete).unwrap();
        let after = decide_pushed(id, &state, &pushed(toggle), &[]).unwrap();

        assert!(matches!(before, Pushed::Record(d) if d.events.len() == 1));
        assert!(matches!(after, Pushed::Conflict(_)));
    }

    #[test]
    fn malformed_pushes_are_rejected() {
        let (id, state) = started();
        let toggle = state
            .toggle_checkbox_event("step-0", "step-0/cb-0", true)
            .unwrap();
        let without_id = EventRecord::from(toggle.clone());
        let elsewhere = decide_pushed(Uuid::new_v4(), &state, &pushed(toggle), &[]);
        let log_meta = pushed(Event::LogMeta {
            at: Utc::now(),
            version: 2,
            tool_version: "x".to_string(),
        });

        assert!(decide_pushed(id, &state, &without_id, &[]).is_err());
        assert!(elsewhere.is_err());
        assert!(decide_pushed(id, &state, &log_meta, &[]).is_err());
    }

    #[test]
    fn attachments_need_their_content() {
        let (id, state) = started();
        let add = pushed(
            state
                .add_attachments_event("step-0", "photo", vec![photo()])
                .unwrap(),
        );
        let upload = StagedFile {
            record: photo(),
            source: std::path::PathBuf::from("upload"),
        };
        let mismatched = StagedFile {
            record: AttachmentRecord {
                path: "attachments/aaaaaaa-other.jpg".to_string(),
                ..photo()
            },
            ..upload.clone()
        };

        assert!(decide_pushed(id, &state, &add, &[]).is_err());
        assert!(decide_pushed(id, &state, &add, &[mismatched]).is_err());
        assert!(matches!(
            decide_pushed(id, &state, &add, &[upload]).unwrap(),
            Pushed::Record(d) if d.files.len() == 1
        ));
    }
}
