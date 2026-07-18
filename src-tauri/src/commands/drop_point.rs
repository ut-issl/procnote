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

use crate::commands::execution::{load_execution_from_disk, summarize};
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
    InstalledBundle, encrypted_bundle_identity, install_bundle, verify_installed_bundle,
};
use crate::persistence::execution_store::{ExecutionStore, InstalledAttachmentSource};
use crate::state::AppState;

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
    pub encrypted_size: u64,
    #[ts(optional)]
    pub dropped_at: Option<String>,
    #[ts(optional)]
    pub first_picked_up_at: Option<String>,
    pub expires_at: String,
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
    let session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
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
        encrypted_size: status.encrypted_size,
        dropped_at: status.dropped_at,
        first_picked_up_at: status.first_picked_up_at,
        expires_at: status.expires_at,
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
) -> Result<super::execution::ExecutionSummary, String> {
    let session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
    ensure_session_target(&session, execution_id, &step_id, &input_id)?;
    let current_state = validate_session_execution_dir(&state.procedures_dir, &session)?;
    if matches!(session.phase, SessionPhase::Waiting) {
        validate_attachment_target(&current_state, &step_id, &input_id)?;
    }

    if matches!(session.phase, SessionPhase::Complete { bundle: None, .. }) {
        return Err("DropPoint session ended before a bundle was installed".to_string());
    }
    if matches!(
        session.phase,
        SessionPhase::Complete {
            bundle: Some(_),
            ..
        }
    ) {
        return record_session_bundle(&state.procedures_dir, &session)?.ok_or_else(|| {
            "completed DropPoint session is missing its installed bundle".to_string()
        });
    }

    let session =
        ensure_bundle_installed(&client, &sessions, &state.procedures_dir, session).await?;
    finish_installed_import(&client, &sessions, &state.procedures_dir, &session).await
}

async fn finish_installed_import(
    client: &DropPointClient,
    sessions: &DropPointSessions,
    procedures_dir: &std::path::Path,
    session: &ActiveDropPointSession,
) -> Result<super::execution::ExecutionSummary, String> {
    let summary = record_session_bundle(procedures_dir, session)?
        .ok_or_else(|| "DropPoint session has no installed bundle".to_string())?;
    let close_pending = session.with_close_pending()?;
    sessions.persist(&close_pending)?;
    match client
        .close(&close_pending.drop_point_id, close_pending.pickup_token()?)
        .await
    {
        Ok(()) => {
            sessions
                .persist(&close_pending.with_complete(CompletionOutcome::ClosedSuccessfully))?;
            Ok(summary)
        }
        Err(error) => {
            if let Some(terminal) = error.terminal() {
                finalize_terminal_session(sessions, procedures_dir, &close_pending, terminal)?;
                Ok(summary)
            } else {
                Err(error.to_string())
            }
        }
    }
}

#[tauri::command]
pub async fn cancel_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    session_id: String,
) -> Result<(), String> {
    let session = sessions.get(&session_id)?;
    let client = configured_receiver_client(&state, &session)?;
    if !session.is_resumable() {
        return Ok(());
    }
    if session.installed_bundle().is_some() {
        record_session_bundle(&state.procedures_dir, &session)?;
    }
    match client
        .close(&session.drop_point_id, session.pickup_token()?)
        .await
    {
        Ok(()) => sessions.persist(&session.with_complete(CompletionOutcome::ClosedSuccessfully)),
        Err(error) => error.terminal().map_or_else(
            || Err(error.to_string()),
            |terminal| {
                finalize_terminal_session(&sessions, &state.procedures_dir, &session, terminal)
            },
        ),
    }
}

async fn ensure_bundle_installed(
    client: &DropPointClient,
    sessions: &DropPointSessions,
    procedures_dir: &std::path::Path,
    session: ActiveDropPointSession,
) -> Result<ActiveDropPointSession, String> {
    match &session.phase {
        SessionPhase::Waiting => {
            let pickup = client
                .pickup(
                    &session.drop_point_id,
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
                &identity,
                &recovered,
            )
            .map_err(|error| error.to_string())?;
            drop(recovered);
            let bundle = InstalledBundleState {
                identity: installed.identity,
                path: installed.path,
            };
            let updated = session.with_bundle_installed(bundle);
            sessions.persist(&updated)?;
            Ok(updated)
        }
        SessionPhase::BundleInstalled { .. } | SessionPhase::ClosePending { .. } => {
            verify_session_bundle(&session)?;
            Ok(session)
        }
        SessionPhase::Complete { .. } => Err("DropPoint session is already terminal".to_string()),
    }
}

fn verify_session_bundle(session: &ActiveDropPointSession) -> Result<InstalledBundle, String> {
    let bundle = session
        .installed_bundle()
        .ok_or_else(|| "DropPoint session has no installed bundle receipt".to_string())?;
    verify_installed_bundle(&bundle.path, &session.drop_point_id, &bundle.identity)
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

fn record_session_bundle(
    procedures_dir: &std::path::Path,
    session: &ActiveDropPointSession,
) -> Result<Option<super::execution::ExecutionSummary>, String> {
    if session.installed_bundle().is_none() {
        return Ok(None);
    }
    validate_session_execution_dir(procedures_dir, session)?;
    let installed = verify_session_bundle(session)?;
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
    summarize(&recorded.state, &recorded.execution_dir).map(Some)
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
    let current = sessions.get(&session.session_id)?;
    if current.installed_bundle().is_some() {
        record_session_bundle(procedures_dir, &current)?;
    }
    sessions.persist(&current.with_complete(terminal.into()))
}

fn validate_status_identity(
    session: &ActiveDropPointSession,
    status: &DropPointStatusResponse,
) -> Result<(), String> {
    let status_expiry = parse_server_datetime(&status.expires_at)?;
    if status.display_name != session.display_name
        || status_expiry != session.expires_at
        || status.encrypted_size > session.max_bytes
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
    use std::io::{Read as _, Write as _};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc;
    use std::thread;
    use std::time::Duration;

    use base64::Engine as _;
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use procnote_core::event::types::Event;
    use procnote_core::execution::ExecutionState;
    use procnote_core::template::parse_template;

    use super::*;
    use crate::persistence::event_log::EventLog;

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
    fn pickup_install_record_restart_and_close_is_end_to_end_resumable() {
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
        let template_path = procedure_dir.join("template.md");
        std::fs::write(&template_path, TEMPLATE).unwrap();
        let template = parse_template(TEMPLATE).unwrap();
        let mut execution_state = ExecutionState::new();
        let initial_events = execution_state.start(&template).unwrap();
        let execution_id = execution_state.execution_id.unwrap();
        let started_at = initial_events
            .iter()
            .find_map(|event| match event {
                Event::ExecutionStarted { at, .. } => Some(*at),
                _ => None,
            })
            .unwrap();
        let recorded = ExecutionStore::new(workspace.clone())
            .create_execution(
                &template_path,
                execution_state,
                initial_events,
                started_at,
                execution_id,
                "test".to_string(),
            )
            .unwrap();

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
            execution_dir: recorded.execution_dir.clone(),
            workspace_root: workspace.clone(),
        });
        sessions.insert(&session).unwrap();

        let installed = tauri::async_runtime::block_on(ensure_bundle_installed(
            &client, &sessions, &workspace, session,
        ))
        .unwrap();
        let first_finish = tauri::async_runtime::block_on(finish_installed_import(
            &client, &sessions, &workspace, &installed,
        ));
        assert!(first_finish.is_err());
        drop(sessions);

        let restarted = DropPointSessions::new(state_root, &workspace).unwrap();
        let resumed = restarted.get(&installed.session_id).unwrap();
        assert!(matches!(resumed.phase, SessionPhase::ClosePending { .. }));
        let resumed = tauri::async_runtime::block_on(ensure_bundle_installed(
            &client, &restarted, &workspace, resumed,
        ))
        .unwrap();
        tauri::async_runtime::block_on(finish_installed_import(
            &client, &restarted, &workspace, &resumed,
        ))
        .unwrap();

        let final_state = restarted.get(&resumed.session_id).unwrap();
        assert!(final_state.recipient_private_key().is_err());
        assert!(final_state.pickup_token().is_err());
        let installed_path = final_state.installed_bundle().unwrap().path.clone();
        assert_eq!(
            std::fs::read(installed_path.join("scan-01.txt")).unwrap(),
            b"hello drop point\n"
        );
        let events = EventLog::new(recorded.execution_dir.join("events.jsonl"))
            .read()
            .unwrap();
        let attachment_events = events
            .iter()
            .filter_map(|event| match event {
                Event::AttachmentsAdded { attachments, .. } => Some(attachments),
                _ => None,
            })
            .collect::<Vec<_>>();
        assert_eq!(attachment_events.len(), 1);
        assert_eq!(
            attachment_events[0][0].content_type,
            "application/octet-stream"
        );

        let observed = requests.into_iter().take(3).collect::<Vec<_>>();
        assert_eq!(
            observed,
            vec![
                ("GET /api/drop-points/dp_example/pickup".to_string(), true),
                ("DELETE /api/drop-points/dp_example".to_string(), true),
                ("DELETE /api/drop-points/dp_example".to_string(), true),
            ]
        );
        server.join().unwrap();
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
