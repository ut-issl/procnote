use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::template::types::StepContent;

/// Unique identifier for an execution.
pub type ExecutionId = Uuid;

/// Completion status of an execution.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq, ts_rs::TS)]
#[ts(export)]
#[serde(rename_all = "lowercase")]
pub enum CompletionStatus {
    Pass,
    Fail,
    Aborted,
}

/// All events that can occur during a procedure execution.
///
/// Events are internally tagged with `"type"` for JSON serialization,
/// and every event carries an `at` timestamp.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    // -- Lifecycle --
    ExecutionStarted {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        procedure_id: String,
        procedure_title: String,
        procedure_version: String,
    },
    ExecutionCompleted {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        status: CompletionStatus,
    },
    ExecutionAborted {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        reason: String,
    },

    // -- Step --
    StepAdded {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        /// Stable element ID for this step.
        step_id: String,
        heading: String,
        /// Ordered content items from the template (prose, checkboxes, input blocks).
        /// Checkbox and input items carry their own IDs.
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        content: Vec<StepContent>,
        /// Insert after this step ID. `None` means append at end.
        #[serde(skip_serializing_if = "Option::is_none")]
        after_step_id: Option<String>,
    },
    StepSkipped {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        step_id: String,
        reason: String,
    },

    // -- Data --
    CheckboxToggled {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        step_id: String,
        checkbox_id: String,
        checked: bool,
    },
    InputRecorded {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        step_id: String,
        input_id: String,
        value: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        unit: Option<String>,
    },
    NoteAdded {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        text: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        step_id: Option<String>,
    },

    // -- Attachment --
    AttachmentAdded {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        step_id: String,
        input_id: String,
        filename: String,
        path: String,
        content_type: String,
        sha256: String,
    },

    // -- Name --
    ExecutionRenamed {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        name: String,
    },

    // -- Revert --
    /// Marks a previously recorded event as reverted.
    /// State is rebuilt by replaying all events, skipping reverted ones.
    EventReverted {
        at: DateTime<Utc>,
        execution_id: ExecutionId,
        /// Zero-based index of the event in the log to revert.
        reverted_event_index: usize,
        /// Human-readable reason for the revert (audit trail).
        reason: String,
    },

    // -- Log metadata --
    /// First-line metadata about the event log format.
    /// Enables future migration code to detect schema version before replay.
    LogMeta {
        at: DateTime<Utc>,
        /// Schema version of the event log (currently 1).
        version: u32,
        /// Version of the procnote tool that created this log.
        tool_version: String,
    },
}

/// Collect the indices of events that have been marked as reverted
/// by an `EventReverted` marker somewhere in the log.
#[must_use]
pub fn reverted_event_indices(events: &[Event]) -> HashSet<usize> {
    events
        .iter()
        .filter_map(|event| match event {
            Event::EventReverted {
                reverted_event_index,
                ..
            } => Some(*reverted_event_index),
            _ => None,
        })
        .collect()
}

/// Whether an event can be reverted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Revertibility {
    /// This event can be reverted by the user.
    Revertible,
    /// This event cannot be reverted (structural/lifecycle).
    NotRevertible,
    /// This event is itself a revert marker and cannot be reverted.
    RevertMarker,
}

impl Event {
    /// Classify whether this event can be reverted.
    ///
    /// This match is exhaustive — adding a new `Event` variant without
    /// updating this method will cause a compile error.
    #[must_use]
    pub const fn revertibility(&self) -> Revertibility {
        match self {
            // Lifecycle/structural/metadata — not revertible
            Self::ExecutionStarted { .. } | Self::StepAdded { .. } | Self::LogMeta { .. } => {
                Revertibility::NotRevertible
            }

            // Everything else (except revert markers) — revertible
            Self::ExecutionCompleted { .. }
            | Self::ExecutionAborted { .. }
            | Self::StepSkipped { .. }
            | Self::CheckboxToggled { .. }
            | Self::InputRecorded { .. }
            | Self::NoteAdded { .. }
            | Self::AttachmentAdded { .. }
            | Self::ExecutionRenamed { .. } => Revertibility::Revertible,

            // Revert marker — not revertible
            Self::EventReverted { .. } => Revertibility::RevertMarker,
        }
    }

    /// Human-readable description of this event for UI display.
    ///
    /// This match is exhaustive — adding a new `Event` variant without
    /// updating this method will cause a compile error.
    #[must_use]
    pub fn description(&self) -> String {
        match self {
            Self::ExecutionStarted { procedure_id, .. } => {
                format!("Started execution of {procedure_id}")
            }
            Self::ExecutionCompleted { status, .. } => {
                format!("Completed execution: {status:?}")
            }
            Self::ExecutionAborted { reason, .. } => {
                format!("Aborted execution: {reason}")
            }
            Self::StepAdded { heading, .. } => format!("Added step: {heading}"),
            Self::StepSkipped {
                step_id, reason, ..
            } => {
                format!("Skipped step: {step_id} ({reason})")
            }
            Self::CheckboxToggled {
                checkbox_id,
                checked,
                ..
            } => {
                let verb = if *checked { "Checked" } else { "Unchecked" };
                format!("{verb} checkbox {checkbox_id}")
            }
            Self::InputRecorded {
                input_id, value, ..
            } => {
                format!("Recorded {input_id} = {value}")
            }
            Self::NoteAdded { text, step_id, .. } => {
                let scope = step_id
                    .as_ref()
                    .map(|id| format!(" to {id}"))
                    .unwrap_or_default();
                let truncated = if text.chars().count() > 50 {
                    let head: String = text.chars().take(50).collect();
                    format!("{head}...")
                } else {
                    text.clone()
                };
                format!("Added note{scope}: {truncated}")
            }
            Self::AttachmentAdded {
                input_id, filename, ..
            } => {
                format!("Recorded {input_id} = {filename}")
            }
            Self::ExecutionRenamed { name, .. } => {
                format!("Renamed execution to: {name}")
            }
            Self::EventReverted {
                reverted_event_index,
                reason,
                ..
            } => {
                format!("Reverted event #{reverted_event_index}: {reason}")
            }
            Self::LogMeta {
                version,
                tool_version,
                ..
            } => {
                format!("Log metadata v{version} (tool {tool_version})")
            }
        }
    }
}
