//! IPC commands for syncing the workspace with a procnote server.

use std::time::Duration;

use procnote_app::execution::ExecutionSummary;
use procnote_app::persistence::outbox::Rejected;
use procnote_app::repository::ExecutionRepository;
use procnote_core::event::types::ExecutionId;
use procnote_sync::client::DeviceClient;
use procnote_sync::connect;
use tauri::State;

use crate::server_sync::{ServerSync, SyncStatus};
use crate::state::AppState;

/// How long to wait for the browser to come back from the server's connect page.
const CONNECT_TIMEOUT: Duration = Duration::from_mins(10);

#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn get_sync_status(state: State<'_, AppState>, sync: State<'_, ServerSync>) -> SyncStatus {
    sync.status(&state.executions())
}

/// Connect the workspace to `server` as `device`: the browser opens the
/// server's connect page, and once the person confirms there, the device
/// token comes back here. Executions started from now on sync.
#[tauri::command]
pub async fn connect_server(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
    server: String,
    device: String,
) -> Result<SyncStatus, String> {
    let device = device.trim();
    if device.is_empty() {
        return Err("name this device".to_string());
    }
    let pending = connect::begin(server.trim(), device).await?;
    tauri_plugin_opener::open_url(pending.url.as_str(), None::<&str>).map_err(|e| e.to_string())?;
    let link = pending.finish(CONNECT_TIMEOUT).await?;
    DeviceClient::new(link.clone())
        .me()
        .await
        .map_err(|e| e.to_string())?;
    sync.set_link(Some(link))?;
    sync.wake();
    Ok(sync.status(&state.executions()))
}

/// Disconnect the workspace. Executions already linked keep what they
/// recorded and sync again once the workspace is reconnected.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn disconnect_server(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
) -> Result<SyncStatus, String> {
    sync.set_link(None)?;
    Ok(sync.status(&state.executions()))
}

#[tauri::command]
pub async fn sync_now(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
) -> Result<SyncStatus, String> {
    Ok(sync.sync_now(&state.executions()).await)
}

fn client(sync: &ServerSync) -> Result<DeviceClient, String> {
    sync.link()?
        .map(DeviceClient::new)
        .ok_or_else(|| "this workspace isn't connected to a server".to_string())
}

/// The server's conflicts for an execution (events that arrived after it moved on).
#[tauri::command]
pub async fn list_conflicts(
    sync: State<'_, ServerSync>,
    execution_id: ExecutionId,
) -> Result<Vec<serde_json::Value>, String> {
    client(&sync)?
        .conflicts(execution_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn resolve_conflict(
    sync: State<'_, ServerSync>,
    execution_id: ExecutionId,
    event_id: uuid::Uuid,
) -> Result<Vec<serde_json::Value>, String> {
    client(&sync)?
        .resolve_conflict(execution_id, event_id)
        .await
        .map_err(|e| e.to_string())
}

/// Events of a linked execution the server refused as malformed.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_rejected(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
) -> Result<Vec<Rejected>, String> {
    state.executions().rejected(execution_id)
}

/// Whether an execution syncs with the server: `Some(registered)` when linked.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn execution_link(
    state: State<'_, AppState>,
    execution_id: ExecutionId,
) -> Result<Option<bool>, String> {
    Ok(state
        .executions()
        .sync_state(execution_id)?
        .map(|sync| sync.registered))
}

/// The phone upload link of a linked execution, as the URL to open: files
/// sent there reach this app on the next sync.
#[tauri::command]
pub async fn create_upload_link(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
    execution_id: ExecutionId,
) -> Result<String, String> {
    let store = state.executions();
    match store.sync_state(execution_id)? {
        None => return Err("This execution isn't shared with the server.".to_string()),
        Some(linked) if !linked.registered => {
            // The server must have the execution before phones can send to it.
            sync.sync_execution(&store, execution_id).await?;
        }
        Some(_) => {}
    }
    let client = client(&sync)?;
    let token = client
        .upload_link(execution_id, true)
        .await
        .map_err(|e| e.to_string())?
        .ok_or("the server didn't create a link")?;
    Ok(format!(
        "{}/upload/{token}",
        client.link().server.trim_end_matches('/')
    ))
}

#[tauri::command]
pub async fn revoke_upload_link(
    sync: State<'_, ServerSync>,
    execution_id: ExecutionId,
) -> Result<(), String> {
    client(&sync)?
        .revoke_upload_link(execution_id)
        .await
        .map_err(|e| e.to_string())
}

/// The server's executions that aren't kept here yet.
#[tauri::command]
pub async fn list_server_executions(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
) -> Result<Vec<serde_json::Value>, String> {
    let summaries = client(&sync)?
        .execution_summaries()
        .await
        .map_err(|e| e.to_string())?;
    let store = state.executions();
    Ok(summaries
        .into_iter()
        .filter(|summary| {
            summary["execution_id"]
                .as_str()
                .and_then(|id| id.parse::<ExecutionId>().ok())
                .is_some_and(|id| store.load(id).is_err())
        })
        .collect())
}

/// Keep a copy of a server execution here, linked, and return it.
#[tauri::command]
pub async fn open_server_execution(
    state: State<'_, AppState>,
    sync: State<'_, ServerSync>,
    execution_id: ExecutionId,
) -> Result<ExecutionSummary, String> {
    let store = state.executions();
    sync.open_remote(&store, execution_id).await?;
    procnote_app::execution::get_execution(&store, execution_id)
}
