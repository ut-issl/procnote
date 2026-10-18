//! Syncing a workspace's linked executions with the server.
//!
//! One sync of an execution: register it if the server doesn't have it yet,
//! upload the content its pending events attach, push them, pull what the
//! server has after this copy (downloading attachments recorded elsewhere),
//! then settle — see `ExecutionStore::settle` for why in that order.

use std::collections::HashSet;

use procnote_app::persistence::execution_store::{ExecutionStore, Settlement};
use procnote_app::persistence::outbox::Rejected;
use procnote_app::repository::{ExecutionRepository, TemplateSnapshot};
use procnote_app::sync::{PushOutcome, referenced_files};
use procnote_core::event::types::{Event, EventRecord, ExecutionId};
use procnote_core::template::parse_template;
use procnote_core::template::types::ProcedureTemplate;

use crate::client::{ClientError, DeviceClient};

/// Why a sync didn't complete.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SyncError {
    /// The server can't be reached now; try later.
    Unreachable(String),
    /// The device token was refused; connect again.
    Unauthorized,
    /// Something else went wrong (reported, not retried silently).
    Failed(String),
}

impl std::fmt::Display for SyncError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "the server can't be reached: {e}"),
            Self::Unauthorized => write!(
                f,
                "this device was disconnected by the server; connect again"
            ),
            Self::Failed(e) => write!(f, "{e}"),
        }
    }
}

impl From<ClientError> for SyncError {
    fn from(error: ClientError) -> Self {
        match error {
            ClientError::Unreachable(e) => Self::Unreachable(e),
            ClientError::Unauthorized => Self::Unauthorized,
            ClientError::Refused(e) => Self::Failed(e),
        }
    }
}

impl From<String> for SyncError {
    fn from(error: String) -> Self {
        Self::Failed(error)
    }
}

/// What one sync did.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Synced {
    pub registered: bool,
    pub pushed: usize,
    pub rejected: usize,
    pub pulled: usize,
}

/// The template an execution started here was started from, as the server
/// takes it when registering.
fn template_of(store: &ExecutionStore, id: ExecutionId) -> Result<ProcedureTemplate, String> {
    match store.template_snapshot(id)? {
        TemplateSnapshot::Markdown(text) => parse_template(&text).map_err(|e| e.to_string()),
        TemplateSnapshot::Json(text) => serde_json::from_str(&text).map_err(|e| e.to_string()),
    }
}

/// Download the content of files the records attach that this copy doesn't have.
async fn download_attachments(
    store: &ExecutionStore,
    client: &DeviceClient,
    id: ExecutionId,
    records: &[EventRecord],
) -> Result<(), SyncError> {
    let mut seen = HashSet::new();
    for file in records
        .iter()
        .flat_map(|record| referenced_files(&record.event))
    {
        if !seen.insert(file.path.clone()) || store.read_attachment(id, &file.path).is_ok() {
            continue;
        }
        let content = client.attachment(id, &file.path).await?;
        store.store_attachment(id, &file, &content)?;
    }
    Ok(())
}

/// Sync one linked execution.
pub async fn sync_execution(
    store: &ExecutionStore,
    client: &DeviceClient,
    id: ExecutionId,
) -> Result<Synced, SyncError> {
    let outbox = store.outbox(id)?;
    let mut synced = Synced::default();

    if !outbox.sync.registered {
        let starting: Vec<EventRecord> = outbox
            .records
            .iter()
            .filter(|record| !matches!(record.event, Event::LogMeta { .. }))
            .cloned()
            .collect();
        client
            .register(
                id,
                &store.procedure_of(id)?,
                &template_of(store, id)?,
                &starting,
            )
            .await?;
        synced.registered = true;
    }

    let mut rejected = Vec::new();
    if !outbox.pending.is_empty() {
        let mut files = Vec::new();
        let mut seen = HashSet::new();
        for (record, path) in &outbox.files {
            if seen.insert(record.sha256.clone()) {
                let content =
                    std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
                files.push((record.filename.clone(), content));
            }
        }
        let uploads = if files.is_empty() {
            Vec::new()
        } else {
            client.upload(files).await?
        };
        let results = client.push(id, &outbox.pending, &uploads).await?;
        if results.len() != outbox.pending.len() {
            return Err(SyncError::Failed(format!(
                "the server answered {} of {} pushed events",
                results.len(),
                outbox.pending.len()
            )));
        }
        for (record, result) in outbox.pending.iter().zip(results) {
            if let PushOutcome::Rejected { reason } = result.outcome {
                rejected.push(Rejected {
                    record: record.clone(),
                    reason,
                });
            }
        }
        synced.pushed = outbox.pending.len();
        synced.rejected = rejected.len();
    }

    let after = outbox.records.len();
    let pulled = client.pull(id, after).await?;
    if pulled.count < after {
        return Err(SyncError::Failed(format!(
            "the server has {} log lines but this copy has {after}; the copies diverged",
            pulled.count
        )));
    }
    download_attachments(store, client, id, &pulled.records).await?;
    synced.pulled = pulled.records.len();
    store.settle(
        id,
        &Settlement {
            registered: synced.registered,
            pending_upto: outbox.pending_upto,
            rejected,
            pulled: pulled.records,
            pulled_after: after,
        },
    )?;
    Ok(synced)
}

/// Sync every linked execution in the workspace. Stops at the first error
/// that means the server is out of reach or refuses this device.
pub async fn sync_all(store: &ExecutionStore, client: &DeviceClient) -> Result<usize, SyncError> {
    let mut synced = 0;
    let mut failure = None;
    for (id, _) in store.linked()? {
        match sync_execution(store, client, id).await {
            Ok(_) => synced += 1,
            Err(e @ (SyncError::Unreachable(_) | SyncError::Unauthorized)) => return Err(e),
            Err(SyncError::Failed(e)) => {
                log::warn!("sync of execution {id} failed: {e}");
                failure.get_or_insert_with(|| SyncError::Failed(format!("execution {id}: {e}")));
            }
        }
    }
    failure.map_or(Ok(synced), Err)
}

/// How many recorded things wait to reach the server (pending events, and
/// executions the server doesn't have yet).
pub fn waiting(store: &ExecutionStore) -> Result<usize, String> {
    let mut count = 0;
    for (id, sync) in store.linked()? {
        count += store.outbox(id)?.pending.len() + usize::from(!sync.registered);
    }
    Ok(count)
}

/// Keep a copy of a server execution here, linked, with its attachments.
pub async fn open_remote(
    store: &ExecutionStore,
    client: &DeviceClient,
    id: ExecutionId,
) -> Result<(), SyncError> {
    let remote = client.execution(id).await?;
    let template = TemplateSnapshot::from_format(&remote.template_format, remote.template)?;
    store.import_remote(&remote.procedure, &template, &remote.records)?;
    download_attachments(store, client, id, &remote.records).await
}
