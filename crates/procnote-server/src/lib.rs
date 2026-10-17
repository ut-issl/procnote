//! HTTP API server exposing the same operations as the desktop IPC commands.
//!
//! Storage is the same procedures directory layout the desktop app uses, so a
//! workspace can be opened from both at once. There is no authentication yet;
//! run it only on a trusted network.

mod live;
mod upload_links;
mod uploads;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tower_http::services::{ServeDir, ServeFile};

use live::LiveUpdates;
use procnote_app::action::{ActionMeta, AttachmentSource, ExecutionAction};
use procnote_app::execution::{ExecutionSummary, infer_attachment_content_type};
use procnote_app::template::TemplateSummary;
use procnote_core::event::types::ExecutionId;
use procnote_core::template::types::ProcedureTemplate;
use upload_links::UploadLinks;
use uploads::UploadStore;

/// Largest accepted multipart upload request.
const MAX_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

pub struct ServerConfig {
    /// Workspace directory containing procedure subdirectories.
    pub procedures_dir: PathBuf,
    /// Directory where uploaded files are staged before being attached.
    pub upload_dir: PathBuf,
    /// Built frontend to serve for non-API paths, if any.
    pub static_dir: Option<PathBuf>,
    /// Directory for server-owned state that must not live in the procedures
    /// directory (upload link tokens).
    pub state_dir: PathBuf,
}

struct AppState {
    procedures_dir: PathBuf,
    uploads: UploadStore,
    links: UploadLinks,
    live: LiveUpdates,
}

type SharedState = Arc<AppState>;

/// Build the application router.
pub fn router(config: ServerConfig) -> std::io::Result<Router> {
    let state = Arc::new(AppState {
        procedures_dir: config.procedures_dir,
        uploads: UploadStore::new(config.upload_dir)?,
        links: UploadLinks::new(config.state_dir.join("upload-links"))?,
        live: LiveUpdates::default(),
    });

    let api = Router::new()
        .route("/templates", get(list_templates))
        .route("/templates/content", get(get_template))
        .route("/executions", get(list_executions).post(start_execution))
        .route("/executions/{id}", get(get_execution))
        .route("/executions/{id}/actions", post(record_action))
        .route("/executions/{id}/live", get(live_updates))
        .route(
            "/executions/{id}/upload-link",
            get(get_upload_link)
                .post(create_upload_link)
                .delete(revoke_upload_link),
        )
        .route("/upload-links/{token}", get(upload_link_target))
        .route(
            "/upload-links/{token}/files",
            post(upload_to_link).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route(
            "/executions/{id}/attachment-preview",
            get(attachment_preview),
        )
        .route(
            "/uploads",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .fallback(|| async { ApiError::new(StatusCode::NOT_FOUND, "no such API endpoint") })
        .with_state(state);

    let app = Router::new().nest("/api", api);
    Ok(match config.static_dir {
        Some(dir) => {
            let index = dir.join("index.html");
            app.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => app,
    })
}

/// Error returned to the browser as `{"error": "..."}`.
struct ApiError {
    status: StatusCode,
    message: String,
}

impl ApiError {
    fn new(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
        }
    }

    fn bad_request(message: impl Into<String>) -> Self {
        Self::new(StatusCode::BAD_REQUEST, message)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(serde_json::json!({ "error": self.message })),
        )
            .into_response()
    }
}

/// Run filesystem-bound work off the async runtime.
async fn blocking<T: Send + 'static>(
    f: impl FnOnce() -> Result<T, String> + Send + 'static,
) -> Result<T, ApiError> {
    tokio::task::spawn_blocking(f)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
        .map_err(ApiError::bad_request)
}

async fn list_templates(
    State(state): State<SharedState>,
) -> Result<Json<Vec<TemplateSummary>>, ApiError> {
    blocking(move || procnote_app::template::list_templates(&state.procedures_dir))
        .await
        .map(Json)
}

async fn list_executions(
    State(state): State<SharedState>,
) -> Result<Json<Vec<ExecutionSummary>>, ApiError> {
    blocking(move || procnote_app::execution::list_executions(&state.procedures_dir))
        .await
        .map(Json)
}

#[derive(Deserialize)]
struct TemplateQuery {
    path: String,
}

async fn get_template(
    State(state): State<SharedState>,
    Query(query): Query<TemplateQuery>,
) -> Result<Json<ProcedureTemplate>, ApiError> {
    blocking(move || {
        procnote_app::template::get_template(
            &state.procedures_dir,
            std::path::Path::new(&query.path),
        )
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct StartExecutionRequest {
    template_path: String,
    #[serde(default)]
    meta: ActionMeta,
}

async fn start_execution(
    State(state): State<SharedState>,
    Json(request): Json<StartExecutionRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    blocking(move || {
        procnote_app::execution::start_execution(
            &state.procedures_dir,
            std::path::Path::new(&request.template_path),
            env!("CARGO_PKG_VERSION").to_string(),
            request.meta,
        )
    })
    .await
    .map(Json)
}

async fn get_execution(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    blocking(move || procnote_app::execution::get_execution(&state.procedures_dir, id))
        .await
        .map(Json)
}

/// An action plus who performed it; `meta` may be omitted.
#[derive(Deserialize)]
struct RecordActionRequest {
    action: ExecutionAction,
    #[serde(default)]
    meta: ActionMeta,
}

async fn record_action(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    Json(request): Json<RecordActionRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    let RecordActionRequest { mut action, meta } = request;
    let live_state = state.clone();
    let summary = blocking(move || {
        let tokens = state.uploads.authorize(&mut action)?;
        let result =
            procnote_app::execution::record_action(&state.procedures_dir, id, action, meta);
        // Uploads are single-use, like the desktop file picker's grants.
        for token in tokens {
            state.uploads.discard(token);
        }
        result
    })
    .await?;
    live_state.live.publish(id, summary.seq);
    Ok(Json(summary))
}

/// WebSocket that sends `{"seq": n}` now and after every write to the
/// execution through this server. Clients refetch when `seq` is ahead of the
/// state they hold, which also covers anything missed while disconnected.
async fn live_updates(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    upgrade: WebSocketUpgrade,
) -> Response {
    upgrade.on_upgrade(move |socket| live_session(state, id, socket))
}

async fn current_seq(state: &SharedState, id: ExecutionId) -> Result<u64, ApiError> {
    let state = state.clone();
    blocking(move || procnote_app::execution::get_execution(&state.procedures_dir, id))
        .await
        .map(|summary| summary.seq)
}

async fn send_seq(socket: &mut WebSocket, seq: u64) -> bool {
    let text = serde_json::json!({ "seq": seq }).to_string();
    socket.send(Message::Text(text.into())).await.is_ok()
}

async fn live_session(state: SharedState, id: ExecutionId, mut socket: WebSocket) {
    // Subscribe before reading the current seq so no write falls in between.
    let mut updates = state.live.subscribe(id);
    let Ok(seq) = current_seq(&state, id).await else {
        let _ = socket.send(Message::Close(None)).await;
        return;
    };
    if !send_seq(&mut socket, seq).await {
        return;
    }
    loop {
        tokio::select! {
            update = updates.recv() => {
                let seq = match update {
                    Ok(seq) => seq,
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {
                        match current_seq(&state, id).await {
                            Ok(seq) => seq,
                            Err(_) => break,
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                };
                if !send_seq(&mut socket, seq).await {
                    break;
                }
            }
            incoming = socket.recv() => match incoming {
                None | Some(Err(_) | Ok(Message::Close(_))) => break,
                Some(Ok(_)) => {}
            },
        }
    }
}

#[derive(Deserialize)]
struct PreviewQuery {
    path: String,
}

async fn attachment_preview(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    Query(query): Query<PreviewQuery>,
) -> Result<Json<Option<String>>, ApiError> {
    blocking(move || {
        procnote_app::execution::attachment_preview_data_url(&state.procedures_dir, id, &query.path)
    })
    .await
    .map(Json)
}

/// A file streamed from a multipart request into the upload staging area.
struct StagedFile {
    token: uuid::Uuid,
    path: PathBuf,
    filename: String,
}

/// Stream every file field of `multipart` into staging; other fields are
/// returned as text by name.
async fn stage_multipart(
    state: &SharedState,
    mut multipart: Multipart,
) -> Result<(Vec<StagedFile>, HashMap<String, String>), ApiError> {
    let mut files = Vec::new();
    let mut fields = HashMap::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        let Some(filename) = field.file_name().map(str::to_string) else {
            if let Some(name) = field.name().map(str::to_string) {
                let text = field
                    .text()
                    .await
                    .map_err(|e| ApiError::bad_request(e.to_string()))?;
                fields.insert(name, text);
            }
            continue;
        };
        let (token, path) = state
            .uploads
            .reserve(&filename)
            .map_err(ApiError::bad_request)?;
        let mut file = tokio::fs::File::create(&path)
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        while let Some(chunk) = field
            .chunk()
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?
        {
            file.write_all(&chunk)
                .await
                .map_err(|e| ApiError::bad_request(e.to_string()))?;
        }
        file.flush()
            .await
            .map_err(|e| ApiError::bad_request(e.to_string()))?;
        let staged_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or(&filename)
            .to_string();
        files.push(StagedFile {
            token,
            path,
            filename: staged_name,
        });
    }
    Ok((files, fields))
}

/// Stage uploaded files and return references usable in attachment actions.
///
/// Every multipart field with a filename is treated as one file.
async fn upload(
    State(state): State<SharedState>,
    multipart: Multipart,
) -> Result<Json<Vec<AttachmentSource>>, ApiError> {
    let (files, _) = stage_multipart(&state, multipart).await?;
    if files.is_empty() {
        return Err(ApiError::bad_request("no files in upload"));
    }
    Ok(Json(
        files
            .into_iter()
            .map(|file| AttachmentSource {
                content_type: infer_attachment_content_type(&file.filename),
                filename: file.filename,
                path: file.token.to_string(),
            })
            .collect(),
    ))
}

// --- Upload links (migration plan §7.2) ---

#[derive(Serialize)]
struct UploadLinkResponse {
    token: Option<String>,
}

async fn get_upload_link(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<UploadLinkResponse>, ApiError> {
    blocking(move || state.links.find(id))
        .await
        .map(|token| Json(UploadLinkResponse { token }))
}

/// Return the execution's upload link, creating it if needed.
async fn create_upload_link(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<UploadLinkResponse>, ApiError> {
    blocking(move || {
        let summary = procnote_app::execution::get_execution(&state.procedures_dir, id)?;
        if summary.status != "active" {
            return Err("upload links are only available for active executions".to_string());
        }
        state.links.get_or_create(id)
    })
    .await
    .map(|token| Json(UploadLinkResponse { token: Some(token) }))
}

async fn revoke_upload_link(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<UploadLinkResponse>, ApiError> {
    blocking(move || state.links.revoke(id))
        .await
        .map(|()| Json(UploadLinkResponse { token: None }))
}

/// What a phone opening an upload link may see: the execution's name only.
#[derive(Serialize)]
struct UploadTarget {
    name: String,
    procedure_title: String,
    active: bool,
}

fn resolve_link(state: &SharedState, token: &str) -> Result<ExecutionId, ApiError> {
    state
        .links
        .resolve(token)
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, "this upload link is not valid"))
}

async fn upload_link_target(
    State(state): State<SharedState>,
    Path(token): Path<String>,
) -> Result<Json<UploadTarget>, ApiError> {
    let id = resolve_link(&state, &token)?;
    blocking(move || procnote_app::execution::get_execution(&state.procedures_dir, id))
        .await
        .map(|summary| {
            Json(UploadTarget {
                name: summary.name.unwrap_or(summary.procedure_id),
                procedure_title: summary.procedure_title,
                active: summary.status == "active",
            })
        })
}

#[derive(Serialize)]
struct UploadReceipt {
    received: usize,
    skipped: usize,
}

/// Receive files sent to an upload link into the execution's inbox.
///
/// Optional text fields `recorded_by` and `device` say who sent them.
async fn upload_to_link(
    State(state): State<SharedState>,
    Path(token): Path<String>,
    multipart: Multipart,
) -> Result<Json<UploadReceipt>, ApiError> {
    let id = resolve_link(&state, &token)?;
    let (files, fields) = stage_multipart(&state, multipart).await?;
    if files.is_empty() {
        return Err(ApiError::bad_request("no files in upload"));
    }
    let meta = ActionMeta {
        event_id: None,
        recorded_by: fields.get("recorded_by").cloned(),
        device: Some(
            fields
                .get("device")
                .cloned()
                .unwrap_or_else(|| "upload-link".to_string()),
        ),
    };
    let total = files.len();
    let live_state = state.clone();
    let (summary, received) = blocking(move || {
        let sources = files
            .iter()
            .map(|file| (file.path.clone(), file.filename.clone()))
            .collect();
        let result =
            procnote_app::execution::receive_files(&state.procedures_dir, id, sources, meta);
        for file in files {
            state.uploads.discard(file.token);
        }
        result
    })
    .await?;
    live_state.live.publish(id, summary.seq);
    Ok(Json(UploadReceipt {
        received,
        skipped: total - received,
    }))
}
