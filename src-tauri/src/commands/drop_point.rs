use std::io::Read;
use std::path::PathBuf;

use chrono::{DateTime, Utc};
use procnote_core::event::types::ExecutionId;
use procnote_core::execution::{ExecutionState, ExecutionStatus, ExecutionStepContent, StepStatus};
use procnote_core::template::types::InputType;
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;
use tauri::State;
use ts_rs::TS;
use url::Url;
use url::form_urlencoded;
use zeroize::Zeroizing;

use crate::drop_point::client::{
    CreateDropPointResponse, DropPointClient, DropPointConfig, DropPointStatusResponse,
    RemoteTerminal,
};
use crate::drop_point::crypto::{decrypt_bundle, encode_base64url, generate_recipient_key_pair};
use crate::drop_point::multipart::parse_pickup_multipart;
use crate::drop_point::session::{
    ActiveDropPointSession, CompletionOutcome, DropPointSessions, InstalledBundleState,
    NewDropPointSession, SessionPhase,
};
use crate::drop_point::storage::{
    InstalledBundle, discover_installed_bundles, encrypted_bundle_identity, install_bundle,
    verify_installed_bundle,
};
use crate::state::AppState;
use procnote_app::execution::{load_execution_from_disk, summarize};
use procnote_app::persistence::execution_store::{ExecutionStore, InstalledAttachmentSource};

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AttachmentDropPointSessionSummary {
    pub session_id: String,
    pub display_name: String,
    pub qr_url: String,
    pub qr_svg: String,
    pub expires_at: String,
    pub max_bytes: u64,
}

#[derive(Debug, Serialize, TS)]
#[ts(export)]
pub struct AttachmentDropPointStatus {
    pub status: String,
    pub display_name: String,
    pub pending_submissions: u64,
    pub pending_bytes: u64,
    pub expires_at: String,
    #[ts(optional)]
    pub needs_import: Option<bool>,
}

#[derive(Debug, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum AttachmentDropPointPollError {
    Retryable { message: String },
    Terminal { message: String },
    Fatal { message: String },
}

impl From<String> for AttachmentDropPointPollError {
    fn from(message: String) -> Self {
        Self::Fatal { message }
    }
}

#[tauri::command]
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn is_drop_point_configured(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
) -> bool {
    state.drop_point_client.is_some()
        || sessions.has_resumable_sessions().unwrap_or_else(|error| {
            log::warn!("failed to inspect resumable DropPoint private state: {error}");
            false
        })
}

#[tauri::command]
pub async fn start_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    execution_id: ExecutionId,
    step_id: String,
    input_id: String,
) -> Result<AttachmentDropPointSessionSummary, String> {
    let _operation = sessions.lock_operation().await;
    let (execution_state, log_path) =
        load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let execution_dir = log_path
        .parent()
        .ok_or_else(|| "event log path has no parent".to_string())?
        .to_path_buf();
    validate_attachment_target(&execution_state, &step_id, &input_id)?;

    if let Some(existing) = sessions.find_for_target(execution_id, &step_id, &input_id)? {
        return session_summary(&existing);
    }

    let config = configured(&state)?;
    let client = configured_client(&state)?;
    let (recipient_private_key, recipient_public_key) = generate_recipient_key_pair();
    let created = client
        .create_drop_point()
        .await
        .map_err(|error| error.to_string())?;
    let target = SessionInputTarget { step_id, input_id };
    let setup = build_session(
        &config,
        created,
        &recipient_private_key,
        recipient_public_key,
        execution_id,
        target,
        execution_dir,
        state.procedures_dir.clone(),
    );

    match setup {
        Ok((session, summary)) => sessions.insert(&session).map(|()| summary),
        Err(error) => {
            close_after_setup_failure(
                &client,
                &error.drop_point_id,
                Ok(error.pickup_token.as_str()),
            )
            .await;
            Err(error.reason)
        }
    }
}

#[tauri::command]
pub async fn poll_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    session_id: String,
) -> Result<AttachmentDropPointStatus, AttachmentDropPointPollError> {
    let _operation = sessions.lock_operation().await;
    let session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
    if matches!(session.phase, SessionPhase::ClosePending) {
        close_session(&client, &sessions, &state.procedures_dir, session.clone()).await?;
        return Ok(AttachmentDropPointStatus {
            status: "closed".to_string(),
            display_name: session.display_name,
            pending_submissions: 0,
            pending_bytes: 0,
            expires_at: session.expires_at.to_rfc3339(),
            needs_import: Some(true),
        });
    }
    if !session.is_resumable() {
        return Err(AttachmentDropPointPollError::Terminal {
            message: "DropPoint session is already terminal".to_string(),
        });
    }

    let status = match client
        .status(&session.drop_point_id, session.pickup_token()?)
        .await
    {
        Ok(status) => status,
        Err(error) => {
            if let Some(terminal) = error.terminal() {
                finalize_terminal_session(&sessions, &state.procedures_dir, &session, terminal)?;
                return Err(AttachmentDropPointPollError::Terminal {
                    message: error.to_string(),
                });
            }
            return if error.is_retryable() {
                Err(AttachmentDropPointPollError::Retryable {
                    message: error.to_string(),
                })
            } else {
                Err(AttachmentDropPointPollError::Fatal {
                    message: error.to_string(),
                })
            };
        }
    };
    validate_status_identity(&session, &status)?;
    if let Some(terminal) = status.status.terminal() {
        finalize_terminal_session(&sessions, &state.procedures_dir, &session, terminal)?;
    }

    Ok(AttachmentDropPointStatus {
        status: status.status.as_str().to_string(),
        display_name: status.display_name,
        pending_submissions: status.pending_submissions,
        pending_bytes: status.pending_bytes,
        expires_at: status.expires_at,
        needs_import: session.pending_local_imports().then_some(true),
    })
}

#[tauri::command]
pub async fn import_attachment_drop_point_upload(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    execution_id: ExecutionId,
    step_id: String,
    input_id: String,
    session_id: String,
) -> Result<procnote_app::execution::ExecutionSummary, String> {
    let _operation = sessions.lock_operation().await;
    let mut session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
    ensure_session_target(&session, execution_id, &step_id, &input_id)?;
    let current_state = validate_session_execution_dir(&state.procedures_dir, &session)?;
    if matches!(session.phase, SessionPhase::Waiting) {
        validate_attachment_target(&current_state, &step_id, &input_id)?;
    }
    record_pending_submissions(&state.procedures_dir, &sessions, &mut session)?;
    if session.is_resumable() && matches!(session.phase, SessionPhase::Waiting) {
        // Retry local acknowledgements even when a previous ACK succeeded remotely
        // but its response or the following private-state write was lost.
        let pending = session
            .submissions
            .iter()
            .filter(|(_, child)| !child.acknowledged)
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        for id in pending {
            acknowledge_submission(&client, &sessions, &mut session, &id).await?;
        }
        let listed = match client
            .list_submissions(&session.drop_point_id, session.pickup_token()?)
            .await
        {
            Ok(listed) => listed,
            Err(error) => {
                if let Some(terminal) = error.terminal() {
                    finalize_terminal_session(
                        &sessions,
                        &state.procedures_dir,
                        &session,
                        terminal,
                    )?;
                }
                return Err(error.to_string());
            }
        };
        let mut first_error = None;
        for child in listed {
            if !session.submissions.contains_key(&child.submission_id) {
                match install_submission(
                    &client,
                    &sessions,
                    &state.procedures_dir,
                    session.clone(),
                    &child.submission_id,
                )
                .await
                {
                    Ok(installed) => session = installed,
                    Err(error) => {
                        first_error.get_or_insert(error);
                        session = sessions.get(&session_id)?;
                        if !session.is_resumable() {
                            break;
                        }
                        continue;
                    }
                }
            }
            record_one_submission(
                &state.procedures_dir,
                &sessions,
                &mut session,
                &child.submission_id,
            )?;
            if !session.submissions[&child.submission_id].acknowledged {
                acknowledge_submission(&client, &sessions, &mut session, &child.submission_id)
                    .await?;
            }
        }
        if let Some(error) = first_error {
            return Err(error);
        }
    }
    let (current, log_path) = load_execution_from_disk(&state.procedures_dir, execution_id)?;
    let execution_dir = log_path
        .parent()
        .ok_or_else(|| "event log path has no parent".to_string())?;
    summarize(&current, Some(&execution_dir.display().to_string()))
}

async fn acknowledge_submission(
    client: &DropPointClient,
    sessions: &DropPointSessions,
    session: &mut ActiveDropPointSession,
    submission_id: &str,
) -> Result<(), String> {
    if !session
        .submissions
        .get(submission_id)
        .is_some_and(|child| child.recorded)
    {
        return Err("cannot acknowledge an unrecorded DropPoint submission".to_string());
    }
    client
        .acknowledge(
            &session.drop_point_id,
            submission_id,
            session.pickup_token()?,
        )
        .await
        .map_err(|e| e.to_string())?;
    *session = session.with_submission_acknowledged(submission_id)?;
    sessions.persist(session)
}

#[tauri::command]
pub async fn cancel_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    session_id: String,
) -> Result<(), String> {
    let _operation = sessions.lock_operation().await;
    let session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
    close_session(&client, &sessions, &state.procedures_dir, session).await
}

async fn close_session(
    client: &DropPointClient,
    sessions: &DropPointSessions,
    procedures_dir: &std::path::Path,
    mut session: ActiveDropPointSession,
) -> Result<(), String> {
    if !session.is_resumable() {
        return Ok(());
    }
    record_pending_submissions(procedures_dir, sessions, &mut session)?;
    session = session.with_close_pending();
    sessions.persist(&session)?;
    match client
        .close(&session.drop_point_id, session.pickup_token()?)
        .await
    {
        Ok(()) => sessions.persist(&session.with_complete(CompletionOutcome::ClosedSuccessfully)),
        Err(error) => error.terminal().map_or_else(
            || Err(error.to_string()),
            |terminal| finalize_terminal_session(sessions, procedures_dir, &session, terminal),
        ),
    }
}

async fn install_submission(
    client: &DropPointClient,
    sessions: &DropPointSessions,
    procedures_dir: &std::path::Path,
    session: ActiveDropPointSession,
    submission_id: &str,
) -> Result<ActiveDropPointSession, String> {
    let pickup = client
        .pickup(
            &session.drop_point_id,
            submission_id,
            session.pickup_token()?,
            session.max_bytes,
        )
        .await;
    let (content_type, body) = match pickup {
        Ok(pickup) => pickup,
        Err(error) => {
            if let Some(terminal) = error.terminal() {
                finalize_terminal_session(sessions, procedures_dir, &session, terminal)?;
                return Err(error.to_string());
            }
            if error.is_not_ready() {
                return Err("DropPoint pickup is not ready; resume polling".to_string());
            }
            if error.is_retryable() {
                return Err(format!("retryable DropPoint pickup failure: {error}"));
            }
            return Err(error.to_string());
        }
    };
    let (envelope_json, encrypted_payload) =
        parse_pickup_multipart(&content_type, &body, session.max_bytes)
            .map_err(|error| error.to_string())?;
    let identity = encrypted_bundle_identity(&envelope_json, &encrypted_payload)
        .map_err(|error| error.to_string())?;
    let private_key = session.recipient_private_key()?;
    let recovered = decrypt_bundle(&private_key, &envelope_json, &encrypted_payload)
        .map_err(|error| error.to_string())?;
    let installed = install_bundle(
        &session.execution_dir,
        &session.drop_point_id,
        submission_id,
        &identity,
        &recovered,
    )
    .map_err(|error| error.to_string())?;
    drop(recovered);
    let bundle = InstalledBundleState {
        identity: installed.identity,
        path: installed.path,
    };
    let updated = session.with_bundle_installed(submission_id.to_string(), bundle);
    sessions.persist(&updated)?;
    Ok(updated)
}

fn verify_session_bundle(
    session: &ActiveDropPointSession,
    submission_id: &str,
) -> Result<InstalledBundle, String> {
    let bundle = session
        .submissions
        .get(submission_id)
        .map(|s| &s.bundle)
        .ok_or_else(|| "DropPoint session has no installed bundle receipt".to_string())?;
    verify_installed_bundle(
        &bundle.path,
        &session.drop_point_id,
        submission_id,
        &bundle.identity,
    )
    .map_err(|error| error.to_string())
}

fn validate_session_execution_dir(
    procedures_dir: &std::path::Path,
    session: &ActiveDropPointSession,
) -> Result<ExecutionState, String> {
    let (execution_state, log_path) =
        load_execution_from_disk(procedures_dir, session.execution_id)?;
    let current_dir = log_path
        .parent()
        .ok_or_else(|| "event log path has no parent".to_string())?
        .canonicalize()
        .map_err(|error| format!("failed to canonicalize execution directory: {error}"))?;
    let persisted_dir = session.execution_dir.canonicalize().map_err(|error| {
        format!("failed to canonicalize persisted execution directory: {error}")
    })?;
    if current_dir != persisted_dir {
        return Err("DropPoint session destination does not match the execution".to_string());
    }
    Ok(execution_state)
}

fn record_one_submission(
    procedures_dir: &std::path::Path,
    sessions: &DropPointSessions,
    session: &mut ActiveDropPointSession,
    submission_id: &str,
) -> Result<Option<procnote_app::execution::ExecutionSummary>, String> {
    if session
        .submissions
        .get(submission_id)
        .is_some_and(|s| s.recorded)
    {
        return Ok(None);
    }
    validate_session_execution_dir(procedures_dir, session)?;
    let installed = verify_session_bundle(session, submission_id)?;
    let sources = installed
        .files
        .iter()
        .map(|file| {
            Ok(InstalledAttachmentSource {
                filename: file.filename.clone(),
                relative_path: file.relative_path.clone(),
                content_type: detected_safe_content_type(&installed.path.join(&file.filename))?,
                sha256: file.sha256.clone(),
            })
        })
        .collect::<Result<Vec<_>, String>>()?;
    let recorded = ExecutionStore::new(procedures_dir.to_path_buf())
        .record_installed_attachment_batch(
            session.execution_id,
            &session.step_id,
            &session.input_id,
            sources,
        )?;
    let summary = summarize(
        &recorded.state,
        Some(&recorded.execution_dir.display().to_string()),
    )?;
    *session = session.with_submission_recorded(submission_id)?;
    sessions.persist(session)?;
    Ok(Some(summary))
}

fn record_pending_submissions(
    procedures_dir: &std::path::Path,
    sessions: &DropPointSessions,
    session: &mut ActiveDropPointSession,
) -> Result<Option<procnote_app::execution::ExecutionSummary>, String> {
    validate_session_execution_dir(procedures_dir, session)?;
    for (id, installed) in
        discover_installed_bundles(&session.execution_dir, &session.drop_point_id)
            .map_err(|error| error.to_string())?
    {
        if !session.submissions.contains_key(&id) {
            *session = session.with_bundle_installed(
                id,
                InstalledBundleState {
                    identity: installed.identity,
                    path: installed.path,
                },
            );
            sessions.persist(session)?;
        }
    }
    let ids = session.submissions.keys().cloned().collect::<Vec<_>>();
    let mut summary = None;
    for id in ids {
        summary = record_one_submission(procedures_dir, sessions, session, &id)?.or(summary);
    }
    Ok(summary)
}

fn detected_safe_content_type(path: &std::path::Path) -> Result<String, String> {
    let mut file = std::fs::File::open(path).map_err(|error| error.to_string())?;
    let mut header = [0u8; 12];
    let read = file.read(&mut header).map_err(|error| error.to_string())?;
    let header = &header[..read];
    let content_type = if header.starts_with(&[0xFF, 0xD8, 0xFF]) {
        "image/jpeg"
    } else if header.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if header.starts_with(b"GIF87a") || header.starts_with(b"GIF89a") {
        "image/gif"
    } else if header.len() >= 12 && &header[..4] == b"RIFF" && &header[8..12] == b"WEBP" {
        "image/webp"
    } else if header.starts_with(b"BM") {
        "image/bmp"
    } else {
        "application/octet-stream"
    };
    Ok(content_type.to_string())
}

fn finalize_terminal_session(
    sessions: &DropPointSessions,
    procedures_dir: &std::path::Path,
    session: &ActiveDropPointSession,
    terminal: RemoteTerminal,
) -> Result<(), String> {
    let mut current = sessions.get(&session.session_id)?;
    record_pending_submissions(procedures_dir, sessions, &mut current)?;
    sessions.persist(&current.with_complete(terminal.into()))
}

fn validate_status_identity(
    session: &ActiveDropPointSession,
    status: &DropPointStatusResponse,
) -> Result<(), String> {
    let status_expiry = parse_server_datetime(&status.expires_at)?;
    if status.display_name != session.display_name
        || status_expiry != session.expires_at
        || status.pending_bytes > status.max_pending_bytes
    {
        return Err(
            "DropPoint status response does not match persisted receiver state".to_string(),
        );
    }
    Ok(())
}

async fn close_after_setup_failure(
    client: &DropPointClient,
    drop_point_id: &str,
    pickup_token: Result<&str, String>,
) {
    let Ok(pickup_token) = pickup_token else {
        return;
    };
    if let Err(error) = client.close(drop_point_id, pickup_token).await {
        log::warn!("DropPoint close failed after local session setup failed: {error}");
    }
}

struct SessionSetupError {
    drop_point_id: String,
    pickup_token: Zeroizing<String>,
    reason: String,
}

struct SessionInputTarget {
    step_id: String,
    input_id: String,
}

#[expect(
    clippy::too_many_arguments,
    reason = "constructs one complete durable receiver state"
)]
fn build_session(
    config: &DropPointConfig,
    created: CreateDropPointResponse,
    recipient_private_key: &Zeroizing<[u8; 32]>,
    recipient_public_key: [u8; 32],
    execution_id: ExecutionId,
    target: SessionInputTarget,
    execution_dir: PathBuf,
    workspace_root: PathBuf,
) -> Result<(ActiveDropPointSession, AttachmentDropPointSessionSummary), SessionSetupError> {
    let drop_point_id = created.drop_point_id;
    let pickup_token = created.pickup_token;
    let setup = (|| {
        let expires_at = parse_server_datetime(&created.expires_at)?;
        let drop_link_with_fragment = drop_link_with_fragment(
            &config.base_url,
            &created.drop_link,
            &recipient_public_key,
            &created.expires_at,
        )?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = ActiveDropPointSession::new(NewDropPointSession {
            session_id,
            base_url: config.base_url.as_str().trim_end_matches('/').to_string(),
            drop_point_id: drop_point_id.clone(),
            display_name: created.display_name,
            pickup_token: pickup_token.clone(),
            recipient_private_key: Zeroizing::new(encode_base64url(&**recipient_private_key)),
            recipient_public_key: encode_base64url(&recipient_public_key),
            drop_link: Zeroizing::new(created.drop_link),
            drop_link_with_fragment: Zeroizing::new(drop_link_with_fragment),
            execution_id,
            step_id: target.step_id,
            input_id: target.input_id,
            expires_at,
            max_bytes: created.max_bytes,
            execution_dir,
            workspace_root,
        });
        let summary = session_summary(&session)?;
        Ok((session, summary))
    })();

    setup.map_err(|reason| SessionSetupError {
        drop_point_id,
        pickup_token,
        reason,
    })
}

fn session_summary(
    session: &ActiveDropPointSession,
) -> Result<AttachmentDropPointSessionSummary, String> {
    let qr_url = session.drop_link_with_fragment()?.to_string();
    Ok(AttachmentDropPointSessionSummary {
        session_id: session.session_id.clone(),
        display_name: session.display_name.clone(),
        qr_svg: render_qr_svg(&qr_url)?,
        qr_url,
        expires_at: session
            .expires_at
            .to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
        max_bytes: session.max_bytes,
    })
}

fn parse_server_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|error| format!("DropPoint timestamp is invalid: {error}"))
}

fn configured(state: &AppState) -> Result<DropPointConfig, String> {
    state
        .drop_point_config
        .clone()
        .ok_or_else(|| "DropPoint is not configured".to_string())
}

fn configured_client(state: &AppState) -> Result<DropPointClient, String> {
    state
        .drop_point_client
        .clone()
        .ok_or_else(|| "DropPoint is not configured".to_string())
}

fn configured_receiver_client(
    state: &AppState,
    session: &ActiveDropPointSession,
) -> Result<DropPointClient, String> {
    let persisted_origin = Url::parse(&session.base_url)
        .map_err(|_| "persisted DropPoint relay origin is invalid".to_string())?;
    match &state.drop_point_client {
        Some(client) if client.base_url() == &persisted_origin => Ok(client.clone()),
        Some(_) | None => DropPointClient::for_receiver(&session.base_url),
    }
}

fn validate_attachment_target(
    state: &ExecutionState,
    step_id: &str,
    input_id: &str,
) -> Result<(), String> {
    match &state.status {
        ExecutionStatus::Active => {}
        ExecutionStatus::Pending => return Err("execution has not been started".to_string()),
        ExecutionStatus::Finished(_) => return Err("execution has already finished".to_string()),
    }
    let step = state
        .steps
        .get(step_id)
        .ok_or_else(|| format!("step not found: {step_id}"))?;
    match &step.status {
        StepStatus::Present => {}
        StepStatus::Skipped { .. } => return Err(format!("step already skipped: {step_id}")),
    }
    let input_type = step.content.iter().find_map(|item| match item {
        ExecutionStepContent::InputBlock { inputs } => inputs
            .iter()
            .find(|definition| definition.id == input_id)
            .map(|definition| &definition.input_type),
        _ => None,
    });
    match input_type {
        Some(InputType::Attachment) => Ok(()),
        Some(_) | None => Err(format!("attachment input not found: {input_id}")),
    }
}

fn ensure_session_target(
    session: &ActiveDropPointSession,
    execution_id: ExecutionId,
    step_id: &str,
    input_id: &str,
) -> Result<(), String> {
    if session.execution_id != execution_id
        || session.step_id != step_id
        || session.input_id != input_id
    {
        return Err(
            "DropPoint session target does not match requested attachment input".to_string(),
        );
    }
    Ok(())
}

fn drop_link_with_fragment(
    base_url: &Url,
    drop_link: &str,
    recipient_public_key: &[u8; 32],
    expires_at: &str,
) -> Result<String, String> {
    let url = Url::parse(drop_link).map_err(|_| "DropPoint drop_link is invalid".to_string())?;
    if !crate::drop_point::client::same_origin(base_url, &url)
        || !url.username().is_empty()
        || url.password().is_some()
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return Err("DropPoint drop_link is not a fragment-free relay URL".to_string());
    }
    let fragment = form_urlencoded::Serializer::new(String::new())
        .append_pair("v", "2")
        .append_pair("pk", &encode_base64url(recipient_public_key))
        .append_pair("exp", expires_at)
        .finish();
    Ok(format!("{url}#{fragment}"))
}

fn render_qr_svg(value: &str) -> Result<String, String> {
    let code = QrCode::new(value.as_bytes()).map_err(|error| error.to_string())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#1a1a2e"))
        .light_color(svg::Color("#ffffff"))
        .build())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use std::io::Write as _;
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use procnote_core::event::types::{Event, EventMeta};
    use procnote_core::execution::ExecutionState;
    use procnote_core::template::parse_template;

    use super::*;
    use procnote_app::persistence::event_log::EventLog;
    use procnote_app::repository::{ExecutionRepository, NewExecution};

    const TEMPLATE: &str = r"---
id: drop-point-restart
title: DropPoint Restart
version: 1.0.0
---

## Capture

```inputs
- id: evidence
  label: Evidence
  type: attachment
```
";
    const RECIPIENT_PRIVATE_KEY: &str = "AQIDBAUGBwgJCgsMDQ4PEBESExQVFhcYGRobHB0eHyA";
    const RECIPIENT_PUBLIC_KEY: &str = "B6N8vBQgk8i3VdwbEOhstCY3StFqqFPtC9_AsrhtHHw";
    const ENVELOPE_JSON: &str = concat!(
        r#"{"protocol_version":2,"key_agreement":"x25519-hkdf-sha256-aesgcm-raw32","sender_ephemeral_public_key":"ZLEBsdC-WocEvQePmJUAH8A-jp-VIvGI3RKNmEbUhGY","metadata_nonce":"gYKDhIWGh4iJiouM","payload_nonce":"oaKjpKWmp6ipqqus","encrypted_metadata":"RXCd3ShA60Tza36-2nebwQVpV_NcAFlqtswR1p3V2_CXK9RVNjBXH2SER4pzbkLgtZj8Il4yGrid_PJ1BQatt8XhCygqbzWI5SCXUm-dZwSHv_bZSg6mhLJX6ED"#,
        r#"E8Uuhr0CYIabnfbDEU1swi_mQ6FshM7aLdi-XQzleiuSNyKclXXGJ-5WbPQI"}"#,
    );
    const ENCRYPTED_PAYLOAD: &str = "95kEDw2nrrpQAuknRO8NY2vBLOEvOd2Qjbzwu0aRORaf";

    #[test]
    #[expect(
        clippy::too_many_lines,
        reason = "the end-to-end restart scenario intentionally keeps every durability phase visible"
    )]
    fn repeated_uploads_ack_restart_and_explicit_close_are_resumable() {
        let encrypted_payload = URL_SAFE_NO_PAD.decode(ENCRYPTED_PAYLOAD).unwrap();
        let pickup_body = multipart_body(ENVELOPE_JSON.as_bytes(), &encrypted_payload);
        let (base_url, requests, server) = mock_relay(pickup_body);
        let config = DropPointConfig::for_test(Url::parse(&base_url).unwrap());
        let client = DropPointClient::new(config).unwrap();

        let temporary = tempfile::tempdir().unwrap();
        let workspace = temporary.path().join("workspace");
        std::fs::create_dir(&workspace).unwrap();
        let workspace = workspace.canonicalize().unwrap();
        let procedure_dir = workspace.join("drop-point-restart");
        std::fs::create_dir(&procedure_dir).unwrap();
        std::fs::write(procedure_dir.join("template.md"), TEMPLATE).unwrap();
        let template = parse_template(TEMPLATE).unwrap();
        let mut execution_state = ExecutionState::new();
        let initial_events = execution_state.start(&template).unwrap();
        let execution_id = execution_state.execution_id.unwrap();
        let created = ExecutionStore::new(workspace.clone())
            .create(NewExecution {
                procedure: "drop-point-restart".to_string(),
                template_source: TEMPLATE.to_string(),
                state: execution_state,
                initial_events,
                tool_version: "test".to_string(),
                meta: EventMeta::default(),
            })
            .unwrap();
        let execution_dir = std::path::PathBuf::from(created.location.unwrap());

        let state_root = temporary.path().join("private/drop-point-sessions");
        let sessions = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let expires_at = Utc::now() + chrono::Duration::minutes(10);
        let fragment = form_urlencoded::Serializer::new(String::new())
            .append_pair("v", "2")
            .append_pair("pk", RECIPIENT_PUBLIC_KEY)
            .append_pair(
                "exp",
                &expires_at.to_rfc3339_opts(chrono::SecondsFormat::AutoSi, true),
            )
            .finish();
        let session = ActiveDropPointSession::new(NewDropPointSession {
            session_id: uuid::Uuid::new_v4().to_string(),
            base_url: base_url.clone(),
            drop_point_id: "dp_example".to_string(),
            display_name: "calm-otter".to_string(),
            pickup_token: Zeroizing::new("pick_example".to_string()),
            recipient_private_key: Zeroizing::new(RECIPIENT_PRIVATE_KEY.to_string()),
            recipient_public_key: RECIPIENT_PUBLIC_KEY.to_string(),
            drop_link: Zeroizing::new(format!("{base_url}/drop/drop_example")),
            drop_link_with_fragment: Zeroizing::new(format!(
                "{base_url}/drop/drop_example#{fragment}"
            )),
            execution_id,
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            expires_at,
            max_bytes: 1024,
            execution_dir: execution_dir.clone(),
            workspace_root: workspace.clone(),
        });
        sessions.insert(&session).unwrap();

        let mut installed = tauri::async_runtime::block_on(install_submission(
            &client,
            &sessions,
            &workspace,
            session.clone(),
            "sub_AAAAAAAAAAAAAAAAAAAAAA",
        ))
        .unwrap();
        record_one_submission(
            &workspace,
            &sessions,
            &mut installed,
            "sub_AAAAAAAAAAAAAAAAAAAAAA",
        )
        .unwrap();
        let ack = tauri::async_runtime::block_on(acknowledge_submission(
            &client,
            &sessions,
            &mut installed,
            "sub_AAAAAAAAAAAAAAAAAAAAAA",
        ));
        assert!(ack.is_err());
        drop(sessions);

        let restarted = DropPointSessions::new(state_root.clone(), &workspace).unwrap();
        let mut resumed = restarted.get(&installed.session_id).unwrap();
        assert!(resumed.submissions["sub_AAAAAAAAAAAAAAAAAAAAAA"].recorded);
        assert!(!resumed.submissions["sub_AAAAAAAAAAAAAAAAAAAAAA"].acknowledged);
        assert!(
            record_pending_submissions(&workspace, &restarted, &mut resumed)
                .unwrap()
                .is_none()
        );
        tauri::async_runtime::block_on(acknowledge_submission(
            &client,
            &restarted,
            &mut resumed,
            "sub_AAAAAAAAAAAAAAAAAAAAAA",
        ))
        .unwrap();
        assert!(resumed.is_resumable());
        let second_id = format!("sub_{}", URL_SAFE_NO_PAD.encode([1u8; 16]));
        let mut second = tauri::async_runtime::block_on(install_submission(
            &client, &restarted, &workspace, resumed, &second_id,
        ))
        .unwrap();
        record_pending_submissions(&workspace, &restarted, &mut second).unwrap();
        tauri::async_runtime::block_on(acknowledge_submission(
            &client,
            &restarted,
            &mut second,
            &second_id,
        ))
        .unwrap();
        assert!(second.is_resumable());
        assert_eq!(second.submissions.len(), 2);
        assert!(
            tauri::async_runtime::block_on(close_session(&client, &restarted, &workspace, second,))
                .is_err()
        );
        drop(restarted);
        let restarted = DropPointSessions::new(state_root, &workspace).unwrap();
        let pending_close = restarted.get(&installed.session_id).unwrap();
        assert!(matches!(pending_close.phase, SessionPhase::ClosePending));
        tauri::async_runtime::block_on(close_session(
            &client,
            &restarted,
            &workspace,
            pending_close,
        ))
        .unwrap();

        let final_state = restarted.get(&installed.session_id).unwrap();
        assert!(final_state.recipient_private_key().is_err());
        assert!(final_state.pickup_token().is_err());
        let installed_path = final_state.submissions["sub_AAAAAAAAAAAAAAAAAAAAAA"]
            .bundle
            .path
            .clone();
        assert_eq!(
            std::fs::read(installed_path.join("scan-01.txt")).unwrap(),
            b"hello drop point\n"
        );
        let events = EventLog::new(execution_dir.join("events.jsonl"))
            .read()
            .unwrap();
        let attachment_events = events
            .iter()
            .filter_map(|event| match event {
                Event::AttachmentsAdded { attachments, .. } => Some(attachments),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(attachment_events.len(), 2);
        assert_eq!(
            attachment_events[0][0].content_type,
            "application/octet-stream"
        );

        let observed = requests.into_iter().collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                (
                    "GET /api/drop-points/dp_example/submissions/sub_AAAAAAAAAAAAAAAAAAAAAA/pickup"
                        .to_string(),
                    true
                ),
                (
                    "DELETE /api/drop-points/dp_example/submissions/sub_AAAAAAAAAAAAAAAAAAAAAA"
                        .to_string(),
                    true
                ),
                (
                    "DELETE /api/drop-points/dp_example/submissions/sub_AAAAAAAAAAAAAAAAAAAAAA"
                        .to_string(),
                    true
                ),
                (
                    format!("GET /api/drop-points/dp_example/submissions/{second_id}/pickup"),
                    true
                ),
                (
                    format!("DELETE /api/drop-points/dp_example/submissions/{second_id}"),
                    true
                ),
                ("DELETE /api/drop-points/dp_example".to_string(), true),
                ("DELETE /api/drop-points/dp_example".to_string(), true),
            ]
        );
        server.join().unwrap();

        // A published receipt must survive expiry even if private-state persistence
        // was interrupted immediately after installation.
        let mut recovery = session;
        recovery.session_id = uuid::Uuid::new_v4().to_string();
        recovery.drop_point_id = "dp_recovery".to_string();
        restarted.insert(&recovery).unwrap();
        let files = decrypt_bundle(
            &recovery.recipient_private_key().unwrap(),
            ENVELOPE_JSON.as_bytes(),
            &encrypted_payload,
        )
        .unwrap();
        let identity =
            encrypted_bundle_identity(ENVELOPE_JSON.as_bytes(), &encrypted_payload).unwrap();
        install_bundle(
            &recovery.execution_dir,
            &recovery.drop_point_id,
            &second_id,
            &identity,
            &files,
        )
        .unwrap();
        assert!(
            restarted
                .get(&recovery.session_id)
                .unwrap()
                .submissions
                .is_empty()
        );
        finalize_terminal_session(&restarted, &workspace, &recovery, RemoteTerminal::Expired)
            .unwrap();
        let recovered = restarted.get(&recovery.session_id).unwrap();
        assert!(recovered.submissions[&second_id].recorded);
        assert!(recovered.recipient_private_key().is_err());
        let events = EventLog::new(execution_dir.join("events.jsonl"))
            .read()
            .unwrap();
        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, Event::AttachmentsAdded { .. }))
                .count(),
            3
        );
    }

    fn multipart_body(envelope: &[u8], payload: &[u8]) -> Vec<u8> {
        let mut body = Vec::new();
        body.extend_from_slice(b"--test-boundary\r\n");
        body.extend_from_slice(b"Content-Disposition: attachment; name=\"envelope\"\r\n");
        body.extend_from_slice(b"Content-Type: application/json\r\n\r\n");
        body.extend_from_slice(envelope);
        body.extend_from_slice(b"\r\n--test-boundary\r\n");
        body.extend_from_slice(b"Content-Disposition: attachment; name=\"payload\"\r\n");
        body.extend_from_slice(b"Content-Type: application/octet-stream\r\n\r\n");
        body.extend_from_slice(payload);
        body.extend_from_slice(b"\r\n--test-boundary--\r\n");
        body
    }

    fn mock_relay(
        pickup_body: Vec<u8>,
    ) -> (
        String,
        mpsc::IntoIter<(String, bool)>,
        thread::JoinHandle<()>,
    ) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let (sender, receiver) = mpsc::channel();
        let server = thread::spawn(move || {
            let responses = [
                http_response(
                    "200 OK",
                    "multipart/mixed; boundary=test-boundary",
                    &pickup_body,
                ),
                http_response(
                    "500 Internal Server Error",
                    "application/json",
                    br#"{"error":{"code":"drop_point_close_failed","message":"temporary"}}"#,
                ),
                http_response("204 No Content", "application/json", b""),
                http_response(
                    "200 OK",
                    "multipart/mixed; boundary=test-boundary",
                    &pickup_body,
                ),
                http_response("204 No Content", "application/json", b""),
                http_response("500 Internal Server Error", "application/json", b"{}"),
                http_response("204 No Content", "application/json", b""),
            ];
            for response in responses {
                let (mut stream, _) = listener.accept().unwrap();
                let (request_line, authorized) = read_request_metadata(&mut stream);
                sender.send((request_line, authorized)).unwrap();
                stream.write_all(&response).unwrap();
            }
        });
        (format!("http://{address}"), receiver.into_iter(), server)
    }

    fn read_request_metadata(stream: &mut TcpStream) -> (String, bool) {
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let mut bytes = Vec::new();
        let mut buffer = [0u8; 1024];
        while !bytes.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = stream.read(&mut buffer).unwrap();
            if read == 0 {
                break;
            }
            bytes.extend_from_slice(&buffer[..read]);
        }
        let request = String::from_utf8(bytes).unwrap();
        let request_line = request.lines().next().unwrap().to_string();
        let request_line = request_line.strip_suffix(" HTTP/1.1").unwrap().to_string();
        let authorized = request
            .lines()
            .any(|line| line.eq_ignore_ascii_case("authorization: Bearer pick_example"));
        (request_line, authorized)
    }

    fn http_response(status: &str, content_type: &str, body: &[u8]) -> Vec<u8> {
        let headers = format!(
            "HTTP/1.1 {status}\r\nContent-Type: {content_type}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        [headers.as_bytes(), body].concat()
    }
}
