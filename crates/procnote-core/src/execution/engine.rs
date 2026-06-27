use std::collections::HashMap;

use chrono::Utc;
use uuid::Uuid;

use crate::event::types::{
    CompletionStatus, Event, ExecutionId, Revertibility, reverted_event_indices,
};
use crate::template::types::{ProcedureTemplate, StepContent};

/// Errors that can occur during execution state transitions.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ExecutionError {
    #[error("execution has not been started")]
    NotStarted,
    #[error("execution has already been started")]
    AlreadyStarted,
    #[error("execution has already finished")]
    AlreadyFinished,
    #[error("step not found: {0}")]
    StepNotFound(String),
    #[error("step already skipped: {0}")]
    StepAlreadySkipped(String),
    #[error("duplicate step id: {0}")]
    DuplicateStepId(String),
    #[error("event index out of range: {0}")]
    EventIndexOutOfRange(usize),
    #[error("event at index {0} is not revertible")]
    EventNotRevertible(usize),
    #[error("event at index {0} has already been reverted")]
    EventAlreadyReverted(usize),
    #[error("reverting event at index {0} would produce an invalid state: {1}")]
    RevertWouldInvalidateState(usize, String),
}

/// Status of the overall execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ExecutionStatus {
    /// Not yet started.
    Pending,
    /// In progress.
    Active,
    /// Finished (pass, fail, or aborted).
    Finished(CompletionStatus),
}

/// Status of a single step.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StepStatus {
    Present,
    Skipped,
}

/// Tracked state for a single step during execution.
#[derive(Debug, Clone)]
pub struct StepState {
    /// Stable element ID for this step.
    pub id: String,
    pub heading: String,
    pub status: StepStatus,
    /// Ordered content items from the template (prose, checkboxes, input blocks).
    /// Checkbox `checked` state is mutated in-place.
    pub content: Vec<StepContent>,
    /// Recorded input values keyed by label.
    pub inputs: HashMap<String, RecordedInput>,
    pub notes: Vec<String>,
}

/// A recorded input value.
#[derive(Debug, Clone)]
pub struct RecordedInput {
    pub label: String,
    pub value: String,
    pub unit: Option<String>,
}

/// The full state of a procedure execution, reconstructable from events.
#[derive(Debug)]
pub struct ExecutionState {
    pub execution_id: Option<ExecutionId>,
    pub procedure_id: Option<String>,
    pub procedure_title: Option<String>,
    pub procedure_version: Option<String>,
    pub name: Option<String>,

    pub status: ExecutionStatus,
    /// Ordered step headings (preserves insertion order).
    pub step_order: Vec<String>,
    pub steps: HashMap<String, StepState>,
    pub global_notes: Vec<String>,
}

impl ExecutionState {
    /// Create a new empty execution state.
    #[must_use]
    pub fn new() -> Self {
        Self {
            execution_id: None,
            procedure_id: None,
            procedure_title: None,
            procedure_version: None,
            name: None,
            status: ExecutionStatus::Pending,
            step_order: Vec::new(),
            steps: HashMap::new(),
            global_notes: Vec::new(),
        }
    }

    /// Reconstruct execution state by replaying a sequence of events,
    /// respecting `EventReverted` markers.
    ///
    /// First collects all reverted indices, then replays only non-reverted
    /// events. `EventReverted` and `LogMeta` events are skipped.
    pub fn from_events(events: &[Event]) -> Result<Self, ExecutionError> {
        let reverted_indices = reverted_event_indices(events);

        let mut state = Self::new();
        for (index, event) in events.iter().enumerate() {
            if reverted_indices.contains(&index) {
                continue;
            }
            if matches!(event, Event::EventReverted { .. } | Event::LogMeta { .. }) {
                continue;
            }
            state.apply(event)?;
        }
        Ok(state)
    }

    /// Apply a single event to the state (used by both replay and transitions).
    #[expect(
        clippy::too_many_lines,
        reason = "exhaustive match over all Event variants for state machine"
    )]
    pub fn apply(&mut self, event: &Event) -> Result<(), ExecutionError> {
        match event {
            Event::ExecutionStarted {
                execution_id,
                procedure_id,
                procedure_title,
                procedure_version,
                ..
            } => {
                match &self.status {
                    ExecutionStatus::Pending => {}
                    ExecutionStatus::Active => return Err(ExecutionError::AlreadyStarted),
                    ExecutionStatus::Finished(_) => return Err(ExecutionError::AlreadyFinished),
                }
                self.execution_id = Some(*execution_id);
                self.procedure_id = Some(procedure_id.clone());
                self.procedure_title = Some(procedure_title.clone());
                self.procedure_version = Some(procedure_version.clone());
                self.status = ExecutionStatus::Active;
            }
            Event::ExecutionCompleted { status, .. } => {
                self.require_active()?;
                self.status = ExecutionStatus::Finished(status.clone());
            }
            Event::ExecutionAborted { .. } => {
                self.require_active()?;
                self.status = ExecutionStatus::Finished(CompletionStatus::Aborted);
            }
            Event::StepAdded {
                step_id,
                heading,
                content,
                after_step_id,
                ..
            } => {
                self.require_active()?;
                if self.steps.contains_key(step_id) {
                    return Err(ExecutionError::DuplicateStepId(step_id.clone()));
                }
                let step_state = StepState {
                    id: step_id.clone(),
                    heading: heading.clone(),
                    status: StepStatus::Present,
                    content: content.clone(),
                    inputs: HashMap::new(),
                    notes: Vec::new(),
                };
                self.steps.insert(step_id.clone(), step_state);
                match after_step_id {
                    Some(after) => {
                        if let Some(pos) = self.step_order.iter().position(|id| id == after) {
                            self.step_order.insert(pos + 1, step_id.clone());
                        } else {
                            self.step_order.push(step_id.clone());
                        }
                    }
                    None => {
                        self.step_order.push(step_id.clone());
                    }
                }
            }
            Event::StepSkipped { step_id, .. } => {
                self.require_active()?;
                let step = self.get_step_mut(step_id)?;
                match step.status {
                    StepStatus::Present => {
                        step.status = StepStatus::Skipped;
                    }
                    StepStatus::Skipped => {
                        return Err(ExecutionError::StepAlreadySkipped(step_id.clone()));
                    }
                }
            }
            Event::CheckboxToggled {
                step_id,
                checkbox_id,
                checked,
                ..
            } => {
                self.require_active()?;
                let step = self.get_step_mut(step_id)?;
                // Find matching checkbox in content by ID and update in-place.
                let found = step.content.iter_mut().any(|item| {
                    if let StepContent::Checkbox {
                        id: Some(id),
                        checked: c,
                        ..
                    } = item
                        && id == checkbox_id
                    {
                        *c = *checked;
                        return true;
                    }
                    false
                });
                if !found {
                    // Checkbox not from template — add dynamically.
                    step.content.push(StepContent::Checkbox {
                        id: Some(checkbox_id.clone()),
                        text: String::new(),
                        checked: *checked,
                    });
                }
            }
            Event::InputRecorded {
                step_id,
                input_id,
                value,
                unit,
                ..
            } => {
                self.require_active()?;
                let step = self.get_step_mut(step_id)?;
                step.inputs.insert(
                    input_id.clone(),
                    RecordedInput {
                        label: input_id.clone(),
                        value: value.clone(),
                        unit: unit.clone(),
                    },
                );
            }
            Event::NoteAdded { text, step_id, .. } => {
                self.require_active()?;
                match step_id {
                    Some(id) => {
                        let step = self.get_step_mut(id)?;
                        step.notes.push(text.clone());
                    }
                    None => {
                        self.global_notes.push(text.clone());
                    }
                }
            }

            Event::AttachmentAdded {
                step_id,
                input_id,
                filename,
                ..
            } => {
                self.require_active()?;
                let step = self.get_step_mut(step_id)?;
                step.inputs.insert(
                    input_id.clone(),
                    RecordedInput {
                        label: input_id.clone(),
                        value: filename.clone(),
                        unit: None,
                    },
                );
            }

            Event::ExecutionRenamed { name, .. } => {
                if self.execution_id.is_none() {
                    return Err(ExecutionError::NotStarted);
                }
                self.name = Some(name.clone());
            }

            // EventReverted is handled at the from_events() level by skipping
            // reverted events. It should not be applied directly.
            Event::EventReverted { .. } | Event::LogMeta { .. } => {}
        }
        Ok(())
    }

    // -- Transition methods: produce events --

    /// Start a new execution from a template.
    pub fn start(&mut self, template: &ProcedureTemplate) -> Result<Vec<Event>, ExecutionError> {
        if self.status != ExecutionStatus::Pending {
            return Err(ExecutionError::AlreadyStarted);
        }
        let execution_id = Uuid::new_v4();
        let now = Utc::now();

        let mut events = Vec::new();

        // Execution started event.
        let started = Event::ExecutionStarted {
            at: now,
            execution_id,
            procedure_id: template.metadata.id.clone(),
            procedure_title: template.metadata.title.clone(),
            procedure_version: template.metadata.version.clone(),
        };
        self.apply(&started)?;
        events.push(started);

        // Auto-generate a name for the execution.
        let auto_name = names::Generator::default()
            .next()
            .unwrap_or_else(|| format!("execution-{}", &execution_id.to_string()[..8]));
        let named = Event::ExecutionRenamed {
            at: now,
            execution_id,
            name: auto_name,
        };
        self.apply(&named)?;
        events.push(named);

        // Add steps from the template, preserving content order.
        // Assign stable IDs to each step and its interactive content items.
        for (step_index, step) in template.steps.iter().enumerate() {
            let step_id = format!("step-{step_index}");

            // Assign IDs to content items.
            let mut cb_index = 0usize;
            let content: Vec<StepContent> = step
                .content
                .iter()
                .map(|item| match item {
                    StepContent::Checkbox { text, checked, .. } => {
                        let id = format!("{step_id}/cb-{cb_index}");
                        cb_index += 1;
                        StepContent::Checkbox {
                            id: Some(id),
                            text: text.clone(),
                            checked: *checked,
                        }
                    }
                    // InputDefinition already has its own `id` from the template YAML.
                    other => other.clone(),
                })
                .collect();

            let step_added = Event::StepAdded {
                at: now,
                execution_id,
                step_id,
                heading: step.heading.clone(),
                content,
                after_step_id: None,
            };
            self.apply(&step_added)?;
            events.push(step_added);
        }

        Ok(events)
    }

    /// Rename the execution.
    ///
    /// Unlike most actions, this works on both active and finished executions
    /// (it's metadata, not a state transition).
    pub fn rename(&mut self, name: &str) -> Result<Event, ExecutionError> {
        let event = Event::ExecutionRenamed {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            name: name.to_string(),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Add a new step during execution.
    pub fn add_step(
        &mut self,
        step_id: &str,
        heading: &str,
        content: Vec<StepContent>,
        after_step_id: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::StepAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            heading: heading.to_string(),
            content,
            after_step_id: after_step_id.map(std::string::ToString::to_string),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Skip a step.
    pub fn skip_step(&mut self, step_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::StepSkipped {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            reason: reason.to_string(),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Toggle a checkbox in a step.
    pub fn toggle_checkbox(
        &mut self,
        step_id: &str,
        checkbox_id: &str,
        checked: bool,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::CheckboxToggled {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            checkbox_id: checkbox_id.to_string(),
            checked,
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Record an input value.
    pub fn record_input(
        &mut self,
        step_id: &str,
        input_id: &str,
        value: &str,
        unit: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::InputRecorded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            value: value.to_string(),
            unit: unit.map(std::string::ToString::to_string),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Add a note.
    pub fn add_note(&mut self, text: &str, step_id: Option<&str>) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::NoteAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            text: text.to_string(),
            step_id: step_id.map(std::string::ToString::to_string),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Add an attachment.
    pub fn add_attachment(
        &mut self,
        step_id: &str,
        input_id: &str,
        filename: &str,
        path: &str,
        content_type: &str,
        sha256: &str,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::AttachmentAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            filename: filename.to_string(),
            path: path.to_string(),
            content_type: content_type.to_string(),
            sha256: sha256.to_string(),
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Complete the execution.
    pub fn complete(&mut self, status: CompletionStatus) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::ExecutionCompleted {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            status,
        };
        self.apply(&event)?;
        Ok(event)
    }

    /// Abort the execution.
    pub fn abort(&mut self, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        let event = Event::ExecutionAborted {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            reason: reason.to_string(),
        };
        self.apply(&event)?;
        Ok(event)
    }

    // -- Revert --

    /// Produce an `EventReverted` marker for the event at the given index.
    ///
    /// Validates that the event is revertible, not already reverted, and that
    /// the resulting state would be consistent (via trial replay).
    pub fn revert_event(
        all_events: &[Event],
        event_index: usize,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        // Validate index is in range.
        let target_event = all_events
            .get(event_index)
            .ok_or(ExecutionError::EventIndexOutOfRange(event_index))?;

        // Validate the event is revertible.
        match target_event.revertibility() {
            Revertibility::Revertible => {}
            Revertibility::NotRevertible | Revertibility::RevertMarker => {
                return Err(ExecutionError::EventNotRevertible(event_index));
            }
        }

        // Check it hasn't already been reverted.
        let already_reverted = all_events.iter().any(|event| {
            matches!(
                event,
                Event::EventReverted {
                    reverted_event_index,
                    ..
                } if *reverted_event_index == event_index
            )
        });
        if already_reverted {
            return Err(ExecutionError::EventAlreadyReverted(event_index));
        }

        // Extract execution_id from the first ExecutionStarted event.
        let execution_id = all_events
            .iter()
            .find_map(|event| match event {
                Event::ExecutionStarted { execution_id, .. } => Some(*execution_id),
                _ => None,
            })
            .ok_or(ExecutionError::NotStarted)?;

        let revert_marker = Event::EventReverted {
            at: Utc::now(),
            execution_id,
            reverted_event_index: event_index,
            reason: reason.to_string(),
        };

        // Validate by trial replay: append the marker and rebuild.
        let mut trial_events = all_events.to_vec();
        trial_events.push(revert_marker.clone());
        Self::from_events(&trial_events)
            .map_err(|e| ExecutionError::RevertWouldInvalidateState(event_index, e.to_string()))?;

        Ok(revert_marker)
    }

    // -- Helpers --

    const fn require_active(&self) -> Result<(), ExecutionError> {
        match &self.status {
            ExecutionStatus::Pending => Err(ExecutionError::NotStarted),
            ExecutionStatus::Active => Ok(()),
            ExecutionStatus::Finished(_) => Err(ExecutionError::AlreadyFinished),
        }
    }

    fn require_execution_id(&self) -> Result<ExecutionId, ExecutionError> {
        self.execution_id.ok_or(ExecutionError::NotStarted)
    }

    fn get_step_mut(&mut self, step_id: &str) -> Result<&mut StepState, ExecutionError> {
        self.steps
            .get_mut(step_id)
            .ok_or_else(|| ExecutionError::StepNotFound(step_id.to_string()))
    }
}

impl Default for ExecutionState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use crate::template::types::{ProcedureMetadata, ProcedureTemplate, Step, StepContent};

    fn sample_template() -> ProcedureTemplate {
        ProcedureTemplate {
            metadata: ProcedureMetadata {
                id: "TVT-001".to_string(),
                title: "Thermal Vacuum Test".to_string(),
                version: "1.0".to_string(),
                author: Some("Nomura".to_string()),
                equipment: vec![],
                requirement_traces: vec![],
            },
            steps: vec![
                Step {
                    id: None,
                    heading: "Preconditions".to_string(),
                    content: vec![],
                },
                Step {
                    id: None,
                    heading: "Step 1: Power On".to_string(),
                    content: vec![],
                },
                Step {
                    id: None,
                    heading: "Postconditions".to_string(),
                    content: vec![],
                },
            ],
        }
    }

    #[test]
    fn test_start_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let events = state.start(&template).unwrap();

        // 1 ExecutionStarted + 1 ExecutionRenamed + 3 StepAdded
        assert_eq!(events.len(), 5);
        assert_eq!(state.status, ExecutionStatus::Active);
        assert!(state.name.is_some());
        assert_eq!(state.step_order.len(), 3);
        assert_eq!(state.step_order[0], "step-0");
        assert_eq!(state.step_order[1], "step-1");
        assert_eq!(state.step_order[2], "step-2");
        // Verify headings are still preserved
        assert_eq!(state.steps["step-0"].heading, "Preconditions");
        assert_eq!(state.steps["step-1"].heading, "Step 1: Power On");
        assert_eq!(state.steps["step-2"].heading, "Postconditions");
    }

    #[test]
    fn test_rename_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();

        let original_name = state.name.clone().unwrap();
        state.rename("my-custom-name").unwrap();
        assert_eq!(state.name.as_deref(), Some("my-custom-name"));
        assert_ne!(state.name.as_deref(), Some(original_name.as_str()));
    }

    #[test]
    fn test_rename_finished_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.complete(CompletionStatus::Pass).unwrap();

        // Renaming should work even after completion.
        state.rename("post-finish-name").unwrap();
        assert_eq!(state.name.as_deref(), Some("post-finish-name"));
    }

    #[test]
    fn test_cannot_rename_before_start() {
        let mut state = ExecutionState::new();
        let result = state.rename("some-name");
        assert_eq!(result.unwrap_err(), ExecutionError::NotStarted);
    }

    #[test]
    fn test_full_execution_flow() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut all_events: Vec<Event> = Vec::new();

        all_events.extend(state.start(&template).unwrap());
        all_events.push(
            state
                .toggle_checkbox("step-0", "step-0/cb-0", true)
                .unwrap(),
        );
        all_events.push(
            state
                .record_input("step-1", "current-draw", "120", Some("mA"))
                .unwrap(),
        );
        all_events.push(state.add_note("Voltage stable", Some("step-1")).unwrap());
        all_events.push(state.skip_step("step-2", "Not applicable").unwrap());
        all_events.push(state.complete(CompletionStatus::Pass).unwrap());

        assert_eq!(
            state.status,
            ExecutionStatus::Finished(CompletionStatus::Pass)
        );
        assert_eq!(state.steps["step-0"].status, StepStatus::Present);
        assert_eq!(state.steps["step-1"].status, StepStatus::Present);
        assert_eq!(state.steps["step-2"].status, StepStatus::Skipped);
        assert!(state.steps["step-0"].content.iter().any(|item| {
            matches!(item, StepContent::Checkbox { id: Some(id), checked, .. } if id == "step-0/cb-0" && *checked)
        }));
        assert_eq!(state.steps["step-1"].inputs["current-draw"].value, "120");
        assert_eq!(state.steps["step-1"].notes.len(), 1);

        let replayed = ExecutionState::from_events(&all_events).unwrap();
        assert_eq!(
            replayed.status,
            ExecutionStatus::Finished(CompletionStatus::Pass)
        );
        assert_eq!(replayed.step_order.len(), 3);
        assert!(replayed.steps["step-0"].content.iter().any(|item| {
            matches!(item, StepContent::Checkbox { id: Some(id), checked, .. } if id == "step-0/cb-0" && *checked)
        }));
    }

    #[test]
    fn test_add_step_during_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();

        state
            .add_step(
                "dyn-step-1",
                "Step 1.5: Verification",
                vec![StepContent::Prose {
                    text: "Extra verification step".to_string(),
                }],
                Some("step-1"),
            )
            .unwrap();

        assert_eq!(state.step_order.len(), 4);
        assert_eq!(state.step_order[0], "step-0");
        assert_eq!(state.step_order[1], "step-1");
        assert_eq!(state.step_order[2], "dyn-step-1");
        assert_eq!(state.step_order[3], "step-2");
        assert_eq!(state.steps["dyn-step-1"].status, StepStatus::Present);
    }

    #[test]
    fn test_cannot_start_twice() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        let result = state.start(&template);
        assert_eq!(result.unwrap_err(), ExecutionError::AlreadyStarted);
    }

    #[test]
    fn test_cannot_act_before_start() {
        let mut state = ExecutionState::new();
        let result = state.skip_step("step-0", "N/A");
        assert_eq!(result.unwrap_err(), ExecutionError::NotStarted);
    }

    #[test]
    fn test_cannot_act_after_finish() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.complete(CompletionStatus::Pass).unwrap();

        let result = state.skip_step("step-0", "N/A");
        assert_eq!(result.unwrap_err(), ExecutionError::AlreadyFinished);
    }

    #[test]
    fn test_cannot_skip_already_skipped_step() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.skip_step("step-0", "N/A").unwrap();

        let result = state.skip_step("step-0", "still N/A");
        assert_eq!(
            result.unwrap_err(),
            ExecutionError::StepAlreadySkipped("step-0".to_string())
        );
    }

    #[test]
    fn test_abort_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.abort("Power failure").unwrap();

        assert_eq!(
            state.status,
            ExecutionStatus::Finished(CompletionStatus::Aborted)
        );
    }

    #[test]
    fn test_attachment() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();

        state
            .add_attachment(
                "step-1",
                "log-file",
                "photo.jpg",
                "attachments/photo.jpg",
                "image/jpeg",
                "abc123",
            )
            .unwrap();

        let input = &state.steps["step-1"].inputs["log-file"];
        assert_eq!(input.value, "photo.jpg");
    }

    #[test]
    fn test_global_note() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();

        state.add_note("General observation", None).unwrap();

        assert_eq!(state.global_notes.len(), 1);
        assert_eq!(state.global_notes[0], "General observation");
    }

    #[test]
    fn test_duplicate_step_id() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();

        let result = state.add_step("step-0", "Preconditions Again", vec![], None);
        assert_eq!(
            result.unwrap_err(),
            ExecutionError::DuplicateStepId("step-0".to_string())
        );
    }

    // -- Revert tests --

    fn events_with_skipped_step() -> Vec<Event> {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(state.skip_step("step-0", "N/A").unwrap()); // index 5
        events
    }

    #[test]
    fn test_revert_step_skipped() {
        let mut events = events_with_skipped_step();
        let revert = ExecutionState::revert_event(&events, 5, "actually needed").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert_eq!(state.steps["step-0"].status, StepStatus::Present);
    }

    #[test]
    fn test_revert_input_recorded() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(
            state
                .record_input("step-0", "voltage", "5.0", Some("V"))
                .unwrap(),
        ); // index 5

        let revert = ExecutionState::revert_event(&events, 5, "wrong value").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert!(!state.steps["step-0"].inputs.contains_key("voltage"));
    }

    #[test]
    fn test_revert_note_added() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(state.add_note("oops", None).unwrap()); // index 5

        let revert = ExecutionState::revert_event(&events, 5, "typo").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert!(state.global_notes.is_empty());
    }

    #[test]
    fn test_revert_checkbox_toggled() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(
            state
                .toggle_checkbox("step-0", "step-0/dyn-cb-0", true)
                .unwrap(),
        ); // index 5

        let revert = ExecutionState::revert_event(&events, 5, "undo check").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert!(!state.steps["step-0"].content.iter().any(|item| {
            matches!(item, StepContent::Checkbox { id: Some(id), .. } if id == "step-0/dyn-cb-0")
        }));
    }

    #[test]
    fn test_revert_execution_completed() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(state.complete(CompletionStatus::Pass).unwrap()); // index 5

        let revert = ExecutionState::revert_event(&events, 5, "not done yet").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert_eq!(state.status, ExecutionStatus::Active);
    }

    #[test]
    fn test_revert_attachment_added() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(
            state
                .add_attachment(
                    "step-1",
                    "log-file",
                    "photo.jpg",
                    "path/photo.jpg",
                    "image/jpeg",
                    "abc123",
                )
                .unwrap(),
        ); // index 5

        let revert = ExecutionState::revert_event(&events, 5, "wrong file").unwrap();
        events.push(revert);

        let state = ExecutionState::from_events(&events).unwrap();
        assert!(!state.steps["step-1"].inputs.contains_key("log-file"));
    }

    #[test]
    fn test_cannot_revert_execution_started() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let events: Vec<Event> = state.start(&template).unwrap();

        let result = ExecutionState::revert_event(&events, 0, "nope");
        assert_eq!(result.unwrap_err(), ExecutionError::EventNotRevertible(0));
    }

    #[test]
    fn test_cannot_revert_step_added() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let events: Vec<Event> = state.start(&template).unwrap();

        let result = ExecutionState::revert_event(&events, 2, "nope");
        assert_eq!(result.unwrap_err(), ExecutionError::EventNotRevertible(2));
    }

    #[test]
    fn test_cannot_revert_already_reverted() {
        let mut events = events_with_skipped_step();
        let revert = ExecutionState::revert_event(&events, 5, "first").unwrap();
        events.push(revert);

        let result = ExecutionState::revert_event(&events, 5, "second");
        assert_eq!(result.unwrap_err(), ExecutionError::EventAlreadyReverted(5));
    }

    #[test]
    fn test_cannot_revert_out_of_range() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let events: Vec<Event> = state.start(&template).unwrap();

        let result = ExecutionState::revert_event(&events, 999, "nope");
        assert_eq!(
            result.unwrap_err(),
            ExecutionError::EventIndexOutOfRange(999)
        );
    }

    #[test]
    fn test_revert_serialization_roundtrip() {
        let mut events = events_with_skipped_step();
        let revert = ExecutionState::revert_event(&events, 5, "test reason").unwrap();
        events.push(revert.clone());

        let json = serde_json::to_string(&revert).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(revert, deserialized);

        let jsons: Vec<String> = events
            .iter()
            .map(|e| serde_json::to_string(e).unwrap())
            .collect();
        let deserialized_events: Vec<Event> = jsons
            .iter()
            .map(|j| serde_json::from_str(j).unwrap())
            .collect();
        let state = ExecutionState::from_events(&deserialized_events).unwrap();
        assert_eq!(state.steps["step-0"].status, StepStatus::Present);
    }

    #[test]
    fn test_from_events_with_interleaved_reverts() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(state.skip_step("step-0", "not needed").unwrap()); // index 5
        events.push(
            state
                .record_input("step-1", "current-draw", "120", Some("mA"))
                .unwrap(),
        ); // index 6

        let revert1 = ExecutionState::revert_event(&events, 5, "actually needed").unwrap();
        events.push(revert1);
        let revert2 = ExecutionState::revert_event(&events, 6, "wrong reading").unwrap();
        events.push(revert2);

        let rebuilt = ExecutionState::from_events(&events).unwrap();
        assert_eq!(rebuilt.steps["step-0"].status, StepStatus::Present);
        assert_eq!(rebuilt.steps["step-1"].status, StepStatus::Present);
        assert!(!rebuilt.steps["step-1"].inputs.contains_key("current-draw"));
    }
}
