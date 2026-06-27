use serde::Deserialize;
use ts_rs::TS;

use procnote_core::event::types::CompletionStatus;
use procnote_core::template::types::StepContent;

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
