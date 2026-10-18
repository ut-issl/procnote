//! The desktop app's sync with a procnote server (migration plan stage 7):
//! the workspace's link, a background loop that syncs linked executions,
//! and the status shown in the UI. The sync itself is `procnote_sync`.

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_sync::client::DeviceClient;
use procnote_sync::engine::{SyncError, sync_all, waiting};
use procnote_sync::link::{Links, ServerLink};
use serde::Serialize;
use ts_rs::TS;

/// How often linked executions are synced besides after each change.
const SYNC_INTERVAL: Duration = Duration::from_secs(30);

/// The server a workspace is connected to (without the token).
#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct ServerInfo {
    pub server: String,
    pub email: String,
    pub device: String,
}

/// What the UI shows about syncing.
#[derive(Debug, Clone, Default, Serialize, TS)]
#[ts(export)]
pub struct SyncStatus {
    #[ts(optional)]
    pub connected: Option<ServerInfo>,
    /// Recorded things not yet on the server.
    pub waiting: u32,
    /// Why the last sync didn't complete: `"unreachable"`, `"unauthorized"`
    /// (connect again), or `"failed"`.
    #[ts(optional)]
    pub problem: Option<String>,
    #[ts(optional)]
    pub message: Option<String>,
    /// ISO 8601 timestamp of the last complete sync.
    #[ts(optional)]
    pub last_synced_at: Option<String>,
}

pub struct ServerSync {
    workspace: PathBuf,
    links_file: PathBuf,
    status: Mutex<SyncStatus>,
    /// One sync at a time.
    running: tokio::sync::Mutex<()>,
    wake: tokio::sync::Notify,
}

impl ServerSync {
    #[must_use]
    pub fn new(workspace: PathBuf, links_file: PathBuf) -> Self {
        Self {
            workspace,
            links_file,
            status: Mutex::new(SyncStatus::default()),
            running: tokio::sync::Mutex::new(()),
            wake: tokio::sync::Notify::new(),
        }
    }

    /// The workspace's link to a server, if connected.
    pub fn link(&self) -> Result<Option<ServerLink>, String> {
        Ok(Links::load(&self.links_file)?.get(&self.workspace).cloned())
    }

    pub fn set_link(&self, link: Option<ServerLink>) -> Result<(), String> {
        let mut links = Links::load(&self.links_file)?;
        match link {
            Some(link) => links.set(&self.workspace, link),
            None => {
                links.remove(&self.workspace);
            }
        }
        links.save(&self.links_file)
    }

    /// Ask the background loop to sync now.
    pub fn wake(&self) {
        self.wake.notify_one();
    }

    /// The status, with a fresh count of what's waiting.
    pub fn status(&self, store: &ExecutionStore) -> SyncStatus {
        let mut status = self
            .status
            .lock()
            .map(|status| status.clone())
            .unwrap_or_default();
        status.connected = self.link().ok().flatten().map(|link| ServerInfo {
            server: link.server,
            email: link.email,
            device: link.device,
        });
        status.waiting = waiting(store)
            .ok()
            .and_then(|count| u32::try_from(count).ok())
            .unwrap_or_default();
        status
    }

    fn record(&self, outcome: Result<usize, SyncError>) {
        let Ok(mut status) = self.status.lock() else {
            return;
        };
        match outcome {
            Ok(_) => {
                status.problem = None;
                status.message = None;
                status.last_synced_at = Some(chrono::Utc::now().to_rfc3339());
            }
            Err(error) => {
                status.problem = Some(
                    match error {
                        SyncError::Unreachable(_) => "unreachable",
                        SyncError::Unauthorized => "unauthorized",
                        SyncError::Failed(_) => "failed",
                    }
                    .to_string(),
                );
                status.message = Some(error.to_string());
            }
        }
    }

    /// Sync every linked execution once, if connected.
    pub async fn sync_now(&self, store: &ExecutionStore) -> SyncStatus {
        let _running = self.running.lock().await;
        match self.link() {
            Ok(Some(link)) => {
                let outcome = sync_all(store, &DeviceClient::new(link)).await;
                if let Err(e) = &outcome {
                    log::info!("sync: {e}");
                }
                self.record(outcome);
            }
            Ok(None) => {}
            Err(e) => self.record(Err(SyncError::Failed(e))),
        }
        self.status(store)
    }

    /// Sync now and then every [`SYNC_INTERVAL`], or sooner when woken.
    pub async fn run(&self, procedures_dir: &Path) {
        let store = ExecutionStore::new(procedures_dir.to_path_buf());
        loop {
            self.sync_now(&store).await;
            let _ = tokio::time::timeout(SYNC_INTERVAL, self.wake.notified()).await;
        }
    }
}
