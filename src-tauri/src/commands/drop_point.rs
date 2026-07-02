use procnote_core::event::types::ExecutionId;
use procnote_core::execution::{ExecutionState, ExecutionStatus, StepStatus};
use procnote_core::template::types::{InputType, StepContent};
use qrcode::QrCode;
use qrcode::render::svg;
use serde::Serialize;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tauri::State;
use ts_rs::TS;
use url::Url;
use url::form_urlencoded;

use crate::commands::execution::{load_execution_from_disk, summarize};
use crate::drop_point::client::{CreateDropPointResponse, DropPointClient, DropPointConfig};
use crate::drop_point::crypto::{decrypt_bundle, encode_base64url, generate_recipient_key_pair};
use crate::drop_point::multipart::parse_pickup_multipart;
use crate::drop_point::{ActiveDropPointSession, DropPointSessions};
use crate::persistence::execution_store::{AttachmentBytesSource, ExecutionStore};
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

#[tauri::command]
#[must_use]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn is_drop_point_configured(state: State<'_, AppState>) -> bool {
    state.drop_point_config.is_some()
}

#[tauri::command]
pub async fn start_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    execution_id: ExecutionId,
    step_id: String,
    input_id: String,
) -> Result<AttachmentDropPointSessionSummary, String> {
    let config = configured(&state)?;
    let (execution_state, _events, _log_path) =
        load_execution_from_disk(&state.procedures_dir, execution_id)?;
    validate_attachment_target(&execution_state, &step_id, &input_id)?;

    let client = DropPointClient::new(config.clone());
    if let Some(previous) = sessions.remove_for_target(execution_id, &step_id, &input_id)?
        && let Err(e) = client
            .close(&previous.drop_point_id, &previous.pickup_token)
            .await
    {
        log::warn!(
            "DropPoint close failed while replacing session {}: {}",
            previous.session_id,
            e
        );
    }

    let (recipient_private_key, recipient_public_key) = generate_recipient_key_pair();
    let created = client
        .create_drop_point()
        .await
        .map_err(|e| e.to_string())?;
    let target = SessionInputTarget { step_id, input_id };
    let session = build_session(
        &config,
        created,
        recipient_private_key,
        recipient_public_key,
        execution_id,
        target,
    );
    match session {
        Ok((session, summary)) => match sessions.insert(session.clone()) {
            Ok(()) => Ok(summary),
            Err(reason) => {
                if let Err(close_error) = client
                    .close(&session.drop_point_id, &session.pickup_token)
                    .await
                {
                    log::warn!(
                        "DropPoint close failed after session insert error for {}: {}",
                        session.drop_point_id,
                        close_error
                    );
                }
                Err(reason)
            }
        },
        Err(e) => {
            if let Err(close_error) = client.close(&e.drop_point_id, &e.pickup_token).await {
                log::warn!(
                    "DropPoint close failed after session setup error for {}: {}",
                    e.drop_point_id,
                    close_error
                );
            }
            Err(e.reason)
        }
    }
}

#[tauri::command]
pub async fn poll_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    session_id: String,
) -> Result<AttachmentDropPointStatus, String> {
    let config = configured(&state)?;
    let session = sessions.get(&session_id)?;
    let status = DropPointClient::new(config)
        .status(&session.drop_point_id, &session.pickup_token)
        .await
        .map_err(|e| e.to_string())?;

    Ok(AttachmentDropPointStatus {
        status: status.status,
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
    let config = configured(&state)?;
    let session = sessions.take(&session_id)?;
    let client = DropPointClient::new(config);

    let import_result = async {
        ensure_session_target(&session, execution_id, &step_id, &input_id)?;
        let (content_type, body) = client
            .pickup(&session.drop_point_id, &session.pickup_token)
            .await
            .map_err(|e| e.to_string())?;
        let (envelope_json, encrypted_payload) =
            parse_pickup_multipart(&content_type, &body).map_err(|e| e.to_string())?;
        let recovered = decrypt_bundle(
            session.recipient_private_key.as_ref(),
            &envelope_json,
            &encrypted_payload,
        )
        .map_err(|e| e.to_string())?;

        let sources = recovered
            .into_iter()
            .map(|file| AttachmentBytesSource {
                filename: file.filename,
                bytes: file.data,
            })
            .collect();
        let recorded = ExecutionStore::new(state.procedures_dir.clone())
            .record_attachment_bytes_batch(execution_id, &step_id, &input_id, sources)?;
        summarize(&recorded.state, &recorded.events, &recorded.execution_dir)
    }
    .await;

    match import_result {
        Ok(summary) => {
            if let Err(e) = client
                .close(&session.drop_point_id, &session.pickup_token)
                .await
            {
                log::warn!(
                    "DropPoint close failed after local import for session {}: {}",
                    session.session_id,
                    e
                );
            }
            Ok(summary)
        }
        Err(e) => {
            sessions.insert(session)?;
            Err(e)
        }
    }
}

#[tauri::command]
pub async fn cancel_attachment_drop_point_session(
    state: State<'_, AppState>,
    sessions: State<'_, DropPointSessions>,
    session_id: String,
) -> Result<(), String> {
    let config = configured(&state)?;
    let Ok(session) = sessions.take(&session_id) else {
        return Ok(());
    };
    if let Err(e) = DropPointClient::new(config)
        .close(&session.drop_point_id, &session.pickup_token)
        .await
    {
        log::warn!(
            "DropPoint close failed while cancelling session {}: {}",
            session.session_id,
            e
        );
    }
    Ok(())
}

struct SessionSetupError {
    drop_point_id: String,
    pickup_token: String,
    reason: String,
}

struct SessionInputTarget {
    step_id: String,
    input_id: String,
}

fn build_session(
    config: &DropPointConfig,
    created: CreateDropPointResponse,
    recipient_private_key: zeroize::Zeroizing<[u8; 32]>,
    recipient_public_key: [u8; 32],
    execution_id: ExecutionId,
    target: SessionInputTarget,
) -> Result<(ActiveDropPointSession, AttachmentDropPointSessionSummary), SessionSetupError> {
    let drop_point_id = created.drop_point_id;
    let pickup_token = created.pickup_token;
    let setup = (|| {
        let expires_at = parse_server_datetime(&created.expires_at)?;
        let qr_url = drop_link_with_fragment(
            &config.base_url,
            &created.drop_link,
            &recipient_public_key,
            &created.expires_at,
        )?;
        let qr_svg = render_qr_svg(&qr_url)?;
        let session_id = uuid::Uuid::new_v4().to_string();
        let session = ActiveDropPointSession {
            session_id: session_id.clone(),
            drop_point_id: drop_point_id.clone(),
            pickup_token: pickup_token.clone(),
            recipient_private_key: Arc::new(recipient_private_key),
            execution_id,
            step_id: target.step_id,
            input_id: target.input_id,
            expires_at,
        };
        let summary = AttachmentDropPointSessionSummary {
            session_id,
            display_name: created.display_name,
            qr_url,
            qr_svg,
            expires_at: created.expires_at,
            max_bytes: created.max_bytes,
        };
        Ok((session, summary))
    })();

    setup.map_err(|reason| SessionSetupError {
        drop_point_id,
        pickup_token,
        reason,
    })
}

fn parse_server_datetime(value: &str) -> Result<DateTime<Utc>, String> {
    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .map_err(|e| format!("DropPoint timestamp is invalid: {e}"))
}

fn configured(state: &AppState) -> Result<DropPointConfig, String> {
    state
        .drop_point_config
        .clone()
        .ok_or_else(|| "DropPoint is not configured".to_string())
}

fn validate_attachment_target(
    state: &ExecutionState,
    step_id: &str,
    input_id: &str,
) -> Result<(), String> {
    match state.status {
        ExecutionStatus::Active => {}
        ExecutionStatus::Pending => return Err("execution has not been started".to_string()),
        ExecutionStatus::Finished(_) => return Err("execution has already finished".to_string()),
    }
    let step = state
        .steps
        .get(step_id)
        .ok_or_else(|| format!("step not found: {step_id}"))?;
    match step.status {
        StepStatus::Present => {}
        StepStatus::Skipped => return Err(format!("step already skipped: {step_id}")),
    }
    let input_type = step.content.iter().find_map(|item| match item {
        StepContent::InputBlock { inputs } => inputs
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
    let mut url =
        Url::parse(drop_link).map_err(|e| format!("DropPoint drop_link is invalid: {e}"))?;
    if !same_origin(base_url, &url) {
        return Err("DropPoint drop_link origin does not match configured server".to_string());
    }
    url.set_fragment(None);
    let fragment = form_urlencoded::Serializer::new(String::new())
        .append_pair("v", "2")
        .append_pair("pk", &encode_base64url(recipient_public_key))
        .append_pair("exp", expires_at)
        .finish();
    Ok(format!("{url}#{fragment}"))
}

fn same_origin(expected: &Url, actual: &Url) -> bool {
    actual.username().is_empty()
        && actual.password().is_none()
        && expected.scheme() == actual.scheme()
        && expected.host_str() == actual.host_str()
        && expected.port_or_known_default() == actual.port_or_known_default()
}

fn render_qr_svg(value: &str) -> Result<String, String> {
    let code = QrCode::new(value.as_bytes()).map_err(|e| e.to_string())?;
    Ok(code
        .render::<svg::Color>()
        .min_dimensions(240, 240)
        .dark_color(svg::Color("#1a1a2e"))
        .light_color(svg::Color("#ffffff"))
        .build())
}
