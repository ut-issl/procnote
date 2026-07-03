use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use procnote_core::event::types::ExecutionId;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::drop_point::client::DropPointClient;

const SESSION_DIR: &str = ".drop_point_sessions";

#[derive(Clone)]
pub struct ActiveDropPointSession {
    pub session_id: String,
    pub drop_point_id: String,
    pub pickup_token: String,
    pub recipient_private_key: Arc<Zeroizing<[u8; 32]>>,
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub input_id: String,
    pub expires_at: DateTime<Utc>,
    pub execution_dir: PathBuf,
}

#[derive(Debug, Serialize, Deserialize)]
struct PersistedDropPointSession {
    session_id: String,
    drop_point_id: String,
    pickup_token: String,
    expires_at: DateTime<Utc>,
}

#[derive(Default)]
pub struct DropPointSessions {
    sessions: Mutex<HashMap<String, ActiveDropPointSession>>,
}

impl DropPointSessions {
    pub fn insert(&self, session: ActiveDropPointSession) -> Result<(), String> {
        persist_session(&session)?;
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    pub fn get(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let (session, expired) = {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            let session = sessions
                .get(session_id)
                .cloned()
                .ok_or_else(|| format!("DropPoint session not found: {session_id}"))?;
            let expired = session.expires_at <= Utc::now();
            if expired {
                sessions.remove(session_id);
            }
            drop(sessions);
            (session, expired)
        };

        if expired {
            delete_persisted_session(&session)?;
            return Err(format!("DropPoint session expired: {session_id}"));
        }
        Ok(session)
    }

    pub fn take(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let session = {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            sessions
                .remove(session_id)
                .ok_or_else(|| format!("DropPoint session not found: {session_id}"))?
        };

        if session.expires_at <= Utc::now() {
            delete_persisted_session(&session)?;
            return Err(format!("DropPoint session expired: {session_id}"));
        }
        Ok(session)
    }

    pub fn remove_for_target(
        &self,
        execution_id: ExecutionId,
        step_id: &str,
        input_id: &str,
    ) -> Result<Option<ActiveDropPointSession>, String> {
        let session = {
            let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
            let session_id = sessions.iter().find_map(|(session_id, session)| {
                (session.execution_id == execution_id
                    && session.step_id == step_id
                    && session.input_id == input_id)
                    .then(|| session_id.clone())
            });
            let session = session_id.and_then(|session_id| sessions.remove(&session_id));
            drop(sessions);
            session
        };
        Ok(session)
    }
}

impl ActiveDropPointSession {
    pub fn delete_persisted(&self) -> Result<(), String> {
        delete_persisted_session(self)
    }
}

pub async fn cleanup_persisted_sessions(procedures_dir: &Path, client: &DropPointClient) {
    let sessions = match persisted_sessions(procedures_dir) {
        Ok(sessions) => sessions,
        Err(e) => {
            log::warn!("failed to scan persisted DropPoint sessions: {e}");
            return;
        }
    };

    for persisted in sessions {
        if persisted.expires_at > Utc::now()
            && let Err(e) = client
                .close(&persisted.drop_point_id, &persisted.pickup_token)
                .await
        {
            log::warn!(
                "failed to close persisted DropPoint session {} during startup cleanup: {e}",
                persisted.session_id
            );
            continue;
        }
        if let Err(e) = std::fs::remove_file(&persisted.path) {
            log::warn!(
                "failed to remove persisted DropPoint session {}: {e}",
                persisted.path.display()
            );
        }
    }
}

struct PersistedSessionFile {
    path: PathBuf,
    session_id: String,
    drop_point_id: String,
    pickup_token: String,
    expires_at: DateTime<Utc>,
}

fn persisted_sessions(procedures_dir: &Path) -> Result<Vec<PersistedSessionFile>, String> {
    if !procedures_dir.exists() {
        return Ok(Vec::new());
    }

    let mut sessions = Vec::new();
    for proc_entry in std::fs::read_dir(procedures_dir).map_err(|e| e.to_string())? {
        let Ok(proc_entry) = proc_entry else {
            continue;
        };
        let exec_base = proc_entry.path().join(".executions");
        let Ok(exec_entries) = std::fs::read_dir(exec_base) else {
            continue;
        };
        for exec_entry in exec_entries.flatten() {
            let session_dir = exec_entry.path().join(SESSION_DIR);
            let Ok(session_entries) = std::fs::read_dir(session_dir) else {
                continue;
            };
            for session_entry in session_entries.flatten() {
                let path = session_entry.path();
                if path.extension().and_then(|ext| ext.to_str()) != Some("json") {
                    continue;
                }
                match read_persisted_session(&path) {
                    Ok(session) => sessions.push(PersistedSessionFile {
                        path,
                        session_id: session.session_id,
                        drop_point_id: session.drop_point_id,
                        pickup_token: session.pickup_token,
                        expires_at: session.expires_at,
                    }),
                    Err(e) => log::warn!(
                        "failed to read persisted DropPoint session {}: {e}",
                        path.display()
                    ),
                }
            }
        }
    }
    Ok(sessions)
}

fn persist_session(session: &ActiveDropPointSession) -> Result<(), String> {
    let session_dir = session.execution_dir.join(SESSION_DIR);
    std::fs::create_dir_all(&session_dir).map_err(|e| e.to_string())?;
    let path = session_path(&session.execution_dir, &session.session_id);
    let temp_path = path.with_extension(format!("json.tmp-{}", uuid::Uuid::new_v4()));
    let persisted = PersistedDropPointSession {
        session_id: session.session_id.clone(),
        drop_point_id: session.drop_point_id.clone(),
        pickup_token: session.pickup_token.clone(),
        expires_at: session.expires_at,
    };
    let bytes = serde_json::to_vec(&persisted).map_err(|e| e.to_string())?;
    let mut file = std::fs::OpenOptions::new()
        .create_new(true)
        .write(true)
        .open(&temp_path)
        .map_err(|e| e.to_string())?;
    file.write_all(&bytes).map_err(|e| e.to_string())?;
    file.write_all(b"\n").map_err(|e| e.to_string())?;
    file.flush().map_err(|e| e.to_string())?;
    file.sync_all().map_err(|e| e.to_string())?;
    std::fs::rename(&temp_path, &path).map_err(|e| e.to_string())?;
    sync_dir(&session_dir).map_err(|e| e.to_string())?;
    Ok(())
}

fn read_persisted_session(path: &Path) -> Result<PersistedDropPointSession, String> {
    let source = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&source).map_err(|e| e.to_string())
}

fn delete_persisted_session(session: &ActiveDropPointSession) -> Result<(), String> {
    let path = session_path(&session.execution_dir, &session.session_id);
    match std::fs::remove_file(&path) {
        Ok(()) => {
            sync_dir(path.parent().unwrap_or(&session.execution_dir)).map_err(|e| e.to_string())
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

fn session_path(execution_dir: &Path, session_id: &str) -> PathBuf {
    execution_dir
        .join(SESSION_DIR)
        .join(format!("{session_id}.json"))
}

#[cfg(not(windows))]
fn sync_dir(path: &Path) -> Result<(), std::io::Error> {
    std::fs::File::open(path)?.sync_all()
}

#[cfg(windows)]
fn sync_dir(path: &Path) -> Result<(), std::io::Error> {
    use std::os::windows::fs::OpenOptionsExt;

    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;

    std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(FILE_FLAG_BACKUP_SEMANTICS)
        .open(path)?
        .sync_all()
}
