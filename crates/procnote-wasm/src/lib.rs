//! procnote's execution logic for the browser (WebAssembly), so a browser can
//! record actions while offline with the same state machine as the server.
//!
//! The browser keeps an execution as its event-log lines and turns each
//! action into events here, with its own clock and IDs; the server later
//! validates the same events with the same code. Values cross the boundary
//! as JSON strings, in the shapes of the generated TypeScript types.

use procnote_app::action::ExecutionAction;
use procnote_app::attachment::sanitize_attachment_filename;
use procnote_app::execution::{ExecutionSummary, infer_attachment_content_type, summarize};
use procnote_app::repository::{StagedFile, decide_action};
use procnote_core::event::types::{AttachmentRecord, Event, EventRecord};
use procnote_core::execution::ExecutionState;
use procnote_core::template::types::ProcedureTemplate;
use wasm_bindgen::prelude::*;

/// Replay log lines (with or without the leading `LogMeta`).
pub fn replay(records: &[EventRecord]) -> Result<ExecutionState, String> {
    let events: Vec<Event> = records.iter().map(|record| record.event.clone()).collect();
    ExecutionState::from_events(&events).map_err(|e| e.to_string())
}

/// The summary the UI shows for an execution's log lines.
pub fn summary_of(records: &[EventRecord]) -> Result<ExecutionSummary, String> {
    summarize(&replay(records)?, None)
}

/// The events an action records on top of `records`. `files` are the
/// attachments an `add_attachment(s)` action adds (see [`attachment_record`]).
pub fn decide(
    records: &[EventRecord],
    action: ExecutionAction,
    files: Vec<AttachmentRecord>,
) -> Result<Vec<Event>, String> {
    let state = replay(records)?;
    let staged = files
        .into_iter()
        .map(|record| StagedFile {
            record,
            source: std::path::PathBuf::new(),
        })
        .collect();
    Ok(decide_action(&state, action, staged)?.events)
}

/// The events that start an execution of `template`, with a new execution ID.
pub fn start(template: &ProcedureTemplate) -> Result<Vec<Event>, String> {
    ExecutionState::new()
        .start(template)
        .map_err(|e| e.to_string())
}

/// How a file with this name and content is recorded, as the server stores
/// it: `attachments/{sha256_7}-{sanitized name}`.
#[must_use]
pub fn attachment_record(filename: &str, sha256: &str) -> AttachmentRecord {
    let filename = sanitize_attachment_filename(filename);
    AttachmentRecord {
        path: format!("attachments/{}-{filename}", &sha256[..7.min(sha256.len())]),
        content_type: infer_attachment_content_type(&filename),
        filename,
        sha256: sha256.to_string(),
    }
}

fn parse_records(records_json: &str) -> Result<Vec<EventRecord>, String> {
    let values: Vec<serde_json::Value> =
        serde_json::from_str(records_json).map_err(|e| format!("invalid records: {e}"))?;
    values
        .iter()
        .map(|value| EventRecord::from_value(value).map_err(|e| format!("invalid record: {e}")))
        .collect()
}

fn to_json(value: &impl serde::Serialize) -> Result<String, String> {
    serde_json::to_string(value).map_err(|e| e.to_string())
}

/// [`summary_of`] for JavaScript: log lines (JSON array) to an `ExecutionSummary` (JSON).
#[wasm_bindgen(js_name = summarize)]
pub fn summarize_js(records_json: &str) -> Result<String, JsError> {
    parse_records(records_json)
        .and_then(|records| summary_of(&records))
        .and_then(|summary| to_json(&summary))
        .map_err(|e| JsError::new(&e))
}

/// [`decide`] for JavaScript: log lines, an `ExecutionAction`, and
/// `AttachmentRecord`s (JSON) to the recorded events (JSON array).
#[wasm_bindgen(js_name = decide)]
pub fn decide_js(
    records_json: &str,
    action_json: &str,
    files_json: &str,
) -> Result<String, JsError> {
    let run = || {
        let records = parse_records(records_json)?;
        let action: ExecutionAction =
            serde_json::from_str(action_json).map_err(|e| format!("invalid action: {e}"))?;
        let files: Vec<AttachmentRecord> =
            serde_json::from_str(files_json).map_err(|e| format!("invalid files: {e}"))?;
        to_json(&decide(&records, action, files)?)
    };
    run().map_err(|e| JsError::new(&e))
}

/// [`start`] for JavaScript: a `ProcedureTemplate` (JSON) to the starting events (JSON array).
#[wasm_bindgen(js_name = start)]
pub fn start_js(template_json: &str) -> Result<String, JsError> {
    serde_json::from_str::<ProcedureTemplate>(template_json)
        .map_err(|e| format!("invalid template: {e}"))
        .and_then(|template| to_json(&start(&template)?))
        .map_err(|e| JsError::new(&e))
}

/// [`attachment_record`] for JavaScript, as JSON.
#[wasm_bindgen(js_name = attachmentRecord)]
pub fn attachment_record_js(filename: &str, sha256: &str) -> Result<String, JsError> {
    to_json(&attachment_record(filename, sha256)).map_err(|e| JsError::new(&e))
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_core::template::parse_template;

    const TEMPLATE: &str = "---\nid: W\ntitle: Wasm\nversion: '1'\n---\n\n## Step\n\n- [ ] Ready\n\n```inputs\n- id: photo\n  label: Photo\n  type: attachment\n```\n";

    fn records(events: Vec<Event>) -> Vec<EventRecord> {
        events.into_iter().map(EventRecord::from).collect()
    }

    #[test]
    fn an_execution_runs_from_start_to_summary() {
        let mut log = records(start(&parse_template(TEMPLATE).unwrap()).unwrap());
        log.extend(records(
            decide(
                &log,
                ExecutionAction::ToggleCheckbox {
                    step_id: "step-0".to_string(),
                    checkbox_id: "step-0/cb-0".to_string(),
                    checked: true,
                },
                Vec::new(),
            )
            .unwrap(),
        ));
        log.extend(records(
            decide(
                &log,
                ExecutionAction::AddAttachments {
                    step_id: "step-0".to_string(),
                    input_id: "photo".to_string(),
                    files: Vec::new(),
                },
                vec![attachment_record("photo 1.jpg", &"ab".repeat(32))],
            )
            .unwrap(),
        ));

        let summary = serde_json::to_value(summary_of(&log).unwrap()).unwrap();

        assert_eq!(summary["procedure_id"], "W");
        assert_eq!(summary["steps"][0]["content"][0]["checked"], true);
        assert_eq!(
            summary["steps"][0]["content"][1]["inputs"][0]["attachments"][0]["path"],
            "attachments/abababa-photo 1.jpg"
        );
        assert_eq!(summary["execution_dir"], serde_json::Value::Null);
    }

    #[test]
    fn invalid_actions_are_refused() {
        let log = records(start(&parse_template(TEMPLATE).unwrap()).unwrap());

        let skipped = decide(
            &log,
            ExecutionAction::SkipStep {
                step_id: "missing".to_string(),
                reason: "x".to_string(),
            },
            Vec::new(),
        );

        assert!(skipped.is_err());
    }

    #[test]
    fn log_lines_are_read_as_the_server_writes_them() {
        let log = records(start(&parse_template(TEMPLATE).unwrap()).unwrap());
        let json = serde_json::to_string(&log).unwrap();

        assert_eq!(parse_records(&json).unwrap(), log);
        assert_eq!(
            attachment_record("../a:b.png", "0123456789").content_type,
            "image/png"
        );
    }
}
