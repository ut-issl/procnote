use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use procnote_core::event::types::ExecutionId;
use serde::{Deserialize, Serialize};
use url::{Url, form_urlencoded};
use x25519_dalek::{PublicKey, StaticSecret};
use zeroize::Zeroizing;

use crate::drop_point::client::{RemoteTerminal, parse_base_url, same_origin};
use crate::drop_point::crypto::decode_base64url;
use crate::drop_point::secure_fs::{
    atomic_write_private, ensure_private_directory, verify_private_regular_file,
};

const SESSION_STATE_VERSION: u32 = 1;
const MAX_PROTOCOL_BYTES: u64 = 1 << 40;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct InstalledBundleState {
    pub identity: String,
    pub path: PathBuf,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum CompletionOutcome {
    ClosedSuccessfully,
    RemoteAlreadyClosed,
    Expired,
    Failed,
    NotFound,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "phase", rename_all = "snake_case", deny_unknown_fields)]
pub enum SessionPhase {
    Waiting,
    BundleInstalled {
        bundle: InstalledBundleState,
    },
    ClosePending {
        bundle: InstalledBundleState,
    },
    Complete {
        bundle: Option<InstalledBundleState>,
        outcome: CompletionOutcome,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ActiveDropPointSession {
    version: u32,
    pub session_id: String,
    pub base_url: String,
    pub drop_point_id: String,
    pub display_name: String,
    pickup_token: Option<Zeroizing<String>>,
    recipient_private_key: Option<Zeroizing<String>>,
    pub recipient_public_key: String,
    drop_link: Option<Zeroizing<String>>,
    drop_link_with_fragment: Option<Zeroizing<String>>,
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub input_id: String,
    pub expires_at: DateTime<Utc>,
    pub max_bytes: u64,
    pub execution_dir: PathBuf,
    pub workspace_root: PathBuf,
    pub phase: SessionPhase,
}

pub struct NewDropPointSession {
    pub session_id: String,
    pub base_url: String,
    pub drop_point_id: String,
    pub display_name: String,
    pub pickup_token: Zeroizing<String>,
    pub recipient_private_key: Zeroizing<String>,
    pub recipient_public_key: String,
    pub drop_link: Zeroizing<String>,
    pub drop_link_with_fragment: Zeroizing<String>,
    pub execution_id: ExecutionId,
    pub step_id: String,
    pub input_id: String,
    pub expires_at: DateTime<Utc>,
    pub max_bytes: u64,
    pub execution_dir: PathBuf,
    pub workspace_root: PathBuf,
}

impl ActiveDropPointSession {
    #[must_use]
    pub fn new(value: NewDropPointSession) -> Self {
        Self {
            version: SESSION_STATE_VERSION,
            session_id: value.session_id,
            base_url: value.base_url,
            drop_point_id: value.drop_point_id,
            display_name: value.display_name,
            pickup_token: Some(value.pickup_token),
            recipient_private_key: Some(value.recipient_private_key),
            recipient_public_key: value.recipient_public_key,
            drop_link: Some(value.drop_link),
            drop_link_with_fragment: Some(value.drop_link_with_fragment),
            execution_id: value.execution_id,
            step_id: value.step_id,
            input_id: value.input_id,
            expires_at: value.expires_at,
            max_bytes: value.max_bytes,
            execution_dir: value.execution_dir,
            workspace_root: value.workspace_root,
            phase: SessionPhase::Waiting,
        }
    }

    pub fn pickup_token(&self) -> Result<&str, String> {
        self.pickup_token
            .as_deref()
            .map(String::as_str)
            .ok_or_else(|| "DropPoint session no longer has a pickup capability".to_string())
    }

    pub fn recipient_private_key(&self) -> Result<Zeroizing<[u8; 32]>, String> {
        let encoded = self
            .recipient_private_key
            .as_deref()
            .ok_or_else(|| "DropPoint session no longer has a recipient private key".to_string())?;
        decode_key(encoded, "recipient private key")
    }

    pub fn drop_link_with_fragment(&self) -> Result<&str, String> {
        self.drop_link_with_fragment
            .as_deref()
            .map(String::as_str)
            .ok_or_else(|| "DropPoint session no longer has a sender link".to_string())
    }

    #[must_use]
    pub const fn installed_bundle(&self) -> Option<&InstalledBundleState> {
        match &self.phase {
            SessionPhase::BundleInstalled { bundle }
            | SessionPhase::ClosePending { bundle }
            | SessionPhase::Complete {
                bundle: Some(bundle),
                ..
            } => Some(bundle),
            SessionPhase::Waiting | SessionPhase::Complete { bundle: None, .. } => None,
        }
    }

    #[must_use]
    pub fn with_bundle_installed(&self, bundle: InstalledBundleState) -> Self {
        let mut updated = self.clone();
        updated.phase = SessionPhase::BundleInstalled { bundle };
        updated
    }

    pub fn with_close_pending(&self) -> Result<Self, String> {
        let bundle = self.installed_bundle().cloned().ok_or_else(|| {
            "cannot close DropPoint before durable bundle installation".to_string()
        })?;
        let mut updated = self.clone();
        updated.phase = SessionPhase::ClosePending { bundle };
        Ok(updated)
    }

    #[must_use]
    pub fn with_complete(&self, outcome: CompletionOutcome) -> Self {
        let mut updated = self.clone();
        updated.phase = SessionPhase::Complete {
            bundle: self.installed_bundle().cloned(),
            outcome,
        };
        updated.pickup_token = None;
        updated.recipient_private_key = None;
        updated.drop_link = None;
        updated.drop_link_with_fragment = None;
        updated
    }

    #[must_use]
    pub const fn is_resumable(&self) -> bool {
        !matches!(self.phase, SessionPhase::Complete { .. })
    }

    fn validate(&self, expected_workspace_root: &Path) -> Result<(), String> {
        if self.version != SESSION_STATE_VERSION {
            return Err("unsupported DropPoint private-state version".to_string());
        }
        validate_session_id(&self.session_id)?;
        let base_url = parse_base_url(&self.base_url)
            .map_err(|_| "DropPoint private state has an invalid relay origin".to_string())?;
        validate_prefixed_value(&self.drop_point_id, "dp_")?;
        validate_key(&self.recipient_public_key, "recipient public key")?;
        if self.max_bytes == 0 || self.max_bytes > MAX_PROTOCOL_BYTES {
            return Err("DropPoint private state has an invalid max_bytes".to_string());
        }
        if self.step_id.is_empty() || self.input_id.is_empty() {
            return Err("DropPoint private state has an invalid attachment target".to_string());
        }
        if self.workspace_root != expected_workspace_root
            || !self.execution_dir.starts_with(expected_workspace_root)
        {
            return Err("DropPoint private state belongs to another workspace".to_string());
        }

        match &self.phase {
            SessionPhase::Waiting
            | SessionPhase::BundleInstalled { .. }
            | SessionPhase::ClosePending { .. } => {
                let pickup = self.pickup_token.as_deref().ok_or_else(|| {
                    "resumable DropPoint private state is missing its pickup capability".to_string()
                })?;
                validate_prefixed_value(pickup, "pick_")?;
                let private = self.recipient_private_key()?;
                let expected_public = PublicKey::from(&StaticSecret::from(*private)).to_bytes();
                let public = decode_key(&self.recipient_public_key, "recipient public key")?;
                if *public != expected_public {
                    return Err(
                        "DropPoint private-state public and private keys do not match".to_string(),
                    );
                }
                if self.drop_link.is_none() || self.drop_link_with_fragment.is_none() {
                    return Err(
                        "resumable DropPoint private state is missing its sender link".to_string(),
                    );
                }
                validate_sender_links(self, &base_url)?;
            }
            SessionPhase::Complete { .. } => {
                if self.pickup_token.is_some()
                    || self.recipient_private_key.is_some()
                    || self.drop_link.is_some()
                    || self.drop_link_with_fragment.is_some()
                {
                    return Err(
                        "completed DropPoint private state still contains capabilities".to_string(),
                    );
                }
            }
        }

        if let Some(bundle) = self.installed_bundle() {
            validate_identity(&bundle.identity)?;
            let expected_path = self
                .execution_dir
                .join("attachments")
                .join(format!("bundle-{}", self.drop_point_id));
            if !bundle.path.is_absolute() || bundle.path != expected_path {
                return Err(
                    "installed DropPoint bundle path is outside its deterministic destination"
                        .to_string(),
                );
            }
        }
        Ok(())
    }
}

pub struct DropPointSessions {
    state_root: PathBuf,
    workspace_root: PathBuf,
    io_lock: Mutex<()>,
}

impl DropPointSessions {
    pub fn new(state_root: PathBuf, workspace_root: &Path) -> Result<Self, String> {
        let workspace_root = workspace_root
            .canonicalize()
            .map_err(|error| format!("failed to canonicalize workspace root: {error}"))?;
        ensure_private_directory(&state_root)?;
        Ok(Self {
            state_root,
            workspace_root,
            io_lock: Mutex::new(()),
        })
    }

    pub fn insert(&self, session: &ActiveDropPointSession) -> Result<(), String> {
        session.validate(&self.workspace_root)?;
        let _guard = self.io_lock.lock().map_err(|error| error.to_string())?;
        let path = self.session_path(&session.session_id)?;
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Ok(_) => return Err("DropPoint private state already exists".to_string()),
            Err(error) => return Err(error.to_string()),
        }
        persist_session_file(&path, session)
    }

    pub fn persist(&self, session: &ActiveDropPointSession) -> Result<(), String> {
        session.validate(&self.workspace_root)?;
        let _guard = self.io_lock.lock().map_err(|error| error.to_string())?;
        let path = self.session_path(&session.session_id)?;
        let current = self.read_session(&session.session_id)?;
        merge_transition(&current, session)?
            .map_or(Ok(()), |merged| persist_session_file(&path, &merged))
    }

    pub fn get(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let _guard = self.io_lock.lock().map_err(|error| error.to_string())?;
        self.read_session(session_id)
    }

    pub fn has_resumable_sessions(&self) -> Result<bool, String> {
        let _guard = self.io_lock.lock().map_err(|error| error.to_string())?;
        Ok(self
            .session_ids()?
            .into_iter()
            .filter_map(|session_id| self.read_session(&session_id).ok())
            .any(|session| session.is_resumable()))
    }

    pub fn find_for_target(
        &self,
        execution_id: ExecutionId,
        step_id: &str,
        input_id: &str,
    ) -> Result<Option<ActiveDropPointSession>, String> {
        let _guard = self.io_lock.lock().map_err(|error| error.to_string())?;
        let matches = self
            .session_ids()?
            .into_iter()
            .map(|session_id| self.read_session(&session_id))
            .filter_map(|result| match result {
                Ok(session) => Some(Ok(session)),
                Err(error) => {
                    log::warn!("ignored invalid DropPoint private-state file: {error}");
                    None
                }
            })
            .filter(|result| {
                result.as_ref().is_ok_and(|session| {
                    session.execution_id == execution_id
                        && session.step_id == step_id
                        && session.input_id == input_id
                        && session.is_resumable()
                })
            })
            .collect::<Result<Vec<_>, String>>()?;
        match matches.as_slice() {
            [] => Ok(None),
            [session] => Ok(Some(session.clone())),
            _ => Err(
                "multiple resumable DropPoint sessions target this attachment input".to_string(),
            ),
        }
    }

    fn read_session(&self, session_id: &str) -> Result<ActiveDropPointSession, String> {
        let path = self.session_path(session_id)?;
        verify_private_regular_file(&path)?;
        let bytes = Zeroizing::new(std::fs::read(path).map_err(|error| error.to_string())?);
        let session: ActiveDropPointSession = serde_json::from_slice(&bytes)
            .map_err(|_| "DropPoint private-state JSON is invalid".to_string())?;
        session.validate(&self.workspace_root)?;
        Ok(session)
    }

    fn session_ids(&self) -> Result<Vec<String>, String> {
        Ok(std::fs::read_dir(&self.state_root)
            .map_err(|error| error.to_string())?
            .filter_map(|entry| match entry {
                Ok(entry) => Some(entry),
                Err(error) => {
                    log::warn!("failed to inspect DropPoint private-state entry: {error}");
                    None
                }
            })
            .filter_map(|entry| {
                let path = entry.path();
                (path.extension().and_then(|extension| extension.to_str()) == Some("json"))
                    .then(|| {
                        path.file_stem()
                            .and_then(|stem| stem.to_str())
                            .map(str::to_string)
                    })
                    .flatten()
            })
            .collect())
    }

    fn session_path(&self, session_id: &str) -> Result<PathBuf, String> {
        validate_session_id(session_id)?;
        Ok(self.state_root.join(format!("{session_id}.json")))
    }
}

fn merge_transition(
    current: &ActiveDropPointSession,
    proposed: &ActiveDropPointSession,
) -> Result<Option<ActiveDropPointSession>, String> {
    if !same_session_identity(current, proposed) {
        return Err("refusing to replace different DropPoint private state".to_string());
    }
    let current_bundle = current.installed_bundle().cloned();
    let proposed_bundle = proposed.installed_bundle().cloned();
    if current_bundle.is_some() && proposed_bundle.is_some() && current_bundle != proposed_bundle {
        return Err("DropPoint private-state bundle identity conflict".to_string());
    }

    if matches!(current.phase, SessionPhase::Complete { .. }) {
        return match (current_bundle, proposed_bundle) {
            (None, Some(bundle)) => {
                let mut merged = current.clone();
                if let SessionPhase::Complete {
                    bundle: current_bundle,
                    ..
                } = &mut merged.phase
                {
                    *current_bundle = Some(bundle);
                }
                Ok(Some(merged))
            }
            _ => Ok(None),
        };
    }

    let mut merged = proposed.clone();
    if let (
        Some(bundle),
        SessionPhase::Complete {
            bundle: proposed_bundle,
            ..
        },
    ) = (current_bundle, &mut merged.phase)
        && proposed_bundle.is_none()
    {
        *proposed_bundle = Some(bundle);
    }
    if phase_rank(&merged.phase) < phase_rank(&current.phase) {
        Ok(None)
    } else {
        Ok(Some(merged))
    }
}

fn same_session_identity(left: &ActiveDropPointSession, right: &ActiveDropPointSession) -> bool {
    left.version == right.version
        && left.session_id == right.session_id
        && left.base_url == right.base_url
        && left.drop_point_id == right.drop_point_id
        && left.display_name == right.display_name
        && left.recipient_public_key == right.recipient_public_key
        && left.execution_id == right.execution_id
        && left.step_id == right.step_id
        && left.input_id == right.input_id
        && left.expires_at == right.expires_at
        && left.max_bytes == right.max_bytes
        && left.execution_dir == right.execution_dir
        && left.workspace_root == right.workspace_root
}

const fn phase_rank(phase: &SessionPhase) -> u8 {
    match phase {
        SessionPhase::Waiting => 0,
        SessionPhase::BundleInstalled { .. } => 1,
        SessionPhase::ClosePending { .. } => 2,
        SessionPhase::Complete { .. } => 3,
    }
}

fn persist_session_file(path: &Path, session: &ActiveDropPointSession) -> Result<(), String> {
    let mut bytes = Zeroizing::new(
        serde_json::to_vec_pretty(session)
            .map_err(|_| "failed to encode DropPoint private state")?,
    );
    bytes.push(b'\n');
    atomic_write_private(path, &bytes)
}

fn validate_sender_links(session: &ActiveDropPointSession, base_url: &Url) -> Result<(), String> {
    let drop_link = Url::parse(
        session
            .drop_link
            .as_deref()
            .ok_or_else(|| "DropPoint private state is missing its sender link".to_string())?,
    )
    .map_err(|_| "DropPoint private state has an invalid sender link".to_string())?;
    let segments = drop_link
        .path_segments()
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    if !same_origin(base_url, &drop_link)
        || !drop_link.username().is_empty()
        || drop_link.password().is_some()
        || drop_link.query().is_some()
        || drop_link.fragment().is_some()
        || segments.len() != 2
        || segments.first().copied() != Some("drop")
        || segments
            .get(1)
            .is_none_or(|token| validate_prefixed_value(token, "drop_").is_err())
    {
        return Err("DropPoint private state has an invalid sender link".to_string());
    }

    let mut full_link =
        Url::parse(session.drop_link_with_fragment.as_deref().ok_or_else(|| {
            "DropPoint private state is missing its full sender link".to_string()
        })?)
        .map_err(|_| "DropPoint private state has an invalid full sender link".to_string())?;
    let fragment = full_link
        .fragment()
        .ok_or_else(|| "DropPoint private state sender link has no key fragment".to_string())?;
    let pairs = form_urlencoded::parse(fragment.as_bytes()).collect::<Vec<_>>();
    let valid_fragment = matches!(
        pairs.as_slice(),
        [(version_key, version), (public_key, public), (expiry_key, expiry)]
            if version_key == "v"
                && version == "2"
                && public_key == "pk"
                && public == &session.recipient_public_key
                && expiry_key == "exp"
                && DateTime::parse_from_rfc3339(expiry)
                    .is_ok_and(|value| value.with_timezone(&Utc) == session.expires_at)
    );
    full_link.set_fragment(None);
    if !valid_fragment || full_link != drop_link {
        return Err("DropPoint private state sender link fragment is invalid".to_string());
    }
    Ok(())
}

fn decode_key(value: &str, label: &str) -> Result<Zeroizing<[u8; 32]>, String> {
    let bytes = decode_base64url(value).map_err(|_| format!("invalid {label} encoding"))?;
    let key =
        <[u8; 32]>::try_from(bytes).map_err(|_| format!("{label} must contain 32 raw bytes"))?;
    Ok(Zeroizing::new(key))
}

fn validate_key(value: &str, label: &str) -> Result<(), String> {
    decode_key(value, label).map(|_| ())
}

fn validate_session_id(value: &str) -> Result<(), String> {
    let parsed =
        uuid::Uuid::parse_str(value).map_err(|_| "DropPoint session ID is invalid".to_string())?;
    if parsed.to_string() == value {
        Ok(())
    } else {
        Err("DropPoint session ID is not canonical".to_string())
    }
}

fn validate_prefixed_value(value: &str, prefix: &str) -> Result<(), String> {
    if value.strip_prefix(prefix).is_some_and(|suffix| {
        !suffix.is_empty()
            && suffix
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    }) {
        Ok(())
    } else {
        Err("DropPoint private state contains an invalid capability or ID".to_string())
    }
}

fn validate_identity(value: &str) -> Result<(), String> {
    if value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("DropPoint private state contains an invalid bundle identity".to_string())
    }
}

impl From<RemoteTerminal> for CompletionOutcome {
    fn from(value: RemoteTerminal) -> Self {
        match value {
            RemoteTerminal::Closed => Self::RemoteAlreadyClosed,
            RemoteTerminal::Expired => Self::Expired,
            RemoteTerminal::Failed => Self::Failed,
            RemoteTerminal::NotFound => Self::NotFound,
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use crate::drop_point::crypto::{encode_base64url, generate_recipient_key_pair};

    use super::*;

    fn session(workspace: &Path) -> ActiveDropPointSession {
        let workspace = workspace.canonicalize().unwrap();
        let (private, public) = generate_recipient_key_pair();
        let public = encode_base64url(&public);
        let expires_at = Utc::now() + chrono::Duration::minutes(10);
        let fragment = form_urlencoded::Serializer::new(String::new())
            .append_pair("v", "2")
            .append_pair("pk", &public)
            .append_pair(
                "exp",
                &expires_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            )
            .finish();
        let execution_dir = workspace.join("procedure/.executions/execution");
        std::fs::create_dir_all(&execution_dir).unwrap();
        ActiveDropPointSession::new(NewDropPointSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            base_url: "https://drop.example.com".to_string(),
            drop_point_id: "dp_example".to_string(),
            display_name: "calm-otter".to_string(),
            pickup_token: Zeroizing::new("pick_example".to_string()),
            recipient_private_key: Zeroizing::new(encode_base64url(&*private)),
            recipient_public_key: public,
            drop_link: Zeroizing::new("https://drop.example.com/drop/drop_example".to_string()),
            drop_link_with_fragment: Zeroizing::new(format!(
                "https://drop.example.com/drop/drop_example#{fragment}"
            )),
            execution_id: ExecutionId::new_v4(),
            step_id: "step".to_string(),
            input_id: "input".to_string(),
            expires_at,
            max_bytes: 1024,
            execution_dir,
            workspace_root: workspace,
        })
    }

    #[test]
    fn private_state_survives_restart_and_is_owner_only() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let session = session(&workspace);
        sessions.insert(&session).unwrap();
        drop(sessions);

        let restarted = DropPointSessions::new(state_root, &workspace).unwrap();
        assert!(restarted.has_resumable_sessions().unwrap());
        let loaded = restarted.get(&session.session_id).unwrap();
        assert_eq!(loaded.drop_point_id, session.drop_point_id);
        assert_eq!(
            *loaded.recipient_private_key().unwrap(),
            *session.recipient_private_key().unwrap()
        );
    }

    #[test]
    fn close_pending_is_resumable_without_reinstalling() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let original = session(&workspace);
        sessions.insert(&original).unwrap();
        let bundle = InstalledBundleState {
            identity: "a".repeat(64),
            path: original.execution_dir.join("attachments/bundle-dp_example"),
        };
        let close_pending = original
            .with_bundle_installed(bundle.clone())
            .with_close_pending()
            .unwrap();
        sessions.persist(&close_pending).unwrap();
        drop(sessions);

        let restarted = DropPointSessions::new(state_root, &workspace).unwrap();
        let loaded = restarted.get(&original.session_id).unwrap();
        assert!(matches!(
            loaded.phase,
            SessionPhase::ClosePending { bundle: ref loaded_bundle } if *loaded_bundle == bundle
        ));
        assert!(loaded.recipient_private_key().is_ok());
    }

    #[test]
    fn terminal_transition_is_durable_and_scrubs_capabilities() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let original = session(&workspace);
        let pickup_token = original.pickup_token().unwrap().to_string();
        let private_key = original.recipient_private_key.as_deref().unwrap().clone();
        let sender_link = original.drop_link_with_fragment().unwrap().to_string();
        sessions.insert(&original).unwrap();
        let terminal = original.with_complete(CompletionOutcome::Expired);
        sessions.persist(&terminal).unwrap();

        let loaded = sessions.get(&original.session_id).unwrap();
        assert!(loaded.pickup_token().is_err());
        assert!(loaded.recipient_private_key().is_err());
        assert!(loaded.drop_link_with_fragment().is_err());
        let persisted =
            std::fs::read_to_string(state_root.join(format!("{}.json", original.session_id)))
                .unwrap();
        assert!(!persisted.contains(&pickup_token));
        assert!(!persisted.contains(&private_key));
        assert!(!persisted.contains(&sender_link));
        assert!(!sessions.has_resumable_sessions().unwrap());
    }

    #[test]
    fn stale_transitions_cannot_erase_installed_bundle_recovery_state() {
        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root, &workspace).unwrap();
        let waiting = session(&workspace);
        sessions.insert(&waiting).unwrap();
        let bundle = InstalledBundleState {
            identity: "b".repeat(64),
            path: waiting.execution_dir.join("attachments/bundle-dp_example"),
        };
        sessions
            .persist(&waiting.with_bundle_installed(bundle.clone()))
            .unwrap();

        sessions.persist(&waiting).unwrap();
        assert_eq!(
            sessions
                .get(&waiting.session_id)
                .unwrap()
                .installed_bundle(),
            Some(&bundle)
        );

        sessions
            .persist(&waiting.with_complete(CompletionOutcome::Expired))
            .unwrap();
        let terminal = sessions.get(&waiting.session_id).unwrap();
        assert!(matches!(terminal.phase, SessionPhase::Complete { .. }));
        assert_eq!(terminal.installed_bundle(), Some(&bundle));
        assert!(terminal.recipient_private_key().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn state_file_is_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let session = session(&workspace);
        sessions.insert(&session).unwrap();
        let metadata =
            std::fs::metadata(state_root.join(format!("{}.json", session.session_id))).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o077, 0);
    }
}
