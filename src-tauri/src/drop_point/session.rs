use std::collections::HashMap;
use std::sync::Mutex;

use procnote_core::event::types::ExecutionId;

#[derive(Clone)]
pub struct ActiveDropPointSession {
    pub session_id: String,
    pub drop_point_id: String,
    pub pickup_token: String,
    pub recipient_private_key: [u8; 32],
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub input_id: String,
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

    pub fn get(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        self.sessions
            .lock()
            .map_err(|e| e.to_string())?
            .get(session_id)
            .cloned()
            .ok_or_else(|| format!("DropPoint session not found: {session_id}"))
    }

    pub fn remove(&self, session_id: &str) -> Result<Option<ActiveDropPointSession>, String> {
        Ok(self
            .sessions
            .lock()
            .map_err(|e| e.to_string())?
            .remove(session_id))
    }
}
