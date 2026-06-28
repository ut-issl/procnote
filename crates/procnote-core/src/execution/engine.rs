use std::collections::HashMap;

use chrono::Utc;
use uuid::Uuid;

use crate::event::types::{CompletionStatus, Event, ExecutionId};
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
    #[error("execution is not finished")]
    NotFinished,
    #[error("step already skipped: {0}")]
    StepAlreadySkipped(String),
    #[error("step is not skipped: {0}")]
    StepNotSkipped(String),
    #[error("step has captured data: {0}")]
    StepHasCapturedData(String),
    #[error("duplicate step id: {0}")]
    DuplicateStepId(String),
    #[error("duplicate note id: {0}")]
    DuplicateNoteId(String),
    #[error("checkbox not found: {0}")]
    CheckboxNotFound(String),
    #[error("input already recorded: {0}")]
    InputAlreadyRecorded(String),
    #[error("input not recorded: {0}")]
    InputNotRecorded(String),
    #[error("attachment not found: {0}")]
    AttachmentNotFound(String),
    #[error("note not found: {0}")]
    NoteNotFound(String),
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
    pub notes: Vec<RecordedNote>,
}

/// A recorded input value.
#[derive(Debug, Clone)]
pub struct RecordedInput {
    pub label: String,
    pub value: String,
    pub unit: Option<String>,
}

/// A recorded note.
#[derive(Debug, Clone)]
pub struct RecordedNote {
    pub id: String,
    pub text: String,
}

/// The full state of a procedure execution, reconstructable from events.
#[derive(Debug, Clone)]
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
    pub global_notes: Vec<RecordedNote>,
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

    /// Reconstruct execution state by replaying a sequence of events.
    ///
    /// `LogMeta` events are replay metadata and are skipped by the state machine.
    pub fn from_events(events: &[Event]) -> Result<Self, ExecutionError> {
        events.iter().try_fold(Self::new(), |mut state, event| {
            match event {
                Event::LogMeta { .. } => {}
                _ => state.apply(event)?,
            }
            Ok(state)
        })
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
            Event::ExecutionReopened { .. } => {
                self.require_finished()?;
                self.status = ExecutionStatus::Active;
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
                let step = self.get_present_step_mut(step_id)?;
                if step.has_captured_data() {
                    return Err(ExecutionError::StepHasCapturedData(step_id.clone()));
                }
                step.status = StepStatus::Skipped;
            }
            Event::StepUnskipped { step_id, .. } => {
                self.require_active()?;
                let step = self.get_step_mut(step_id)?;
                match step.status {
                    StepStatus::Present => Err(ExecutionError::StepNotSkipped(step_id.clone())),
                    StepStatus::Skipped => {
                        step.status = StepStatus::Present;
                        Ok(())
                    }
                }?;
            }
            Event::CheckboxToggled {
                step_id,
                checkbox_id,
                checked,
                ..
            } => {
                self.require_active()?;
                let step = self.get_present_step_mut(step_id)?;
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
                    return Err(ExecutionError::CheckboxNotFound(checkbox_id.clone()));
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
                let step = self.get_present_step_mut(step_id)?;
                if step.inputs.contains_key(input_id) {
                    return Err(ExecutionError::InputAlreadyRecorded(input_id.clone()));
                }
                step.inputs.insert(
                    input_id.clone(),
                    RecordedInput {
                        label: input_id.clone(),
                        value: value.clone(),
                        unit: unit.clone(),
                    },
                );
            }
            Event::InputCleared {
                step_id, input_id, ..
            } => {
                self.require_active()?;
                let step = self.get_present_step_mut(step_id)?;
                step.inputs
                    .remove(input_id)
                    .map(|_| ())
                    .ok_or_else(|| ExecutionError::InputNotRecorded(input_id.clone()))?;
            }
            Event::NoteAdded {
                note_id,
                text,
                step_id,
                ..
            } => {
                self.require_active()?;
                if self.note_exists(note_id) {
                    return Err(ExecutionError::DuplicateNoteId(note_id.clone()));
                }
                let note = RecordedNote {
                    id: note_id.clone(),
                    text: text.clone(),
                };
                match step_id {
                    Some(id) => {
                        let step = self.get_present_step_mut(id)?;
                        step.notes.push(note);
                    }
                    None => {
                        self.global_notes.push(note);
                    }
                }
            }
            Event::NoteRemoved { note_id, .. } => {
                self.require_active()?;
                self.remove_note(note_id)?;
            }

            Event::AttachmentAdded {
                step_id,
                input_id,
                filename,
                ..
            } => {
                self.require_active()?;
                let step = self.get_present_step_mut(step_id)?;
                if step.inputs.contains_key(input_id) {
                    return Err(ExecutionError::InputAlreadyRecorded(input_id.clone()));
                }
                step.inputs.insert(
                    input_id.clone(),
                    RecordedInput {
                        label: input_id.clone(),
                        value: filename.clone(),
                        unit: None,
                    },
                );
            }
            Event::AttachmentRemoved {
                step_id, input_id, ..
            } => {
                self.require_active()?;
                let step = self.get_present_step_mut(step_id)?;
                step.inputs
                    .remove(input_id)
                    .map(|_| ())
                    .ok_or_else(|| ExecutionError::AttachmentNotFound(input_id.clone()))?;
            }

            Event::ExecutionRenamed { name, .. } => {
                if self.execution_id.is_none() {
                    return Err(ExecutionError::NotStarted);
                }
                self.name = Some(name.clone());
            }

            Event::LogMeta { .. } => {}
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

    /// Build and validate a rename event without mutating this state.
    ///
    /// Unlike most actions, this works on both active and finished executions
    /// (it's metadata, not a state transition).
    pub fn rename_event(&self, name: &str) -> Result<Event, ExecutionError> {
        self.validated_candidate(Event::ExecutionRenamed {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            name: name.to_string(),
        })
    }

    /// Rename the execution.
    pub fn rename(&mut self, name: &str) -> Result<Event, ExecutionError> {
        let event = self.rename_event(name)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a new step event without mutating this state.
    pub fn add_step_event(
        &self,
        step_id: &str,
        heading: &str,
        content: Vec<StepContent>,
        after_step_id: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::StepAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            heading: heading.to_string(),
            content,
            after_step_id: after_step_id.map(std::string::ToString::to_string),
        })
    }

    /// Add a new step during execution.
    pub fn add_step(
        &mut self,
        step_id: &str,
        heading: &str,
        content: Vec<StepContent>,
        after_step_id: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        let event = self.add_step_event(step_id, heading, content, after_step_id)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a step-skip event without mutating this state.
    pub fn skip_step_event(&self, step_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::StepSkipped {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Skip a step.
    pub fn skip_step(&mut self, step_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        let event = self.skip_step_event(step_id, reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a step-unskip event without mutating this state.
    pub fn unskip_step_event(&self, step_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::StepUnskipped {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Unskip a step.
    pub fn unskip_step(&mut self, step_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        let event = self.unskip_step_event(step_id, reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a checkbox-toggle event without mutating this state.
    pub fn toggle_checkbox_event(
        &self,
        step_id: &str,
        checkbox_id: &str,
        checked: bool,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::CheckboxToggled {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            checkbox_id: checkbox_id.to_string(),
            checked,
        })
    }

    /// Toggle a checkbox in a step.
    pub fn toggle_checkbox(
        &mut self,
        step_id: &str,
        checkbox_id: &str,
        checked: bool,
    ) -> Result<Event, ExecutionError> {
        let event = self.toggle_checkbox_event(step_id, checkbox_id, checked)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an input-recorded event without mutating this state.
    pub fn record_input_event(
        &self,
        step_id: &str,
        input_id: &str,
        value: &str,
        unit: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::InputRecorded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            value: value.to_string(),
            unit: unit.map(std::string::ToString::to_string),
        })
    }

    /// Record an input value.
    pub fn record_input(
        &mut self,
        step_id: &str,
        input_id: &str,
        value: &str,
        unit: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        let event = self.record_input_event(step_id, input_id, value, unit)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an input-cleared event without mutating this state.
    pub fn clear_input_event(
        &self,
        step_id: &str,
        input_id: &str,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::InputCleared {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Clear an input value.
    pub fn clear_input(
        &mut self,
        step_id: &str,
        input_id: &str,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        let event = self.clear_input_event(step_id, input_id, reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a note-added event without mutating this state.
    pub fn add_note_event(
        &self,
        text: &str,
        step_id: Option<&str>,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::NoteAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            note_id: Uuid::new_v4().to_string(),
            text: text.to_string(),
            step_id: step_id.map(std::string::ToString::to_string),
        })
    }

    /// Add a note.
    pub fn add_note(&mut self, text: &str, step_id: Option<&str>) -> Result<Event, ExecutionError> {
        let event = self.add_note_event(text, step_id)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate a note-removed event without mutating this state.
    pub fn remove_note_event(&self, note_id: &str, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::NoteRemoved {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            note_id: note_id.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Remove a note.
    pub fn remove_note_action(
        &mut self,
        note_id: &str,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        let event = self.remove_note_event(note_id, reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an attachment-added event without mutating this state.
    pub fn add_attachment_event(
        &self,
        step_id: &str,
        input_id: &str,
        filename: &str,
        path: &str,
        content_type: &str,
        sha256: &str,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::AttachmentAdded {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            filename: filename.to_string(),
            path: path.to_string(),
            content_type: content_type.to_string(),
            sha256: sha256.to_string(),
        })
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
        let event =
            self.add_attachment_event(step_id, input_id, filename, path, content_type, sha256)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an attachment-removed event without mutating this state.
    pub fn remove_attachment_event(
        &self,
        step_id: &str,
        input_id: &str,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::AttachmentRemoved {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            step_id: step_id.to_string(),
            input_id: input_id.to_string(),
            reason: reason.to_string(),
        })
    }

    /// Remove an attachment.
    pub fn remove_attachment(
        &mut self,
        step_id: &str,
        input_id: &str,
        reason: &str,
    ) -> Result<Event, ExecutionError> {
        let event = self.remove_attachment_event(step_id, input_id, reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an execution-completed event without mutating this state.
    pub fn complete_event(&self, status: CompletionStatus) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::ExecutionCompleted {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            status,
        })
    }

    /// Complete the execution.
    pub fn complete(&mut self, status: CompletionStatus) -> Result<Event, ExecutionError> {
        let event = self.complete_event(status)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an execution-aborted event without mutating this state.
    pub fn abort_event(&self, reason: &str) -> Result<Event, ExecutionError> {
        self.require_active()?;
        self.validated_candidate(Event::ExecutionAborted {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            reason: reason.to_string(),
        })
    }

    /// Abort the execution.
    pub fn abort(&mut self, reason: &str) -> Result<Event, ExecutionError> {
        let event = self.abort_event(reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    /// Build and validate an execution-reopened event without mutating this state.
    pub fn reopen_event(&self, reason: &str) -> Result<Event, ExecutionError> {
        self.require_finished()?;
        self.validated_candidate(Event::ExecutionReopened {
            at: Utc::now(),
            execution_id: self.require_execution_id()?,
            reason: reason.to_string(),
        })
    }

    /// Reopen the execution.
    pub fn reopen(&mut self, reason: &str) -> Result<Event, ExecutionError> {
        let event = self.reopen_event(reason)?;
        self.apply(&event)?;
        Ok(event)
    }

    // -- Helpers --

    const fn require_active(&self) -> Result<(), ExecutionError> {
        match &self.status {
            ExecutionStatus::Pending => Err(ExecutionError::NotStarted),
            ExecutionStatus::Active => Ok(()),
            ExecutionStatus::Finished(_) => Err(ExecutionError::AlreadyFinished),
        }
    }

    const fn require_finished(&self) -> Result<(), ExecutionError> {
        match &self.status {
            ExecutionStatus::Pending => Err(ExecutionError::NotStarted),
            ExecutionStatus::Active => Err(ExecutionError::NotFinished),
            ExecutionStatus::Finished(_) => Ok(()),
        }
    }

    fn require_execution_id(&self) -> Result<ExecutionId, ExecutionError> {
        self.execution_id.ok_or(ExecutionError::NotStarted)
    }

    fn validated_candidate(&self, event: Event) -> Result<Event, ExecutionError> {
        let mut trial = self.clone();
        trial.apply(&event)?;
        Ok(event)
    }

    fn get_step_mut(&mut self, step_id: &str) -> Result<&mut StepState, ExecutionError> {
        self.steps
            .get_mut(step_id)
            .ok_or_else(|| ExecutionError::StepNotFound(step_id.to_string()))
    }

    fn get_present_step_mut(&mut self, step_id: &str) -> Result<&mut StepState, ExecutionError> {
        let step = self.get_step_mut(step_id)?;
        match step.status {
            StepStatus::Present => Ok(step),
            StepStatus::Skipped => Err(ExecutionError::StepAlreadySkipped(step_id.to_string())),
        }
    }

    fn note_exists(&self, note_id: &str) -> bool {
        self.global_notes.iter().any(|note| note.id == note_id)
            || self
                .steps
                .values()
                .any(|step| step.notes.iter().any(|note| note.id == note_id))
    }

    fn remove_note(&mut self, note_id: &str) -> Result<(), ExecutionError> {
        if let Some(index) = self
            .global_notes
            .iter()
            .position(|note| note.id.as_str() == note_id)
        {
            self.global_notes.remove(index);
            return Ok(());
        }

        self.steps
            .values_mut()
            .find_map(|step| {
                if step.status == StepStatus::Skipped {
                    return None;
                }
                step.notes
                    .iter()
                    .position(|note| note.id.as_str() == note_id)
                    .map(|index| (step, index))
            })
            .map(|(step, index)| {
                step.notes.remove(index);
            })
            .ok_or_else(|| ExecutionError::NoteNotFound(note_id.to_string()))
    }
}

impl StepState {
    fn has_captured_data(&self) -> bool {
        !self.inputs.is_empty()
            || !self.notes.is_empty()
            || self
                .content
                .iter()
                .any(|item| matches!(item, StepContent::Checkbox { checked: true, .. }))
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
                    content: vec![StepContent::Checkbox {
                        id: None,
                        text: "Ready".to_string(),
                        checked: false,
                    }],
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
        assert_eq!(state.global_notes[0].text, "General observation");
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

    // -- Reversal action tests --

    #[test]
    fn test_unskip_step() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.skip_step("step-1", "N/A").unwrap();
        state.unskip_step("step-1", "actually needed").unwrap();

        assert_eq!(state.steps["step-1"].status, StepStatus::Present);
    }

    #[test]
    fn test_clear_input_allows_recording_again() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state
            .record_input("step-1", "voltage", "5.0", Some("V"))
            .unwrap();

        let duplicate = state.record_input("step-1", "voltage", "5.1", Some("V"));
        assert_eq!(
            duplicate.unwrap_err(),
            ExecutionError::InputAlreadyRecorded("voltage".to_string())
        );

        state
            .clear_input("step-1", "voltage", "wrong value")
            .unwrap();
        state
            .record_input("step-1", "voltage", "5.1", Some("V"))
            .unwrap();

        assert_eq!(state.steps["step-1"].inputs["voltage"].value, "5.1");
    }

    #[test]
    fn test_remove_note() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        let note = state.add_note("oops", Some("step-1")).unwrap();
        let Event::NoteAdded { note_id, .. } = note else {
            unreachable!();
        };

        state.remove_note_action(&note_id, "typo").unwrap();

        assert!(state.steps["step-1"].notes.is_empty());
    }

    #[test]
    fn test_remove_attachment() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state
            .add_attachment(
                "step-1",
                "log-file",
                "photo.jpg",
                "path/photo.jpg",
                "image/jpeg",
                "abc123",
            )
            .unwrap();

        state
            .remove_attachment("step-1", "log-file", "wrong file")
            .unwrap();

        assert!(!state.steps["step-1"].inputs.contains_key("log-file"));
    }

    #[test]
    fn test_toggle_checkbox_back() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state
            .toggle_checkbox("step-0", "step-0/cb-0", true)
            .unwrap();
        state
            .toggle_checkbox("step-0", "step-0/cb-0", false)
            .unwrap();

        assert!(state.steps["step-0"].content.iter().any(|item| {
            matches!(item, StepContent::Checkbox { id: Some(id), checked, .. } if id == "step-0/cb-0" && !*checked)
        }));
    }

    #[test]
    fn test_reopen_execution() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.complete(CompletionStatus::Pass).unwrap();
        state.reopen("not done yet").unwrap();

        assert_eq!(state.status, ExecutionStatus::Active);
    }

    #[test]
    fn test_cannot_record_data_on_skipped_step() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state.skip_step("step-1", "N/A").unwrap();

        let result = state.record_input("step-1", "voltage", "5.0", Some("V"));
        assert_eq!(
            result.unwrap_err(),
            ExecutionError::StepAlreadySkipped("step-1".to_string())
        );
    }

    #[test]
    fn test_cannot_skip_step_with_captured_data() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        state.start(&template).unwrap();
        state
            .record_input("step-1", "voltage", "5.0", Some("V"))
            .unwrap();

        let result = state.skip_step("step-1", "N/A");
        assert_eq!(
            result.unwrap_err(),
            ExecutionError::StepHasCapturedData("step-1".to_string())
        );
    }

    #[test]
    fn test_reversal_serialization_roundtrip() {
        let template = sample_template();
        let mut state = ExecutionState::new();
        let mut events: Vec<Event> = Vec::new();
        events.extend(state.start(&template).unwrap());
        events.push(state.skip_step("step-1", "not needed").unwrap());
        events.push(state.unskip_step("step-1", "actually needed").unwrap());

        let jsons: Vec<String> = events
            .iter()
            .map(|event| serde_json::to_string(event).unwrap())
            .collect();
        let deserialized_events: Vec<Event> = jsons
            .iter()
            .map(|json| serde_json::from_str(json).unwrap())
            .collect();
        let rebuilt = ExecutionState::from_events(&deserialized_events).unwrap();
        assert_eq!(rebuilt.steps["step-1"].status, StepStatus::Present);
    }
}
