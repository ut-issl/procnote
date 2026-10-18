//! Files of an execution linked to a server (desktop sync, migration plan
//! stage 7), next to its `events.jsonl`:
//!
//! - `events.jsonl` holds the log in the server's order (confirmed events).
//! - `pending.jsonl` holds events recorded here and not yet confirmed, in
//!   order. Like the log it is only appended to.
//! - `rejected.jsonl` holds pushed events the server refused as malformed,
//!   with the reason, so nothing recorded is lost.
//! - `sync.json` (state, rewritten atomically) says whether the server knows
//!   the execution and how many pending lines have been dealt with.
//!
//! The execution's state is the log plus the pending lines not dealt with.

use std::io::Write;
use std::path::Path;

use procnote_core::event::types::EventRecord;
use serde::{Deserialize, Serialize};

use crate::persistence::event_log::sync_dir;

pub const PENDING_FILE: &str = "pending.jsonl";
pub const REJECTED_FILE: &str = "rejected.jsonl";
pub const SYNC_FILE: &str = "sync.json";

/// The sync state of a linked execution.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SyncState {
    /// Whether the server has the execution (it was started there, or this
    /// device registered it).
    pub registered: bool,
    /// How many lines of `pending.jsonl` the server has dealt with
    /// (recorded, deduplicated, kept as conflicts, or refused).
    pub handled: usize,
}

/// A pushed event the server refused, kept with its reason.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Rejected {
    pub record: EventRecord,
    pub reason: String,
}

/// The sync state, or `None` when the execution isn't linked to a server.
pub fn read_state(execution_dir: &Path) -> Result<Option<SyncState>, String> {
    match std::fs::read(execution_dir.join(SYNC_FILE)) {
        Ok(json) => serde_json::from_slice(&json)
            .map(Some)
            .map_err(|e| format!("invalid {SYNC_FILE}: {e}")),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Replace the sync state durably (write a temporary file, then rename).
pub fn write_state(execution_dir: &Path, state: SyncState) -> Result<(), String> {
    let temp = execution_dir.join(format!(".{SYNC_FILE}.tmp-{}", uuid::Uuid::new_v4()));
    let json = serde_json::to_vec(&state).map_err(|e| e.to_string())?;
    let write = || -> std::io::Result<()> {
        let mut file = std::fs::File::create(&temp)?;
        file.write_all(&json)?;
        file.sync_all()?;
        std::fs::rename(&temp, execution_dir.join(SYNC_FILE))?;
        sync_dir(execution_dir)
    };
    write().map_err(|e| {
        let _ = std::fs::remove_file(&temp);
        e.to_string()
    })
}

/// Parse JSON lines, ignoring a truncated last line (a write cut short by a
/// crash, never acknowledged). A bad line before the last is an error.
fn parse_lines<T>(
    text: &str,
    parse: impl Fn(&serde_json::Value) -> Result<T, String>,
) -> Result<Vec<T>, String> {
    let lines: Vec<&str> = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .collect();
    let mut items = Vec::with_capacity(lines.len());
    for (index, line) in lines.iter().enumerate() {
        match serde_json::from_str::<serde_json::Value>(line) {
            Ok(value) => items.push(parse(&value)?),
            Err(_) if index + 1 == lines.len() && !text.ends_with('\n') => {}
            Err(e) => return Err(format!("corrupt line {}: {e}", index + 1)),
        }
    }
    Ok(items)
}

fn read_text(path: &Path) -> Result<String, String> {
    match std::fs::read_to_string(path) {
        Ok(text) => Ok(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(String::new()),
        Err(e) => Err(e.to_string()),
    }
}

/// Every line of `pending.jsonl`, handled or not.
pub fn read_pending(execution_dir: &Path) -> Result<Vec<EventRecord>, String> {
    parse_lines(&read_text(&execution_dir.join(PENDING_FILE))?, |value| {
        EventRecord::from_value(value).map_err(|e| format!("invalid pending event: {e}"))
    })
}

/// Refused events, each once (a crash may have written one twice).
pub fn read_rejected(execution_dir: &Path) -> Result<Vec<Rejected>, String> {
    let all = parse_lines(&read_text(&execution_dir.join(REJECTED_FILE))?, |value| {
        let record = value
            .get("record")
            .ok_or("rejected line without record")
            .and_then(|record| EventRecord::from_value(record).map_err(|_| "invalid record"))?;
        let reason = value
            .get("reason")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string();
        Ok(Rejected { record, reason })
    })?;
    let mut unique: Vec<Rejected> = Vec::with_capacity(all.len());
    for rejected in all {
        let id = rejected.record.meta.event_id;
        if id.is_none() || !unique.iter().any(|known| known.record.meta.event_id == id) {
            unique.push(rejected);
        }
    }
    Ok(unique)
}

fn append_lines(path: &Path, lines: &[serde_json::Value]) -> Result<(), String> {
    if lines.is_empty() {
        return Ok(());
    }
    let mut text = String::new();
    for line in lines {
        text.push_str(&serde_json::to_string(line).map_err(|e| e.to_string())?);
        text.push('\n');
    }
    let existed = path.exists();
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(path)
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    if !existed {
        path.parent()
            .map_or(Ok(()), sync_dir)
            .map_err(|e| e.to_string())?;
    }
    Ok(())
}

pub fn append_pending(execution_dir: &Path, records: &[EventRecord]) -> Result<(), String> {
    let lines = records
        .iter()
        .map(|record| serde_json::to_value(record).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    append_lines(&execution_dir.join(PENDING_FILE), &lines)
}

pub fn append_rejected(execution_dir: &Path, rejected: &[Rejected]) -> Result<(), String> {
    let lines = rejected
        .iter()
        .map(|rejected| serde_json::to_value(rejected).map_err(|e| e.to_string()))
        .collect::<Result<Vec<_>, _>>()?;
    append_lines(&execution_dir.join(REJECTED_FILE), &lines)
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use chrono::Utc;
    use procnote_core::event::types::{Event, EventMeta};

    fn record(name: &str) -> EventRecord {
        EventRecord {
            event: Event::ExecutionRenamed {
                at: Utc::now(),
                execution_id: uuid::Uuid::new_v4(),
                name: name.to_string(),
            },
            meta: EventMeta {
                event_id: Some(uuid::Uuid::new_v4()),
                ..EventMeta::default()
            },
        }
    }

    #[test]
    fn pending_lines_read_back_and_a_cut_off_last_line_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let (a, b) = (record("a"), record("b"));

        append_pending(dir.path(), std::slice::from_ref(&a)).unwrap();
        append_pending(dir.path(), std::slice::from_ref(&b)).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(dir.path().join(PENDING_FILE))
            .unwrap();
        file.write_all(b"{\"type\":\"execution_ren").unwrap();

        assert_eq!(read_pending(dir.path()).unwrap(), [a, b]);
    }

    #[test]
    fn a_corrupt_line_in_the_middle_is_an_error() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join(PENDING_FILE), "garbage\n{}\n").unwrap();

        assert!(read_pending(dir.path()).is_err());
    }

    #[test]
    fn the_sync_state_is_absent_until_linked_and_replaced_whole() {
        let dir = tempfile::tempdir().unwrap();

        assert_eq!(read_state(dir.path()).unwrap(), None);
        write_state(dir.path(), SyncState::default()).unwrap();
        let next = SyncState {
            registered: true,
            handled: 3,
        };
        write_state(dir.path(), next).unwrap();

        assert_eq!(read_state(dir.path()).unwrap(), Some(next));
    }

    #[test]
    fn rejected_events_are_kept_once_each() {
        let dir = tempfile::tempdir().unwrap();
        let refused = Rejected {
            record: record("x"),
            reason: "malformed".to_string(),
        };

        append_rejected(dir.path(), std::slice::from_ref(&refused)).unwrap();
        append_rejected(dir.path(), std::slice::from_ref(&refused)).unwrap();

        assert_eq!(read_rejected(dir.path()).unwrap(), [refused]);
    }
}
