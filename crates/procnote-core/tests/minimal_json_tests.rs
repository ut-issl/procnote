//! Round-trip tests for minimal JSON per event variant.
//!
//! Each test deserializes an event from a "minimal" JSON string (only required
//! fields, no optional fields). This catches missing `#[serde(default)]`
//! annotations on new optional fields — if a new field is added without

#![expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
//! `default`, these tests will fail because old logs won't contain it.

use procnote_core::event::types::Event;

/// Parse a JSON string as an Event, then re-serialize and re-parse to verify
/// round-trip stability.
fn round_trip(json: &str) -> Event {
    let event: Event = serde_json::from_str(json)
        .unwrap_or_else(|e| panic!("Failed to parse JSON: {e}\nInput: {json}"));
    let reserialized = serde_json::to_string(&event).unwrap();
    let reparsed: Event = serde_json::from_str(&reserialized)
        .unwrap_or_else(|e| panic!("Failed round-trip: {e}\nReserialized: {reserialized}"));
    assert_eq!(event, reparsed);
    event
}

#[test]
fn minimal_execution_started() {
    round_trip(
        r#"{"type":"execution_started","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","procedure_id":"P-001","procedure_title":"Test","procedure_version":"1.0"}"#,
    );
}

#[test]
fn minimal_execution_completed() {
    round_trip(
        r#"{"type":"execution_completed","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","status":"pass"}"#,
    );
}

#[test]
fn minimal_execution_aborted() {
    round_trip(
        r#"{"type":"execution_aborted","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","reason":"power failure"}"#,
    );
}

#[test]
fn minimal_step_added_no_optional_fields() {
    // StepAdded without content (empty default) and without after_step_id.
    round_trip(
        r#"{"type":"step_added","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","heading":"Step 1"}"#,
    );
}

#[test]
fn minimal_step_skipped() {
    round_trip(
        r#"{"type":"step_skipped","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","reason":"N/A"}"#,
    );
}

#[test]
fn minimal_checkbox_toggled() {
    round_trip(
        r#"{"type":"checkbox_toggled","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","checkbox_id":"s1/cb0","checked":true}"#,
    );
}

#[test]
fn minimal_input_recorded_no_unit() {
    // InputRecorded without the optional unit field.
    round_trip(
        r#"{"type":"input_recorded","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","input_id":"s1/v","value":"42"}"#,
    );
}

#[test]
fn minimal_note_added_no_step() {
    // NoteAdded without the optional step_id (global note).
    round_trip(
        r#"{"type":"note_added","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","text":"observation"}"#,
    );
}

#[test]
fn minimal_attachment_added() {
    round_trip(
        r#"{"type":"attachment_added","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","input_id":"s1/f","filename":"data.csv","path":"attachments/abc-data.csv","content_type":"text/csv","sha256":"deadbeef"}"#,
    );
}

#[test]
fn minimal_execution_renamed() {
    round_trip(
        r#"{"type":"execution_renamed","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","name":"Run A"}"#,
    );
}

#[test]
fn minimal_event_reverted() {
    round_trip(
        r#"{"type":"event_reverted","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","reverted_event_index":3,"reason":"mistake"}"#,
    );
}

#[test]
fn minimal_log_meta() {
    round_trip(
        r#"{"type":"log_meta","at":"2025-01-01T00:00:00Z","version":1,"tool_version":"0.1.0"}"#,
    );
}

#[test]
fn extra_fields_on_known_event_are_ignored() {
    // Verify that extra/unknown fields on a known event type don't cause errors.
    // This is important for forward compatibility: new fields added to existing
    // events must not break old code.
    let json = r#"{"type":"step_skipped","at":"2025-01-01T00:00:00Z","execution_id":"550e8400-e29b-41d4-a716-446655440000","step_id":"s1","reason":"N/A","new_future_field":"hello","another_one":42}"#;
    let event: Event = serde_json::from_str(json).unwrap();
    assert!(matches!(event, Event::StepSkipped { .. }));
}
