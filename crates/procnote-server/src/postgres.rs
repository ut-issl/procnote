//! Executions stored in PostgreSQL, with attachment content in an object
//! store (a local directory, or an S3-compatible bucket such as Cloudflare R2)
//! under `sha256/{hash}`.
//!
//! Each event-log line is a row of `events`, so a log can be exported back to
//! `events.jsonl` unchanged. Recording takes a row lock on the execution, so
//! writers of the same execution are serialized as with the file lock of the
//! filesystem store.
//!
//! [`ExecutionRepository`] is synchronous: call it from blocking threads
//! (`spawn_blocking`), never from async tasks.

use std::sync::Arc;

use chrono::Utc;
use object_store::{ObjectStore, ObjectStoreExt, PutMode};
use procnote_app::persistence::attachment_store::sha256_hex;
use procnote_app::repository::{
    Decide, ExecutionRepository, NewExecution, StagedFile, StoredExecution, apply_decision,
};
use procnote_core::event::SUPPORTED_VERSION;
use procnote_core::event::types::{AttachmentRecord, Event, EventMeta, EventRecord, ExecutionId};
use procnote_core::execution::ExecutionState;
use serde::Deserialize;
use sqlx::postgres::{PgPool, PgPoolOptions};
use sqlx::types::Json;
use tokio::runtime::Handle;

pub struct PostgresStore {
    pool: PgPool,
    runtime: Handle,
    /// Attachment content, one object per SHA-256.
    attachments: Arc<dyn ObjectStore>,
}

impl PostgresStore {
    /// Connect and bring the schema up to date. Must be called within a
    /// Tokio runtime, which the store then uses to run its queries.
    pub async fn connect(
        database_url: &str,
        attachments: Arc<dyn ObjectStore>,
    ) -> Result<Self, String> {
        let pool = PgPoolOptions::new()
            .max_connections(8)
            .connect(database_url)
            .await
            .map_err(|e| format!("failed to connect to the database: {e}"))?;
        sqlx::migrate!()
            .run(&pool)
            .await
            .map_err(|e| format!("failed to migrate the database: {e}"))?;
        Ok(Self {
            pool,
            runtime: Handle::current(),
            attachments,
        })
    }

    /// The connection pool, for other tables in the same database.
    #[must_use]
    pub fn pool(&self) -> PgPool {
        self.pool.clone()
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
            self.store_content(file).await?;
            insert_attachment(
                &mut tx,
                execution_id,
                &file.record.path,
                &file.record.sha256,
            )
            .await?;
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

    /// Store a staged file's content, unless it is already stored. The
    /// content is checked against the staged hash, so a source that changed
    /// after staging is rejected.
    async fn store_content(&self, file: &StagedFile) -> Result<(), String> {
        let bytes = std::fs::read(&file.source).map_err(|e| e.to_string())?;
        let actual = sha256_hex(&bytes);
        if actual != file.record.sha256 {
            return Err(format!(
                "attachment changed after it was staged: expected {}, got {actual}",
                file.record.sha256
            ));
        }
        match self
            .attachments
            .put_opts(
                &content_key(&file.record.sha256),
                bytes.into(),
                PutMode::Create.into(),
            )
            .await
        {
            Ok(_) | Err(object_store::Error::AlreadyExists { .. }) => Ok(()),
            Err(e) => Err(format!("failed to store attachment: {e}")),
        }
    }
}

/// An execution as moved between storages: its log lines verbatim, its
/// template snapshot, and the files its events reference.
pub struct TransferredExecution<F> {
    pub procedure: String,
    pub template_source: String,
    pub records: Vec<EventRecord>,
    pub files: Vec<F>,
}

impl PostgresStore {
    /// Store an execution's log as it is. Returns `false`, changing nothing,
    /// when an execution with its ID is already stored.
    pub async fn import(
        &self,
        execution: TransferredExecution<StagedFile>,
    ) -> Result<bool, String> {
        let state = replay(&execution.records)?;
        let execution_id = state
            .execution_id
            .ok_or("event log has no ExecutionStarted")?;
        let mut tx = self.pool.begin().await.map_err(db_error)?;
        let inserted = sqlx::query(
            "INSERT INTO executions (id, procedure, template_source) VALUES ($1, $2, $3) \
             ON CONFLICT DO NOTHING",
        )
        .bind(execution_id)
        .bind(&execution.procedure)
        .bind(&execution.template_source)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?
        .rows_affected();
        if inserted == 0 {
            return Ok(false);
        }
        for file in &execution.files {
            self.store_content(file).await?;
            insert_attachment(
                &mut tx,
                execution_id,
                &file.record.path,
                &file.record.sha256,
            )
            .await?;
        }
        for (seq, record) in execution.records.iter().enumerate() {
            insert_event(&mut tx, execution_id, seq, record).await?;
        }
        tx.commit().await.map_err(db_error)?;
        Ok(true)
    }

    /// IDs of all stored executions, oldest first.
    pub async fn execution_ids(&self) -> Result<Vec<ExecutionId>, String> {
        sqlx::query_scalar("SELECT id FROM executions ORDER BY created_at")
            .fetch_all(&self.pool)
            .await
            .map_err(db_error)
    }

    /// An execution's log, template snapshot, and attachment contents by
    /// stored path.
    pub async fn export(
        &self,
        execution_id: ExecutionId,
    ) -> Result<TransferredExecution<(String, Vec<u8>)>, String> {
        let (procedure, template_source): (String, String) =
            sqlx::query_as("SELECT procedure, template_source FROM executions WHERE id = $1")
                .bind(execution_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(db_error)?
                .ok_or_else(|| format!("Execution not found: {execution_id}"))?;
        let records = self.records(&self.pool, execution_id).await?;
        let attachments: Vec<(String, String)> = sqlx::query_as(
            "SELECT path, sha256 FROM attachments WHERE execution_id = $1 ORDER BY path",
        )
        .bind(execution_id)
        .fetch_all(&self.pool)
        .await
        .map_err(db_error)?;
        let mut files = Vec::with_capacity(attachments.len());
        for (path, sha256) in attachments {
            let content = self.content(&sha256).await?;
            files.push((path, content));
        }
        Ok(TransferredExecution {
            procedure,
            template_source,
            records,
            files,
        })
    }

    async fn content(&self, sha256: &str) -> Result<Vec<u8>, String> {
        let content = self
            .attachments
            .get(&content_key(sha256))
            .await
            .map_err(|e| format!("failed to read attachment: {e}"))?;
        content
            .bytes()
            .await
            .map(|bytes| bytes.to_vec())
            .map_err(|e| format!("failed to read attachment: {e}"))
    }
}

/// The attachment files a log references, by stored path (first record of
/// each path wins).
#[must_use]
pub fn referenced_files(records: &[EventRecord]) -> Vec<AttachmentRecord> {
    let mut files: Vec<AttachmentRecord> = Vec::new();
    for record in records {
        let referenced = match &record.event {
            Event::AttachmentsAdded { attachments, .. } => attachments.clone(),
            Event::AttachmentReceived { attachment, .. } => vec![attachment.clone()],
            Event::AttachmentAdded {
                filename,
                path,
                content_type,
                sha256,
                ..
            } => vec![AttachmentRecord {
                filename: filename.clone(),
                path: path.clone(),
                content_type: content_type.clone(),
                sha256: sha256.clone(),
            }],
            _ => Vec::new(),
        };
        for file in referenced {
            if !files.iter().any(|known| known.path == file.path) {
                files.push(file);
            }
        }
    }
    files
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
        self.runtime.block_on(self.content(&sha256))
    }
}

async fn insert_attachment(
    tx: &mut sqlx::Transaction<'static, sqlx::Postgres>,
    execution_id: ExecutionId,
    path: &str,
    sha256: &str,
) -> Result<(), String> {
    sqlx::query(
        "INSERT INTO attachments (execution_id, path, sha256) VALUES ($1, $2, $3) \
         ON CONFLICT DO NOTHING",
    )
    .bind(execution_id)
    .bind(path)
    .bind(sha256)
    .execute(&mut **tx)
    .await
    .map_err(db_error)?;
    Ok(())
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

/// The object holding content with this SHA-256.
fn content_key(sha256: &str) -> object_store::path::Path {
    object_store::path::Path::from_iter(["sha256", sha256])
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str, sha256: &str) -> AttachmentRecord {
        AttachmentRecord {
            filename: path.to_string(),
            path: path.to_string(),
            content_type: "text/plain".to_string(),
            sha256: sha256.to_string(),
        }
    }

    #[test]
    fn referenced_files_lists_each_stored_path_once() {
        let at = Utc::now();
        let execution_id = uuid::Uuid::new_v4();
        let records: Vec<EventRecord> = vec![
            Event::AttachmentReceived {
                at,
                execution_id,
                attachment: file("attachments/a", "1"),
            }
            .into(),
            Event::AttachmentsAdded {
                at,
                execution_id,
                step_id: "s".to_string(),
                input_id: "i".to_string(),
                attachments: vec![file("attachments/a", "1"), file("attachments/b", "2")],
            }
            .into(),
            Event::ExecutionRenamed {
                at,
                execution_id,
                name: "x".to_string(),
            }
            .into(),
        ];

        let paths: Vec<String> = referenced_files(&records)
            .into_iter()
            .map(|file| file.path)
            .collect();

        assert_eq!(paths, ["attachments/a", "attachments/b"]);
    }
}
