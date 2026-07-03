use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use procnote_core::event::types::ExecutionId;
use zeroize::Zeroizing;

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
}

#[derive(Default)]
pub struct DropPointSessions {
    sessions: Mutex<HashMap<String, ActiveDropPointSession>>,
}

impl DropPointSessions {
    pub fn insert(&self, session: ActiveDropPointSession) -> Result<(), String> {
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .insert(session.session_id.clone(), session);
        Ok(())
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "bottom DropPoint branch holds the lock only for in-memory session lookup"
    )]
    pub fn get(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("DropPoint session not found: {session_id}"))?;
        if session.expires_at <= Utc::now() {
            sessions.remove(session_id);
            return Err(format!("DropPoint session expired: {session_id}"));
        }
        Ok(session)
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "bottom DropPoint branch holds the lock only for in-memory session removal"
    )]
    pub fn take(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session = sessions
            .remove(session_id)
            .ok_or_else(|| format!("DropPoint session not found: {session_id}"))?;
        if session.expires_at <= Utc::now() {
            return Err(format!("DropPoint session expired: {session_id}"));
        }
        Ok(session)
    }

    #[expect(
        clippy::significant_drop_tightening,
        reason = "bottom DropPoint branch holds the lock only for in-memory session removal"
    )]
    pub fn remove_for_target(
        &self,
        execution_id: ExecutionId,
        step_id: &str,
        input_id: &str,
    ) -> Result<Option<ActiveDropPointSession>, String> {
        let mut sessions = self.sessions.lock().map_err(|e| e.to_string())?;
        let session_id = sessions.iter().find_map(|(session_id, session)| {
            (session.execution_id == execution_id
                && session.step_id == step_id
                && session.input_id == input_id)
                .then(|| session_id.clone())
        });
        Ok(session_id.and_then(|session_id| sessions.remove(&session_id)))
    }
}
