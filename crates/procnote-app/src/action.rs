use serde::{Deserialize, Serialize};
use ts_rs::TS;

use procnote_core::event::types::{CompletionStatus, EventMeta};
use procnote_core::template::types::StepContent;

/// A file selected by the trusted native file picker for attachment storage.
#[derive(Debug, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct AttachmentSource {
    pub filename: String,
    pub path: String,
    pub content_type: String,
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
    UnskipStep {
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
    ClearInput {
        step_id: String,
        input_id: String,
        reason: String,
    },
    AddNote {
        text: String,
        #[ts(optional)]
        step_id: Option<String>,
    },
    RemoveNote {
        note_id: String,
        reason: String,
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
    AddAttachments {
        step_id: String,
        input_id: String,
        files: Vec<AttachmentSource>,
    },
    RemoveAttachmentFile {
        step_id: String,
        input_id: String,
        path: String,
    },
    ClearAttachments {
        step_id: String,
        input_id: String,
    },
    RemoveAttachment {
        step_id: String,
        input_id: String,
        reason: String,
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
    ReopenExecution {
        reason: String,
    },
}

/// Who performs an action and from where, sent by the frontend with each action.
#[derive(Debug, Default, Deserialize, Serialize, TS)]
#[ts(export)]
pub struct ActionMeta {
    /// Client-generated ID used to recognize retries of the same action.
    #[serde(default)]
    #[ts(optional)]
    pub event_id: Option<uuid::Uuid>,
    /// Operator name as entered in the UI.
    #[serde(default)]
    #[ts(optional)]
    pub recorded_by: Option<String>,
    /// Stable per-device identifier.
    #[serde(default)]
    #[ts(optional)]
    pub device: Option<String>,
}

impl ActionMeta {
    /// The metadata to store with the resulting event; blank strings are omitted.
    #[must_use]
    pub fn into_event_meta(self) -> EventMeta {
        let non_blank = |value: Option<String>| {
            value
                .map(|v| v.trim().to_string())
                .filter(|v| !v.is_empty())
        };
        EventMeta {
            event_id: self.event_id,
            recorded_by: non_blank(self.recorded_by),
            device: non_blank(self.device),
        }
    }
}

/// Mutable view of one attachment source referenced by an [`ExecutionAction`].
///
/// Shells use it to replace frontend-supplied file references with trusted
/// ones before the action reaches the execution store.
pub struct AttachmentSourceMut<'a> {
    pub filename: &'a mut String,
    pub path: &'a mut String,
    pub content_type: &'a mut String,
}

impl ExecutionAction {
    /// Attachment sources that this action would copy into the execution.
    pub fn attachment_sources_mut(&mut self) -> Vec<AttachmentSourceMut<'_>> {
        match self {
            Self::AddAttachment {
                filename,
                path,
                content_type,
                ..
            } => vec![AttachmentSourceMut {
                filename,
                path,
                content_type,
            }],
            Self::AddAttachments { files, .. } => files
                .iter_mut()
                .map(|file| AttachmentSourceMut {
                    filename: &mut file.filename,
                    path: &mut file.path,
                    content_type: &mut file.content_type,
                })
                .collect(),
            Self::SkipStep { .. }
            | Self::UnskipStep { .. }
            | Self::ToggleCheckbox { .. }
            | Self::RecordInput { .. }
            | Self::ClearInput { .. }
            | Self::AddNote { .. }
            | Self::RemoveNote { .. }
            | Self::AddStep { .. }
            | Self::RemoveAttachmentFile { .. }
            | Self::ClearAttachments { .. }
            | Self::RemoveAttachment { .. }
            | Self::Complete { .. }
            | Self::Abort { .. }
            | Self::RenameExecution { .. }
            | Self::ReopenExecution { .. } => Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(path: &str) -> AttachmentSource {
        AttachmentSource {
            filename: format!("{path}.txt"),
            path: path.to_string(),
            content_type: "text/plain".to_string(),
        }
    }

    #[test]
    fn add_attachment_exposes_its_single_source() {
        let mut action = ExecutionAction::AddAttachment {
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            filename: "a.txt".to_string(),
            path: "a".to_string(),
            content_type: "text/plain".to_string(),
        };

        for source in action.attachment_sources_mut() {
            *source.path = "trusted".to_string();
        }

        assert!(matches!(
            action,
            ExecutionAction::AddAttachment { ref path, .. } if path == "trusted"
        ));
    }

    #[test]
    fn add_attachments_exposes_every_source_in_order() {
        let mut action = ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            files: vec![source("a"), source("b")],
        };

        let paths: Vec<String> = action
            .attachment_sources_mut()
            .into_iter()
            .map(|source| source.path.clone())
            .collect();

        assert_eq!(paths, ["a", "b"]);
    }

    #[test]
    fn action_meta_drops_blank_strings_and_trims() {
        let id = uuid::Uuid::new_v4();
        let meta = ActionMeta {
            event_id: Some(id),
            recorded_by: Some("  Nomura ".to_string()),
            device: Some("   ".to_string()),
        }
        .into_event_meta();

        assert_eq!(
            meta,
            EventMeta {
                event_id: Some(id),
                recorded_by: Some("Nomura".to_string()),
                device: None,
            }
        );
    }

    #[test]
    fn other_actions_have_no_sources() {
        let mut action = ExecutionAction::AddNote {
            text: "note".to_string(),
            step_id: None,
        };

        assert!(action.attachment_sources_mut().is_empty());
    }
}
