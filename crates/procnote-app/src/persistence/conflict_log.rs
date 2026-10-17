//! `conflicts.jsonl`: pushed events that didn't apply ([`crate::sync`]).
//!
//! It sits next to an execution's `events.jsonl` and, like it, is only
//! appended to: a conflict line when one is kept, a resolution line when
//! someone deals with it.

use std::io::Write;
use std::path::Path;

use chrono::{DateTime, Utc};
use procnote_core::event::types::EventRecord;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::sync::{Conflict, Resolution};

pub const FILE_NAME: &str = "conflicts.jsonl";

#[derive(Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
enum Line {
    Conflict {
        record: serde_json::Value,
        reason: String,
        received_at: DateTime<Utc>,
    },
    Resolved {
        event_id: Uuid,
        #[serde(flatten)]
        resolution: Resolution,
    },
}

/// The conflicts the lines describe, in the order they were kept.
pub fn parse(text: &str) -> Result<Vec<Conflict>, String> {
    let mut conflicts: Vec<Conflict> = Vec::new();
    for line in text.lines().filter(|line| !line.trim().is_empty()) {
        match serde_json::from_str(line).map_err(|e| format!("invalid conflict line: {e}"))? {
            Line::Conflict {
                record,
                reason,
                received_at,
            } => conflicts.push(Conflict {
                record: EventRecord::from_value(&record)
                    .map_err(|e| format!("invalid conflict record: {e}"))?,
                reason,
                received_at,
                resolved: None,
            }),
            Line::Resolved {
                event_id,
                resolution,
            } => {
                if let Some(conflict) = conflicts
                    .iter_mut()
                    .find(|conflict| conflict.record.meta.event_id == Some(event_id))
                {
                    conflict.resolved = Some(resolution);
                }
            }
        }
    }
    Ok(conflicts)
}

pub fn read(execution_dir: &Path) -> Result<Vec<Conflict>, String> {
    match std::fs::read_to_string(execution_dir.join(FILE_NAME)) {
        Ok(text) => parse(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Vec::new()),
        Err(e) => Err(e.to_string()),
    }
}

fn append(execution_dir: &Path, line: &Line) -> Result<(), String> {
    let mut text = serde_json::to_string(line).map_err(|e| e.to_string())?;
    text.push('\n');
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(execution_dir.join(FILE_NAME))
        .map_err(|e| e.to_string())?;
    file.write_all(text.as_bytes()).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())
}

pub fn append_conflict(execution_dir: &Path, conflict: &Conflict) -> Result<(), String> {
    append(
        execution_dir,
        &Line::Conflict {
            record: serde_json::to_value(&conflict.record).map_err(|e| e.to_string())?,
            reason: conflict.reason.clone(),
            received_at: conflict.received_at,
        },
    )
}

pub fn append_resolution(
    execution_dir: &Path,
    event_id: Uuid,
    resolution: &Resolution,
) -> Result<(), String> {
    append(
        execution_dir,
        &Line::Resolved {
            event_id,
            resolution: resolution.clone(),
        },
    )
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_core::event::types::{Event, EventMeta};

    #[test]
    fn conflicts_and_their_resolutions_read_back() {
        let dir = tempfile::tempdir().unwrap();
        let event_id = Uuid::new_v4();
        let conflict = Conflict {
            record: EventRecord {
                event: Event::ExecutionRenamed {
                    at: Utc::now(),
                    execution_id: Uuid::new_v4(),
                    name: "x".to_string(),
                },
                meta: EventMeta {
                    event_id: Some(event_id),
                    recorded_by: Some("offline".to_string()),
                    device: None,
                },
            },
            reason: "execution is finished".to_string(),
            received_at: Utc::now(),
            resolved: None,
        };
        let resolution = Resolution {
            at: Utc::now(),
            by: Some("reviewer".to_string()),
        };

        append_conflict(dir.path(), &conflict).unwrap();
        let open = read(dir.path()).unwrap();
        append_resolution(dir.path(), event_id, &resolution).unwrap();
        let resolved = read(dir.path()).unwrap();

        assert_eq!(open, vec![conflict.clone()]);
        assert_eq!(
            resolved,
            vec![Conflict {
                resolved: Some(resolution),
                ..conflict
            }]
        );
    }
}
