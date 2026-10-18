//! Where executions are stored, independent of the storage technology.
//!
//! [`ExecutionRepository`] is implemented by the filesystem store
//! ([`crate::persistence::execution_store::ExecutionStore`]) and, for the
//! server, by database-backed stores. Deciding *what* to record is shared
//! and pure ([`decide_action`], [`decide_receive`]); the repository decides
//! *how* to record it atomically.

use std::path::{Path, PathBuf};

use procnote_core::event::types::{AttachmentRecord, Event, EventMeta, ExecutionId};
use procnote_core::execution::ExecutionState;
use procnote_core::template::types::ProcedureTemplate;

use crate::action::ExecutionAction;
use crate::execution::infer_attachment_content_type;
use crate::persistence::attachment_store::{compute_sha256, sanitize_attachment_filename};

/// An execution as stored: its replayed state and, for file storage, the
/// directory it lives in.
pub struct StoredExecution {
    pub state: ExecutionState,
    pub location: Option<String>,
}

/// A file to store alongside the events that reference it.
///
/// Its record (stored path, content type, SHA-256) is computed from the
/// source file before any lock is taken; stored paths are content-addressed
/// (`attachments/{sha256_7}-{filename}`).
#[derive(Debug, Clone)]
pub struct StagedFile {
    pub record: AttachmentRecord,
    pub source: PathBuf,
}

impl StagedFile {
    pub fn new(source: &Path, filename: &str) -> Result<Self, String> {
        let filename = sanitize_attachment_filename(filename);
        let sha256 = compute_sha256(source).map_err(|e| e.to_string())?;
        Ok(Self {
            record: AttachmentRecord {
                path: format!("attachments/{}-{filename}", &sha256[..7]),
                content_type: infer_attachment_content_type(&filename),
                filename,
                sha256,
            },
            source: source.to_path_buf(),
        })
    }
}

/// What to record: events to append, in order, and the files they reference.
#[derive(Debug, Default)]
pub struct Decision {
    pub events: Vec<Event>,
    pub files: Vec<StagedFile>,
}

/// The template an execution was started from, kept as it was.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TemplateSnapshot {
    /// A workspace `template.md`.
    Markdown(String),
    /// A shared template, as `ProcedureTemplate` JSON.
    Json(String),
}

impl TemplateSnapshot {
    pub const MARKDOWN_FILE: &str = "template.md";
    pub const JSON_FILE: &str = "template.json";

    /// The snapshot of a template that only exists as data.
    pub fn of_template(template: &ProcedureTemplate) -> Result<Self, String> {
        serde_json::to_string_pretty(template)
            .map(Self::Json)
            .map_err(|e| e.to_string())
    }

    /// The snapshot stored as `format` (see [`Self::format`]).
    pub fn from_format(format: &str, text: String) -> Result<Self, String> {
        match format {
            "markdown" => Ok(Self::Markdown(text)),
            "json" => Ok(Self::Json(text)),
            other => Err(format!("unknown template snapshot format: {other}")),
        }
    }

    #[must_use]
    pub const fn format(&self) -> &'static str {
        match self {
            Self::Markdown(_) => "markdown",
            Self::Json(_) => "json",
        }
    }

    /// The file name of the snapshot in an execution directory.
    #[must_use]
    pub const fn file_name(&self) -> &'static str {
        match self {
            Self::Markdown(_) => Self::MARKDOWN_FILE,
            Self::Json(_) => Self::JSON_FILE,
        }
    }

    #[must_use]
    pub fn text(&self) -> &str {
        match self {
            Self::Markdown(text) | Self::Json(text) => text,
        }
    }
}

/// A new execution to store.
pub struct NewExecution {
    /// The procedure's directory name under the procedures directory.
    pub procedure: String,
    /// The template at execution start.
    pub template: TemplateSnapshot,
    /// State after applying `initial_events`.
    pub state: ExecutionState,
    pub initial_events: Vec<Event>,
    /// Recorded in the log's `LogMeta`.
    pub tool_version: String,
    pub meta: EventMeta,
}

/// Decides what a request records, given the execution's current state.
pub type Decide<'a> = dyn FnMut(&ExecutionState) -> Result<Decision, String> + 'a;

pub trait ExecutionRepository: Send + Sync {
    /// Every readable execution. Unreadable ones are skipped (and logged).
    fn list(&self) -> Result<Vec<StoredExecution>, String>;

    fn load(&self, execution_id: ExecutionId) -> Result<StoredExecution, String>;

    fn create(&self, new: NewExecution) -> Result<StoredExecution, String>;

    /// Record what `decide` returns, atomically with respect to other writers
    /// of the same execution: replay the current state, call `decide`, apply
    /// its events, store its files, then append the events with `meta`.
    ///
    /// When `meta.event_id` is already recorded the request is a retry:
    /// `decide` is not called and the current state is returned.
    fn record(
        &self,
        execution_id: ExecutionId,
        meta: &EventMeta,
        decide: &mut Decide<'_>,
    ) -> Result<StoredExecution, String>;

    /// The content of a stored attachment, by its stored path.
    fn read_attachment(&self, execution_id: ExecutionId, path: &str) -> Result<Vec<u8>, String>;
}

/// Apply a decision's events to `state` in order, validating each.
pub fn apply_decision(state: &mut ExecutionState, decision: &Decision) -> Result<(), String> {
    decision
        .events
        .iter()
        .try_for_each(|event| state.apply(event).map_err(|e| e.to_string()))
}

/// The event an action records. `files` are the staged attachment sources of
/// an `AddAttachment(s)` action, in the action's order.
pub fn decide_action(
    state: &ExecutionState,
    action: ExecutionAction,
    files: Vec<StagedFile>,
) -> Result<Decision, String> {
    let event = match action {
        ExecutionAction::SkipStep { step_id, reason } => state.skip_step_event(&step_id, &reason),
        ExecutionAction::UnskipStep { step_id, reason } => {
            state.unskip_step_event(&step_id, &reason)
        }
        ExecutionAction::ToggleCheckbox {
            step_id,
            checkbox_id,
            checked,
        } => state.toggle_checkbox_event(&step_id, &checkbox_id, checked),
        ExecutionAction::RecordInput {
            step_id,
            input_id,
            value,
            unit,
        } => state.record_input_event(&step_id, &input_id, &value, unit.as_deref()),
        ExecutionAction::ClearInput {
            step_id,
            input_id,
            reason,
        } => state.clear_input_event(&step_id, &input_id, &reason),
        ExecutionAction::AddNote { text, step_id } => {
            state.add_note_event(&text, step_id.as_deref())
        }
        ExecutionAction::RemoveNote { note_id, reason } => {
            state.remove_note_event(&note_id, &reason)
        }
        ExecutionAction::AddStep {
            step_id,
            heading,
            content,
            after_step_id,
        } => state.add_step_event(&step_id, &heading, content, after_step_id.as_deref()),
        ExecutionAction::AddAttachment {
            step_id, input_id, ..
        }
        | ExecutionAction::AddAttachments {
            step_id, input_id, ..
        } => {
            if files.is_empty() {
                return Err("at least one attachment file is required".to_string());
            }
            let records = files.iter().map(|file| file.record.clone()).collect();
            let event = state
                .add_attachments_event(&step_id, &input_id, records)
                .map_err(|e| e.to_string())?;
            return Ok(Decision {
                events: vec![event],
                files,
            });
        }
        ExecutionAction::RemoveAttachmentFile {
            step_id,
            input_id,
            path,
        } => state.remove_attachment_file_event(&step_id, &input_id, &path),
        ExecutionAction::ClearAttachments { step_id, input_id } => {
            state.clear_attachments_event(&step_id, &input_id)
        }
        ExecutionAction::RemoveAttachment {
            step_id,
            input_id,
            reason,
        } => state.remove_attachment_event(&step_id, &input_id, &reason),
        ExecutionAction::AssignAttachment {
            step_id,
            input_id,
            path,
        } => state.assign_attachment_event(&step_id, &input_id, &path),
        ExecutionAction::UnassignAttachment {
            step_id,
            input_id,
            path,
        } => state.unassign_attachment_event(&step_id, &input_id, &path),
        ExecutionAction::Complete { status } => state.complete_event(status),
        ExecutionAction::Abort { reason } => state.abort_event(&reason),
        ExecutionAction::RenameExecution { name } => state.rename_event(&name),
        ExecutionAction::ReopenExecution { reason } => state.reopen_event(&reason),
    }
    .map_err(|e| e.to_string())?;
    Ok(Decision {
        events: vec![event],
        files: Vec::new(),
    })
}

/// Receive files into the inbox, skipping content the execution already has
/// (compared by SHA-256, including earlier files of the same batch).
pub fn decide_receive(state: &ExecutionState, files: Vec<StagedFile>) -> Result<Decision, String> {
    let mut next = state.clone();
    let mut decision = Decision::default();
    for file in files {
        if has_content(&next, &file.record.sha256) || next.has_attachment_path(&file.record.path) {
            continue;
        }
        let event = next
            .receive_attachment_event(file.record.clone())
            .map_err(|e| e.to_string())?;
        next.apply(&event).map_err(|e| e.to_string())?;
        decision.events.push(event);
        decision.files.push(file);
    }
    Ok(decision)
}

/// Whether the execution already holds a file with this content.
fn has_content(state: &ExecutionState, sha256: &str) -> bool {
    state
        .inbox
        .iter()
        .chain(
            state
                .steps
                .values()
                .flat_map(|step| step.attachments.values().flatten()),
        )
        .any(|file| file.sha256 == sha256)
}

/// Record an action. Attachment sources in `action` must already be
/// authorized by the caller; they are hashed before the write lock is taken.
pub fn record_action(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    action: ExecutionAction,
    meta: &EventMeta,
) -> Result<StoredExecution, String> {
    let files = match &action {
        ExecutionAction::AddAttachment { filename, path, .. } => {
            vec![StagedFile::new(Path::new(path), filename)?]
        }
        ExecutionAction::AddAttachments { files, .. } => files
            .iter()
            .map(|file| StagedFile::new(Path::new(&file.path), &file.filename))
            .collect::<Result<_, _>>()?,
        _ => Vec::new(),
    };
    let mut action = Some((action, files));
    repository.record(execution_id, meta, &mut |state| {
        let (action, files) = action
            .take()
            .ok_or_else(|| "action was already decided".to_string())?;
        decide_action(state, action, files)
    })
}

/// Receive `(source, filename)` files into the execution's inbox.
///
/// Returns the new state and how many files were received; files whose
/// content the execution already has are skipped.
pub fn receive_files(
    repository: &dyn ExecutionRepository,
    execution_id: ExecutionId,
    files: &[(PathBuf, String)],
    meta: &EventMeta,
) -> Result<(StoredExecution, usize), String> {
    let staged = files
        .iter()
        .map(|(source, filename)| StagedFile::new(source, filename))
        .collect::<Result<Vec<_>, _>>()?;
    let mut received = 0;
    let stored = repository.record(execution_id, meta, &mut |state| {
        let decision = decide_receive(state, staged.clone())?;
        received = decision.events.len();
        Ok(decision)
    })?;
    Ok((stored, received))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_core::template::parse_template;

    const TEMPLATE: &str = r"---
id: decide-test
title: Decide Test
version: 1.0.0
---

## Capture

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";

    fn started() -> ExecutionState {
        let mut state = ExecutionState::new();
        state.start(&parse_template(TEMPLATE).unwrap()).unwrap();
        state
    }

    fn staged(name: &str, sha256: &str) -> StagedFile {
        StagedFile {
            record: AttachmentRecord {
                filename: name.to_string(),
                path: format!("attachments/{}-{name}", &sha256[..7]),
                content_type: "text/plain".to_string(),
                sha256: sha256.to_string(),
            },
            source: PathBuf::from(name),
        }
    }

    #[test]
    fn receiving_skips_content_already_received_in_the_same_batch_or_before() {
        let mut state = started();
        let first = decide_receive(&state, vec![staged("a.txt", &"a".repeat(64))]).unwrap();
        apply_decision(&mut state, &first).unwrap();

        let next = decide_receive(
            &state,
            vec![
                staged("again.txt", &"a".repeat(64)),
                staged("b.txt", &"b".repeat(64)),
                staged("b-copy.txt", &"b".repeat(64)),
            ],
        )
        .unwrap();

        let names: Vec<&str> = next
            .files
            .iter()
            .map(|file| file.record.filename.as_str())
            .collect();
        assert_eq!(names, ["b.txt"]);
        assert_eq!(next.events.len(), 1);
    }

    #[test]
    fn adding_attachments_records_the_staged_files() {
        let state = started();
        let action = || ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            files: Vec::new(),
        };

        let decision =
            decide_action(&state, action(), vec![staged("a.txt", &"a".repeat(64))]).unwrap();
        let empty = decide_action(&state, action(), Vec::new());

        assert_eq!(decision.files.len(), 1);
        assert!(matches!(
            decision.events.as_slice(),
            [Event::AttachmentsAdded { attachments, .. }] if attachments.len() == 1
        ));
        assert!(empty.is_err());
    }

    #[test]
    fn an_invalid_action_is_rejected() {
        let state = started();

        let result = decide_action(
            &state,
            ExecutionAction::SkipStep {
                step_id: "no-such-step".to_string(),
                reason: "x".to_string(),
            },
            Vec::new(),
        );

        assert!(result.is_err());
    }
}
