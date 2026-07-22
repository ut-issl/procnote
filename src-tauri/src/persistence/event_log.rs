use std::io::Write;
use std::path::{Path, PathBuf};

use fs2::FileExt;
use procnote_core::event::types::Event;
use procnote_core::event::{EventLogError, read_log};

/// Filesystem-backed append-only JSONL event log.
///
/// This type is the Tauri shell's persistence boundary for `events.jsonl`.
/// Low-level durability concerns such as flushing, syncing, and parent
/// directory syncing should live here rather than in command handlers.
#[derive(Debug, Clone)]
pub struct EventLog {
    path: PathBuf,
}

impl EventLog {
    #[must_use]
    pub const fn new(path: PathBuf) -> Self {
        Self { path }
    }

    pub fn read(&self) -> Result<Vec<Event>, EventLogError> {
        read_log(&self.path)
    }

    pub fn read_locked(&self) -> Result<Vec<Event>, EventLogError> {
        self.with_shared_lock(|| self.read())?
    }

    /// Run a closure while holding an exclusive advisory lock for this event log.
    ///
    /// The lock file lives next to `events.jsonl`. Callers should keep the full
    /// read/replay/validate/append sequence inside this critical section.
    pub fn with_exclusive_lock<T>(&self, f: impl FnOnce() -> T) -> Result<T, EventLogError> {
        let lock_file = self.open_lock_file()?;
        lock_file.lock_exclusive()?;
        let result = f();
        self.unlock_or_warn(&lock_file);
        Ok(result)
    }

    pub fn with_shared_lock<T>(&self, f: impl FnOnce() -> T) -> Result<T, EventLogError> {
        let lock_file = self.open_lock_file()?;
        lock_file.lock_shared()?;
        let result = f();
        self.unlock_or_warn(&lock_file);
        Ok(result)
    }

    /// Append a single event and force it to durable storage before returning.
    ///
    /// A successful return means Procnote has asked the OS to persist both the
    /// file contents and, for newly-created logs, the parent directory entry.
    pub fn append_durable(&self, event: &Event) -> Result<(), EventLogError> {
        self.append_batch_durable(std::slice::from_ref(event))
    }

    /// Append a batch of events and force them to durable storage before returning.
    ///
    /// Events are written as complete JSONL lines in order. Readers already
    /// tolerate a truncated final line after a crash; syncing here reduces the
    /// window where a reported-successful append can still be lost.
    pub fn append_batch_durable(&self, events: &[Event]) -> Result<(), EventLogError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let existed = self.path.exists();
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.path)?;

        write_events(&mut file, events)?;
        file.flush()?;
        file.sync_all()?;

        if !existed {
            sync_parent_dir(&self.path)?;
        }

        Ok(())
    }

    /// Create a new event log with the provided events and sync it durably.
    ///
    /// This is intended for initial execution creation, where appending to a
    /// pre-existing log would indicate a storage bug.
    pub fn create_with_events_durable(&self, events: &[Event]) -> Result<(), EventLogError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&self.path)?;

        write_events(&mut file, events)?;
        file.flush()?;
        file.sync_all()?;
        sync_parent_dir(&self.path)?;

        Ok(())
    }

    fn open_lock_file(&self) -> Result<std::fs::File, EventLogError> {
        if let Some(parent) = self.path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let lock_path = self.lock_path();
        let lock_existed = lock_path.exists();
        let lock_file = std::fs::OpenOptions::new()
            .create(true)
            .truncate(false)
            .read(true)
            .write(true)
            .open(&lock_path)?;
        if !lock_existed {
            sync_parent_dir(&lock_path)?;
        }
        Ok(lock_file)
    }

    fn unlock_or_warn(&self, lock_file: &std::fs::File) {
        if let Err(e) = lock_file.unlock() {
            log::warn!(
                "failed to unlock event log {}; lock will be released on close: {e}",
                self.path.display()
            );
        }
    }

    fn lock_path(&self) -> PathBuf {
        let filename = self.path.file_name().map_or_else(
            || "events.jsonl.lock".to_string(),
            |name| format!("{}.lock", name.to_string_lossy()),
        );
        self.path.with_file_name(filename)
    }
}

fn write_events(file: &mut std::fs::File, events: &[Event]) -> Result<(), EventLogError> {
    for event in events {
        let json = serde_json::to_string(event)?;
        writeln!(file, "{json}")?;
    }
    Ok(())
}

fn sync_parent_dir(path: &Path) -> Result<(), std::io::Error> {
    path.parent().map_or(Ok(()), sync_dir)
}

#[cfg(not(windows))]
pub(super) fn sync_dir(path: &Path) -> Result<(), std::io::Error> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
#[expect(
    clippy::unnecessary_wraps,
    reason = "keeps the directory durability API uniform across platforms"
)]
pub(super) fn sync_dir(_path: &Path) -> Result<(), std::io::Error> {
    // Windows has no supported equivalent of POSIX directory fsync. Calling
    // File::sync_all on a read-only directory handle uses FlushFileBuffers,
    // which requires write access and returns ERROR_ACCESS_DENIED. Actual files
    // continue to use File::sync_all at their durable write points.
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use chrono::Utc;
    use procnote_core::event::SUPPORTED_VERSION;

    fn log_meta() -> Event {
        Event::LogMeta {
            at: Utc::now(),
            version: SUPPORTED_VERSION,
            tool_version: "test".to_string(),
        }
    }

    #[test]
    fn syncing_existing_directory_succeeds() {
        let dir = tempfile::tempdir().unwrap();

        sync_dir(dir.path()).unwrap();
    }

    #[test]
    fn durable_append_creates_parent_dirs_and_round_trips() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("events.jsonl");
        let log = EventLog::new(path);

        log.append_durable(&log_meta()).unwrap();

        let events = log.read().unwrap();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], Event::LogMeta { .. }));
    }

    #[test]
    fn durable_batch_appends_events_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let log = EventLog::new(dir.path().join("events.jsonl"));
        let first = log_meta();
        let second = Event::ExecutionRenamed {
            at: Utc::now(),
            execution_id: uuid::Uuid::new_v4(),
            name: "new-name".to_string(),
        };

        log.append_batch_durable(&[first.clone(), second.clone()])
            .unwrap();

        assert_eq!(log.read().unwrap(), vec![first, second]);
    }
}
