//! Snapshot fixture tests for backward compatibility of the event log format.
//!
//! These tests deserialize committed JSONL fixture files and assert the resulting
//! `ExecutionState` matches expectations. If a field is renamed, a type changes,
//! or a variant is removed, these tests break immediately.

#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]

use std::path::Path;

use procnote_core::event::read_log;
use procnote_core::execution::{ExecutionState, ExecutionStatus, StepStatus};

fn fixture_path(name: &str) -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures")
        .join(name)
}

#[test]
fn v2_basic_execution_parses_to_finished_pass() {
    let events = read_log(&fixture_path("v2_basic_execution.jsonl")).unwrap();
    let state = ExecutionState::from_events(&events).unwrap();

    assert!(matches!(
        state.status,
        ExecutionStatus::Finished(procnote_core::event::types::CompletionStatus::Pass)
    ));
    assert_eq!(state.step_order.len(), 3);
    assert_eq!(state.steps["step-0"].status, StepStatus::Present);
    assert_eq!(state.steps["step-1"].status, StepStatus::Present);
    assert_eq!(state.steps["step-2"].status, StepStatus::Present);
    // Verify input was recorded
    assert_eq!(state.steps["step-1"].inputs["step-1/temp"].value, "-39.5");
}

#[test]
fn v2_with_reversal_actions_applies_correction() {
    let events = read_log(&fixture_path("v2_with_reversal_actions.jsonl")).unwrap();
    let state = ExecutionState::from_events(&events).unwrap();

    assert!(matches!(
        state.status,
        ExecutionStatus::Finished(procnote_core::event::types::CompletionStatus::Pass)
    ));
    // The cleared input (999) should not appear; the corrected value (-39.5) should.
    assert_eq!(state.steps["step-1"].inputs["step-1/temp"].value, "-39.5");
}

#[test]
fn v2_all_event_types_parses_successfully() {
    let events = read_log(&fixture_path("v2_all_event_types.jsonl")).unwrap();
    let state = ExecutionState::from_events(&events).unwrap();

    assert!(matches!(
        state.status,
        ExecutionStatus::Finished(procnote_core::event::types::CompletionStatus::Pass)
    ));
    assert_eq!(state.name.as_deref(), Some("Morning run"));
    assert_eq!(state.step_order.len(), 2);
    // step-1 was skipped then unskipped, so it ended up present
    assert_eq!(state.steps["step-1"].status, StepStatus::Present);
    // Global note was recorded
    assert_eq!(state.global_notes.len(), 1);
    assert_eq!(state.global_notes[0].text, "Global observation");
    // Step note was recorded
    assert_eq!(state.steps["step-0"].notes.len(), 1);
}
