//! Executions stored in PostgreSQL, with attachment content in a directory.
//!
//! Each event-log line is a row of `events`, so a log can be exported back to
//! `events.jsonl` unchanged. Recording takes a row lock on the execution, so
//! writers of the same execution are serialized as with the file lock of the
//! filesystem store.
//!
//! [`ExecutionRepository`] is synchronous: call it from blocking threads
//! (`spawn_blocking`), never from async tasks.

use std::io::Write;
use std::path::{Path, PathBuf};

use chrono::Utc;
use procnote_app::persistence::attachment_store::compute_sha256;
use procnote_app::repository::{
    Decide, ExecutionRepository, NewExecution, StagedFile, StoredExecution, apply_decision,
};
use procnote_core::event::SUPPORTED_VERSION;
use procnote_core::event::types::{Event, EventMeta, EventRecord, ExecutionId};
use procnote_core::execution::ExecutionState;
use serde::Deserialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;
use tokio::runtime::Handle;

pub struct PostgresStore {
    pool: PgPool,
    runtime: Handle,
    /// Attachment content, one file per SHA-256.
    attachment_dir: PathBuf,
}

impl PostgresStore {
    /// Connect and bring the schema up to date. Must be called within a
    /// Tokio runtime, which the store then uses to run its queries.
    pub async fn connect(database_url: &str, attachment_dir: PathBuf) -> Result<Self, String> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| format!("failed to connect to the database: {e}"))?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .map_err(|e| format!("failed to migrate the database: {e}"))?;
        std::fs::create_dir_all(&attachment_dir).map_err(|e| {
            format!(
                "failed to create attachment directory {}: {e}",
                attachment_dir.display()
            )
        })?;
        Ok(Self {
            pool,
            runtime: Handle::current(),
            attachment_dir,
        })
    }

    async fn records(
        &self,
        executor: impl sqlx::PgExecutor<'_>,
        execution_id: ExecutionId,
    ) -> Result<Vec<EventRecord>, String> {
        let rows: Vec<serde_json::Value> =
            sqlx::query_scalar("SELECT record FROM events WHERE execution_id = $1 ORDER BY seq")
                .bind(execution_id)
                .fetch_all(executor)
                .await
                .map_err(db_error)?;
        rows.iter().map(parse_record).collect()
    }

    #[expect(
        clippy::future_not_send,
        reason = "only run with block_on on the calling thread, never spawned"
    )]
    async fn record_async(
        &self,
        execution_id: ExecutionId,
        meta: &EventMeta,
        decide: &mut Decide<'_>,
    ) -> Result<StoredExecution, String> {
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let exists: Option<i32> =
            sqlx::query_scalar("SELECT 1 FROM executions WHERE id = $1 FOR UPDATE")
                .bind(execution_id)
                .fetch_optional(&mut *tx)
                .await
                .map_err(db_error)?;
        if exists.is_none() {
            return Err(format!("Execution not found: {execution_id}"));
        }
        let records = self.records(&mut *tx, execution_id).await?;
        let mut state = replay(&records)?;

        let is_retry = meta.event_id.is_some_and(|id| {
            records
                .iter()
                .any(|record| record.meta.event_id == Some(id))
        });
        if is_retry {
            return Ok(stored(state));
        }

        let decision = decide(&state)?;
        if decision.events.is_empty() {
            return Ok(stored(state));
        }
        apply_decision(&mut state, &decision)?;
        for file in &decision.files {
            self.store_content(file)?;
            sqlx::query(
                "INSERT INTO attachments (execution_id, path, sha256) VALUES ($1, $2, $3) \
                 ON CONFLICT DO NOTHING",
            )
            .bind(execution_id)
            .bind(&file.record.path)
            .bind(&file.record.sha256)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
        }
        let first_seq = records.len();
        for (offset, event) in decision.events.into_iter().enumerate() {
            insert_event(
                &mut tx,
                execution_id,
                first_seq + offset,
                &EventRecord {
                    event,
                    meta: meta.clone(),
                },
            )
            .await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(stored(state))
    }

    fn content_path(&self, sha256: &str) -> PathBuf {
        self.attachment_dir.join(sha256)
    }

    /// Durably copy a staged file's content, unless it is already stored.
    fn store_content(&self, file: &StagedFile) -> Result<(), String> {
        let destination = self.content_path(&file.record.sha256);
        if destination.exists() {
            return Ok(());
        }
        let temp = self.attachment_dir.join(format!(
            ".{}.tmp-{}",
            file.record.sha256,
            uuid::Uuid::new_v4()
        ));
        let result = copy_verified(&file.source, &temp, &file.record.sha256)
            .and_then(|()| std::fs::rename(&temp, &destination).map_err(|e| e.to_string()));
        if result.is_err() {
            let _ = std::fs::remove_file(&temp);
        }
        result
    }
}

impl ExecutionRepository for PostgresStore {
    fn list(&self) -> Result<Vec<StoredExecution>, String> {
        self.runtime.block_on(async {
            let ids: Vec<ExecutionId> =
                sqlx::query_scalar("SELECT id FROM executions ORDER BY created_at")
                    .fetch_all(&self.pool)
                    .await
                    .map_err(db_error)?;
            let mut executions = Vec::with_capacity(ids.len());
            for id in ids {
                match replay(&self.records(&self.pool, id).await?) {
                    Ok(state) => executions.push(stored(state)),
                    Err(e) => log::warn!("skipping unreadable execution {id}: {e}"),
                }
            }
            Ok(executions)
        })
    }

    fn load(&self, execution_id: ExecutionId) -> Result<StoredExecution, String> {
        self.runtime.block_on(async {
            let records = self.records(&self.pool, execution_id).await?;
            if records.is_empty() {
                return Err(format!("Execution not found: {execution_id}"));
            }
            replay(&records).map(stored)
        })
    }

    fn create(&self, new: NewExecution) -> Result<StoredExecution, String> {
        let Some(execution_id) = new.state.execution_id else {
            return Err("execution state has not been started".to_string());
        };
        self.runtime.block_on(async {
            let mut tx = self.pool.begin().await.map_err(db_error)?;
            sqlx::query(
                "INSERT INTO executions (id, procedure, template_source) VALUES ($1, $2, $3)",
            )
            .bind(execution_id)
            .bind(&new.procedure)
            .bind(&new.template_source)
            .execute(&mut *tx)
            .await
            .map_err(db_error)?;
            let log_meta = EventRecord::from(Event::LogMeta {
                at: Utc::now(),
                version: SUPPORTED_VERSION,
                tool_version: new.tool_version,
            });
            let records =
                std::iter::once(log_meta).chain(new.initial_events.into_iter().map(|event| {
                    EventRecord {
                        event,
                        meta: new.meta.clone(),
                    }
                }));
            for (seq, record) in records.enumerate() {
                insert_event(&mut tx, execution_id, seq, &record).await?;
            }
            tx.commit().await.map_err(db_error)?;
            Ok(stored(new.state))
        })
    }

    fn record(
        &self,
        execution_id: ExecutionId,
        meta: &EventMeta,
        decide: &mut Decide<'_>,
    ) -> Result<StoredExecution, String> {
        self.runtime
            .block_on(self.record_async(execution_id, meta, decide))
    }

    fn read_attachment(&self, execution_id: ExecutionId, path: &str) -> Result<Vec<u8>, String> {
        let sha256: Option<String> = self.runtime.block_on(async {
            sqlx::query_scalar(
                "SELECT sha256 FROM attachments WHERE execution_id = $1 AND path = $2",
            )
            .bind(execution_id)
            .bind(path)
            .fetch_optional(&self.pool)
            .await
            .map_err(db_error)
        })?;
        let sha256 = sha256.ok_or_else(|| format!("Attachment not found: {path}"))?;
        std::fs::read(self.content_path(&sha256)).map_err(|e| e.to_string())
    }
}

async fn insert_event(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    execution_id: ExecutionId,
    seq: usize,
    record: &EventRecord,
) -> Result<(), String> {
    let seq = i64::try_from(seq).map_err(|e| e.to_string())?;
    sqlx::query("INSERT INTO events (execution_id, seq, event_id, record) VALUES ($1, $2, $3, $4)")
        .bind(execution_id)
        .bind(seq)
        .bind(record.meta.event_id)
        .bind(Json(record))
        .execute(&mut **tx)
        .await
        .map_err(db_error)?;
    Ok(())
}

/// Parse a stored log line, as the file log reader does: the event and its
/// metadata are read from the same JSON object.
fn parse_record(value: &serde_json::Value) -> Result<EventRecord, String> {
    let invalid = |e: serde_json::Error| format!("invalid stored event: {e}");
    Ok(EventRecord {
        event: Event::deserialize(value).map_err(invalid)?,
        meta: EventMeta::deserialize(value).map_err(invalid)?,
    })
}

/// Replay a log whose first line is a `LogMeta` of the supported version.
fn replay(records: &[EventRecord]) -> Result<ExecutionState, String> {
    match records.first().map(|record| &record.event) {
        Some(Event::LogMeta { version, .. }) if *version == SUPPORTED_VERSION => {}
        Some(Event::LogMeta { version, .. }) => {
            return Err(format!(
                "unsupported event log version {version} (supported: {SUPPORTED_VERSION})"
            ));
        }
        _ => return Err("event log does not start with LogMeta".to_string()),
    }
    let events: Vec<Event> = records.iter().map(|record| record.event.clone()).collect();
    ExecutionState::from_events(&events).map_err(|e| e.to_string())
}

const fn stored(state: ExecutionState) -> StoredExecution {
    StoredExecution {
        state,
        location: None,
    }
}

#[expect(clippy::needless_pass_by_value, reason = "used as a map_err function")]
fn db_error(error: sqlx::Error) -> String {
    format!("database error: {error}")
}

/// Copy `source` to a new file at `destination`, durably, checking that the
/// copied content still has the expected hash.
fn copy_verified(source: &Path, destination: &Path, sha256: &str) -> Result<(), String> {
    let bytes = std::fs::read(source).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(destination)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    let copied = compute_sha256(destination).map_err(|e| e.to_string())?;
    if copied == sha256 {
        Ok(())
    } else {
        Err(format!(
            "attachment changed while copying: expected {sha256}, got {copied}"
        ))
    }
}
