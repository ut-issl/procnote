use std::io::{BufRead, Write};
use std::path::Path;

use super::types::Event;

/// The current supported schema version for event logs.
pub const SUPPORTED_VERSION: u32 = 2;

/// Errors that can occur during event log operations.
#[derive(Debug, thiserror::Error)]
pub enum EventLogError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON serialization error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("event log is missing the required LogMeta first line")]
    MissingLogMeta,
    #[error(
        "unsupported event log version {found} (this version of procnote supports version {supported})"
    )]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error(
        "unknown event type {type_name:?} at line {line} (within a supported schema version, this is a bug)"
    )]
    UnknownEventType { type_name: String, line: usize },
    #[error("corrupt data at line {line} in event log (not valid JSON)")]
    CorruptLine { line: usize },
}

/// Append a single event to a JSONL file.
///
/// Creates the file (and parent directories) if it does not exist.
pub fn append_event(path: &Path, event: &Event) -> Result<(), EventLogError> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)?;
    let json = serde_json::to_string(event)?;
    writeln!(file, "{json}")?;
    Ok(())
}

/// Read all events from a JSONL event log.
///
/// Validates that:
/// - The first line is a [`Event::LogMeta`] with a supported version.
/// - All subsequent lines are known event types.
/// - Invalid JSON is only tolerated at the tail of the file (truncated write
///   from a crash); mid-file corruption is an error.
pub fn read_log(path: &Path) -> Result<Vec<Event>, EventLogError> {
    let file = std::fs::File::open(path)?;
    let reader = std::io::BufReader::new(file);
    let lines: Vec<String> = reader.lines().collect::<Result<_, _>>()?;
    let total_lines = lines.len();

    // Find the first non-empty line — it must be LogMeta.
    let first_content_idx = lines
        .iter()
        .position(|line| !line.trim().is_empty())
        .ok_or(EventLogError::MissingLogMeta)?;

    let first_line = lines[first_content_idx].trim();
    let first_event: Event =
        serde_json::from_str(first_line).map_err(|_| EventLogError::MissingLogMeta)?;

    match &first_event {
        Event::LogMeta { version, .. } => {
            if *version != SUPPORTED_VERSION {
                return Err(EventLogError::UnsupportedVersion {
                    found: *version,
                    supported: SUPPORTED_VERSION,
                });
            }
        }
        _ => return Err(EventLogError::MissingLogMeta),
    }

    let mut events = vec![first_event];

    for (line_idx, line) in lines.iter().enumerate().skip(first_content_idx + 1) {
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        if let Ok(event) = serde_json::from_str::<Event>(trimmed) {
            events.push(event);
        } else if let Ok(value) = serde_json::from_str::<serde_json::Value>(trimmed) {
            // Valid JSON but unknown event type.
            let type_name = value
                .get("type")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("<no type>")
                .to_string();
            return Err(EventLogError::UnknownEventType {
                type_name,
                line: line_idx + 1,
            });
        } else if line_idx + 1 == total_lines {
            // Tolerate truncated write at the tail.
            let preview: String = trimmed.chars().take(100).collect();
            log::warn!("Skipping truncated line at end of event log: {preview}");
        } else {
            return Err(EventLogError::CorruptLine { line: line_idx + 1 });
        }
    }
    Ok(events)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use crate::event::types::{AttachmentRecord, CompletionStatus, ExecutionId};
    use crate::template::types::StepContent;
    use chrono::Utc;
    use uuid::Uuid;

    fn sample_execution_id() -> ExecutionId {
        Uuid::parse_str("550e8400-e29b-41d4-a716-446655440000").unwrap()
    }

    fn log_meta() -> Event {
        Event::LogMeta {
            at: Utc::now(),
            version: SUPPORTED_VERSION,
            tool_version: "0.1.0".to_string(),
        }
    }

    fn sample_events() -> Vec<Event> {
        let id = sample_execution_id();
        let now = Utc::now();
        vec![
            Event::ExecutionStarted {
                at: now,
                execution_id: id,
                procedure_id: "TVT-001".to_string(),
                procedure_title: "Thermal Vacuum Test".to_string(),
                procedure_version: "1.0".to_string(),
            },
            Event::CheckboxToggled {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                checkbox_id: "step-0/cb-0".to_string(),
                checked: true,
            },
            Event::ExecutionCompleted {
                at: now,
                execution_id: id,
                status: CompletionStatus::Pass,
            },
        ]
    }

    /// Write a `LogMeta` line followed by events to a file.
    fn write_log_with_meta(path: &std::path::Path, events: &[Event]) {
        append_event(path, &log_meta()).unwrap();
        for event in events {
            append_event(path, event).unwrap();
        }
    }

    #[test]
    fn test_round_trip_single_event() {
        let event = &sample_events()[0];
        let json = serde_json::to_string(event).unwrap();
        let deserialized: Event = serde_json::from_str(&json).unwrap();
        assert_eq!(*event, deserialized);
    }

    #[test]
    fn test_append_and_read() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        let events = sample_events();
        write_log_with_meta(&path, &events);

        let read_events = read_log(&path).unwrap();
        // First event is LogMeta, then the sample events.
        assert_eq!(read_events.len(), events.len() + 1);
        assert!(matches!(&read_events[0], Event::LogMeta { .. }));
        for (original, read) in events.iter().zip(read_events[1..].iter()) {
            assert_eq!(original, read);
        }
    }

    #[test]
    fn test_tail_truncation_tolerated() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        let events = sample_events();
        write_log_with_meta(&path, &[events[0].clone()]);

        // Append a corrupt line at the tail (simulating truncated write).
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{{corrupt json line").unwrap();
        drop(file);

        // Tail truncation is tolerated.
        let result = read_log(&path).unwrap();
        assert_eq!(result.len(), 2); // LogMeta + 1 event
    }

    #[test]
    fn test_mid_file_corruption_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        let events = sample_events();
        write_log_with_meta(&path, &[events[0].clone()]);

        // Append a corrupt line in the middle, then a valid event.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file, "{{corrupt json line").unwrap();
        drop(file);
        append_event(&path, &events[1]).unwrap();

        let result = read_log(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("corrupt"),
            "expected corrupt error, got: {err}"
        );
    }

    #[test]
    fn test_unknown_event_type_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        let events = sample_events();
        write_log_with_meta(&path, &[events[0].clone()]);

        // Append an unknown event type (valid JSON, unrecognized "type").
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(
            file,
            r#"{{"type":"future_event","at":"2025-01-01T00:00:00Z","data":"hello"}}"#
        )
        .unwrap();
        drop(file);

        let result = read_log(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("future_event"),
            "expected unknown event error, got: {err}"
        );
    }

    #[test]
    fn test_missing_log_meta_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        // Write events without LogMeta first line.
        let events = sample_events();
        append_event(&path, &events[0]).unwrap();

        let result = read_log(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("LogMeta"),
            "expected missing LogMeta error, got: {err}"
        );
    }

    #[test]
    fn test_unsupported_version_errors() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        // Write LogMeta with version 99.
        let meta = Event::LogMeta {
            at: Utc::now(),
            version: 99,
            tool_version: "99.0.0".to_string(),
        };
        append_event(&path, &meta).unwrap();

        let result = read_log(&path);
        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(
            err.contains("unsupported"),
            "expected unsupported version error, got: {err}"
        );
    }

    #[test]
    fn test_empty_lines_skipped() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("events.jsonl");

        let events = sample_events();
        write_log_with_meta(&path, &[events[0].clone()]);

        // Append empty lines.
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(file).unwrap();
        writeln!(file, "   ").unwrap();
        drop(file);

        append_event(&path, &events[1]).unwrap();

        let result = read_log(&path).unwrap();
        assert_eq!(result.len(), 3); // LogMeta + 2 events
    }

    #[test]
    fn test_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("events.jsonl");

        let events = sample_events();
        append_event(&path, &events[0]).unwrap();

        assert!(path.exists());
    }

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "round-trip fixture intentionally enumerates all event variants"
    )]
    fn test_all_event_types_serialize() {
        let id = sample_execution_id();
        let now = Utc::now();

        let all_events = vec![
            Event::ExecutionStarted {
                at: now,
                execution_id: id,
                procedure_id: "P-001".to_string(),
                procedure_title: "Procedure 001".to_string(),
                procedure_version: "1.0".to_string(),
            },
            Event::ExecutionCompleted {
                at: now,
                execution_id: id,
                status: CompletionStatus::Pass,
            },
            Event::ExecutionAborted {
                at: now,
                execution_id: id,
                reason: "Power failure".to_string(),
            },
            Event::ExecutionReopened {
                at: now,
                execution_id: id,
                reason: "Need more work".to_string(),
            },
            Event::StepAdded {
                at: now,
                execution_id: id,
                step_id: "dyn-step-1".to_string(),
                heading: "New Step".to_string(),
                content: vec![StepContent::Prose {
                    text: "Added during execution".to_string(),
                }],
                after_step_id: Some("step-0".to_string()),
            },
            Event::StepSkipped {
                at: now,
                execution_id: id,
                step_id: "step-1".to_string(),
                reason: "Not applicable".to_string(),
            },
            Event::StepUnskipped {
                at: now,
                execution_id: id,
                step_id: "step-1".to_string(),
                reason: "Actually needed".to_string(),
            },
            Event::CheckboxToggled {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                checkbox_id: "step-0/cb-0".to_string(),
                checked: true,
            },
            Event::InputRecorded {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "current-draw".to_string(),
                value: "120".to_string(),
                unit: Some("mA".to_string()),
            },
            Event::InputCleared {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "current-draw".to_string(),
                reason: "Wrong value".to_string(),
            },
            Event::NoteAdded {
                at: now,
                execution_id: id,
                note_id: "note-1".to_string(),
                text: "Observation noted".to_string(),
                step_id: Some("step-0".to_string()),
            },
            Event::NoteRemoved {
                at: now,
                execution_id: id,
                note_id: "note-1".to_string(),
                reason: "Typo".to_string(),
            },
            Event::AttachmentAdded {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "log-file".to_string(),
                filename: "photo.jpg".to_string(),
                path: "attachments/photo.jpg".to_string(),
                content_type: "image/jpeg".to_string(),
                sha256: "abc123".to_string(),
            },
            Event::AttachmentsAdded {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "log-files".to_string(),
                attachments: vec![AttachmentRecord {
                    filename: "photo-2.jpg".to_string(),
                    path: "attachments/photo-2.jpg".to_string(),
                    content_type: "image/jpeg".to_string(),
                    sha256: "def456".to_string(),
                }],
            },
            Event::AttachmentFileRemoved {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "log-files".to_string(),
                path: "attachments/photo-2.jpg".to_string(),
            },
            Event::AttachmentsCleared {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "log-files".to_string(),
            },
            Event::AttachmentRemoved {
                at: now,
                execution_id: id,
                step_id: "step-0".to_string(),
                input_id: "log-file".to_string(),
                reason: "Wrong file".to_string(),
            },
            Event::ExecutionRenamed {
                at: now,
                execution_id: id,
                name: "New Name".to_string(),
            },
            Event::LogMeta {
                at: now,
                version: SUPPORTED_VERSION,
                tool_version: "0.1.0".to_string(),
            },
        ];

        // Round-trip all event types through JSON.
        for event in &all_events {
            let json = serde_json::to_string(event).unwrap();
            let deserialized: Event = serde_json::from_str(&json).unwrap();
            assert_eq!(*event, deserialized);
        }
    }
}
