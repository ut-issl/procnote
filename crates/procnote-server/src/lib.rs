//! HTTP API server exposing the same operations as the desktop IPC commands.
//!
//! Storage is the same procedures directory layout the desktop app uses, so a
//! workspace can be opened from both at once. Users are authenticated by
//! Cloudflare Access in front of the server (see [`access`]); the server
//! verifies Access's token on every API request except upload links.

pub mod access;
pub mod collab;
mod live;
pub mod postgres;
pub mod transfer;
mod upload_links;
mod uploads;

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, Request, State};
use axum::http::StatusCode;
use axum::middleware::{Next, from_fn_with_state};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Extension, Json, Router};
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tower_http::services::{ServeDir, ServeFile};

use access::{AccessConfig, AccessVerifier, Identity, token_from_headers};
use collab::{CollabTemplateSummary, CollabTemplates};
use live::LiveUpdates;
use procnote_app::action::{ActionMeta, AttachmentSource, ExecutionAction};
use procnote_app::execution::{ExecutionSummary, infer_attachment_content_type};
use procnote_app::repository::{ExecutionRepository, TemplateSnapshot};
use procnote_app::sync::{self, Conflict, PushOutcome, Resolution};
use procnote_app::template::TemplateSummary;
use procnote_core::event::types::{EventRecord, ExecutionId};
use procnote_core::template::types::ProcedureTemplate;
use upload_links::UploadLinks;
use uploads::UploadStore;

/// Largest accepted multipart upload request.
const MAX_UPLOAD_BYTES: usize = 512 * 1024 * 1024;

pub struct ServerConfig {
    /// Workspace directory containing procedure subdirectories.
    pub procedures_dir: PathBuf,
    /// Where executions are stored: the procedures directory itself
    /// ([`procnote_app::persistence::execution_store::ExecutionStore`]) or a database ([`postgres::PostgresStore`]).
    pub executions: Box<dyn ExecutionRepository>,
    /// Directory where uploaded files are staged before being attached.
    pub upload_dir: PathBuf,
    /// Built frontend to serve for non-API paths, if any.
    pub static_dir: Option<PathBuf>,
    /// Directory for server-owned state that must not live in the procedures
    /// directory (upload link tokens).
    pub state_dir: PathBuf,
    pub auth: Auth,
    /// Collaborative template editing, when a database and the collab
    /// service are configured.
    pub collab: Option<CollabTemplates>,
}

/// How API requests are authenticated.
pub enum Auth {
    /// Behind Cloudflare Access: every API request (except upload links) must
    /// carry a valid Access token, whose email becomes the recorder.
    Access(AccessConfig),
    /// No authentication; recorders are whatever the client sends. For local
    /// development only.
    Disabled,
}

struct AppState {
    procedures_dir: PathBuf,
    executions: Box<dyn ExecutionRepository>,
    uploads: UploadStore,
    links: UploadLinks,
    live: LiveUpdates,
    access: Option<AccessVerifier>,
    collab: Option<CollabTemplates>,
}

type SharedState = Arc<AppState>;

/// Build the application router.
pub fn router(config: ServerConfig) -> std::io::Result<Router> {
    let state = Arc::new(AppState {
        executions: config.executions,
        procedures_dir: config.procedures_dir,
        uploads: UploadStore::new(config.upload_dir)?,
        links: UploadLinks::new(config.state_dir.join("upload-links"))?,
        live: LiveUpdates::default(),
        access: match config.auth {
            Auth::Access(access) => Some(AccessVerifier::new(access)),
            Auth::Disabled => None,
        },
        collab: config.collab,
    });

    // Upload links are reached by phones without logging in (they are an
    // Access Bypass); the random token in the path is their credential.
    let link_routes = Router::new()
        .route("/upload-links/{token}", get(upload_link_target))
        .route(
            "/upload-links/{token}/files",
            post(upload_to_link).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        );

    let api = Router::new()
        .route("/me", get(me))
        .route("/templates", get(list_templates))
        .route("/templates/content", get(get_template))
        .route(
            "/collab/templates",
            get(list_collab_templates).post(create_collab_template),
        )
        .route("/collab/templates/{id}/token", post(collab_token))
        .route(
            "/collab/templates/{id}/executions",
            post(start_collab_execution),
        )
        .route("/collab/templates/{id}/markdown", get(collab_markdown))
        .route("/executions", get(list_executions).post(start_execution))
        .route(
            "/executions/{id}",
            get(get_execution).put(create_started_execution),
        )
        .route(
            "/executions/{id}/events",
            get(pull_events).post(push_events),
        )
        .route("/executions/{id}/conflicts", get(list_conflicts))
        .route(
            "/executions/{id}/conflicts/{event_id}/resolve",
            post(resolve_conflict),
        )
        .route("/executions/{id}/actions", post(record_action))
        .route("/executions/{id}/live", get(live_updates))
        .route(
            "/executions/{id}/upload-link",
            get(get_upload_link)
                .post(create_upload_link)
                .delete(revoke_upload_link),
        )
        .route(
            "/executions/{id}/attachment-preview",
            get(attachment_preview),
        )
        .route(
            "/uploads",
            post(upload).layer(DefaultBodyLimit::max(MAX_UPLOAD_BYTES)),
        )
        .route_layer(from_fn_with_state(state.clone(), require_identity))
        .merge(link_routes)
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

/// Reject requests without a valid Access token, and attach the caller's
/// [`Identity`] to those with one. A no-op when authentication is disabled.
async fn require_identity(
    State(state): State<SharedState>,
    mut request: Request,
    next: Next,
) -> Response {
    let Some(verifier) = &state.access else {
        return next.run(request).await;
    };
    let outcome = match token_from_headers(request.headers()) {
        Some(token) => verifier.verify(token).await,
        None => Err(access::AuthError::Missing),
    };
    match outcome {
        Ok(identity) => {
            request.extensions_mut().insert(identity);
            next.run(request).await
        }
        Err(e) => {
            log::info!(
                "rejected {} {}: {e}",
                request.method(),
                request.uri().path()
            );
            ApiError::new(StatusCode::UNAUTHORIZED, e.to_string()).into_response()
        }
    }
}

/// The caller as the server sees them.
#[derive(Serialize)]
struct Me {
    /// `"access"` when Cloudflare Access authenticates users, else `"none"`.
    auth: &'static str,
    email: Option<String>,
}

async fn me(State(state): State<SharedState>, identity: Option<Extension<Identity>>) -> Json<Me> {
    Json(Me {
        auth: if state.access.is_some() {
            "access"
        } else {
            "none"
        },
        email: identity.map(|Extension(identity)| identity.email),
    })
}

/// With an authenticated caller, they are the recorder, whatever the client sent.
fn attribute(mut meta: ActionMeta, identity: Option<Extension<Identity>>) -> ActionMeta {
    if let Some(Extension(identity)) = identity {
        meta.recorded_by = Some(identity.email);
    }
    meta
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
    blocking(move || procnote_app::execution::list_executions(state.executions.as_ref()))
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

fn collab(state: &AppState) -> Result<&CollabTemplates, ApiError> {
    state.collab.as_ref().ok_or_else(|| {
        ApiError::new(
            StatusCode::NOT_FOUND,
            "collaborative editing is not enabled on this server",
        )
    })
}

#[derive(Serialize)]
struct CollabTemplateList {
    /// Whether this server has collaborative editing at all.
    enabled: bool,
    templates: Vec<CollabTemplateSummary>,
}

async fn list_collab_templates(
    State(state): State<SharedState>,
) -> Result<Json<CollabTemplateList>, ApiError> {
    let Some(collab) = &state.collab else {
        return Ok(Json(CollabTemplateList {
            enabled: false,
            templates: Vec::new(),
        }));
    };
    let templates = collab
        .list()
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e))?;
    Ok(Json(CollabTemplateList {
        enabled: true,
        templates,
    }))
}

#[derive(Deserialize)]
struct CreateCollabTemplateRequest {
    /// Workspace template to start the document from.
    template_path: String,
}

#[derive(Serialize)]
struct CreatedCollabTemplate {
    id: uuid::Uuid,
}

async fn create_collab_template(
    State(state): State<SharedState>,
    Json(request): Json<CreateCollabTemplateRequest>,
) -> Result<Json<CreatedCollabTemplate>, ApiError> {
    collab(&state)?;
    let template = blocking({
        let state = state.clone();
        move || {
            procnote_app::template::get_template(
                &state.procedures_dir,
                std::path::Path::new(&request.template_path),
            )
        }
    })
    .await?;
    let id = collab(&state)?
        .create(&template)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e))?;
    Ok(Json(CreatedCollabTemplate { id }))
}

#[derive(Deserialize)]
struct StartCollabExecutionRequest {
    #[serde(default)]
    meta: ActionMeta,
}

/// Start an execution of a shared template as it is now, keeping it as JSON.
async fn start_collab_execution(
    State(state): State<SharedState>,
    Path(id): Path<uuid::Uuid>,
    identity: Option<Extension<Identity>>,
    Json(request): Json<StartCollabExecutionRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    let template = collab(&state)?
        .current(id)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e))?
        .usable()
        .map_err(ApiError::bad_request)?;
    let procedure = collab::procedure_name(&template.metadata.id).map_err(ApiError::bad_request)?;
    let snapshot = TemplateSnapshot::of_template(&template).map_err(ApiError::bad_request)?;
    let meta = attribute(request.meta, identity);
    blocking(move || {
        procnote_app::execution::start_execution_of(
            state.executions.as_ref(),
            procedure,
            &template,
            snapshot,
            env!("CARGO_PKG_VERSION").to_string(),
            meta,
        )
    })
    .await
    .map(Json)
}

/// A shared template as procnote Markdown (`template.md`), e.g. to keep it in git.
async fn collab_markdown(
    State(state): State<SharedState>,
    Path(id): Path<uuid::Uuid>,
) -> Result<Response, ApiError> {
    let template = collab(&state)?
        .current(id)
        .await
        .map_err(|e| ApiError::new(StatusCode::BAD_GATEWAY, e))?
        .usable()
        .map_err(ApiError::bad_request)?;
    let markdown = procnote_core::template::to_markdown(&template)
        .map_err(|e| ApiError::bad_request(e.to_string()))?;
    Ok((
        [(
            axum::http::header::CONTENT_TYPE,
            "text/markdown; charset=utf-8",
        )],
        markdown,
    )
        .into_response())
}

#[derive(Serialize)]
struct CollabToken {
    token: String,
}

async fn collab_token(
    State(state): State<SharedState>,
    Path(id): Path<uuid::Uuid>,
    identity: Option<Extension<Identity>>,
) -> Result<Json<CollabToken>, ApiError> {
    let subject = identity
        .map(|Extension(identity)| identity.email)
        .unwrap_or_default();
    collab(&state)?
        .token(id, &subject)
        .await
        .map_err(|e| ApiError::new(StatusCode::INTERNAL_SERVER_ERROR, e))?
        .map(|token| Json(CollabToken { token }))
        .ok_or_else(|| ApiError::new(StatusCode::NOT_FOUND, format!("no template {id}")))
}

#[derive(Deserialize)]
struct StartExecutionRequest {
    template_path: String,
    #[serde(default)]
    meta: ActionMeta,
}

async fn start_execution(
    State(state): State<SharedState>,
    identity: Option<Extension<Identity>>,
    Json(request): Json<StartExecutionRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    let meta = attribute(request.meta, identity);
    blocking(move || {
        procnote_app::execution::start_execution(
            state.executions.as_ref(),
            &state.procedures_dir,
            std::path::Path::new(&request.template_path),
            env!("CARGO_PKG_VERSION").to_string(),
            meta,
        )
    })
    .await
    .map(Json)
}

async fn get_execution(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    blocking(move || procnote_app::execution::get_execution(state.executions.as_ref(), id))
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
    identity: Option<Extension<Identity>>,
    Json(request): Json<RecordActionRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    let RecordActionRequest { mut action, meta } = request;
    let meta = attribute(meta, identity);
    let live_state = state.clone();
    let summary = blocking(move || {
        let tokens = state.uploads.authorize(&mut action)?;
        let result =
            procnote_app::execution::record_action(state.executions.as_ref(), id, action, meta);
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

#[derive(Deserialize)]
struct PullQuery {
    /// Log lines already held; only later ones are sent.
    #[serde(default)]
    after: usize,
}

#[derive(Serialize)]
struct PulledEvents {
    /// Log lines from position `after` on.
    records: Vec<EventRecord>,
    /// Number of log lines in total: the next `after`.
    count: usize,
}

/// The execution's log lines after the first `after`, for devices that keep
/// a copy of the log (offline recording).
async fn pull_events(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    Query(query): Query<PullQuery>,
) -> Result<Json<PulledEvents>, ApiError> {
    blocking(move || {
        let records = state.executions.records(id)?;
        let count = records.len();
        Ok(PulledEvents {
            records: records.into_iter().skip(query.after).collect(),
            count,
        })
    })
    .await
    .map(Json)
}

/// Log lines as JSON objects (an event and its metadata in one object).
fn parse_records(values: &[serde_json::Value]) -> Result<Vec<EventRecord>, ApiError> {
    values
        .iter()
        .map(|value| {
            EventRecord::from_value(value)
                .map_err(|e| ApiError::bad_request(format!("invalid event: {e}")))
        })
        .collect()
}

/// With an authenticated caller, they are the recorder of what they push.
fn attribute_record(mut record: EventRecord, identity: Option<&Identity>) -> EventRecord {
    if let Some(identity) = identity {
        record.meta.recorded_by = Some(identity.email.clone());
    }
    record
}

#[derive(Deserialize)]
struct PushRequest {
    /// Log lines recorded on the device, in order, each with an `event_id`.
    events: Vec<serde_json::Value>,
    /// Upload tokens for the attachment content the events reference.
    #[serde(default)]
    uploads: Vec<uuid::Uuid>,
}

#[derive(Serialize)]
struct PushedEvent {
    event_id: Option<uuid::Uuid>,
    #[serde(flatten)]
    outcome: PushOutcome,
}

#[derive(Serialize)]
struct PushResponse {
    results: Vec<PushedEvent>,
}

/// Record events a device recorded, possibly offline. Each is recorded if it
/// still applies, skipped if already recorded, or kept as a conflict.
async fn push_events(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    identity: Option<Extension<Identity>>,
    Json(request): Json<PushRequest>,
) -> Result<Json<PushResponse>, ApiError> {
    let identity = identity.map(|Extension(identity)| identity);
    let records: Vec<EventRecord> = parse_records(&request.events)?
        .into_iter()
        .map(|record| attribute_record(record, identity.as_ref()))
        .collect();
    let live_state = state.clone();
    let (results, seq) = blocking(move || {
        let uploaded = request
            .uploads
            .iter()
            .map(|token| state.uploads.staged(*token))
            .collect::<Result<Vec<_>, _>>();
        let results = uploaded.and_then(|uploaded| {
            records
                .iter()
                .map(|record| {
                    sync::push_event(
                        state.executions.as_ref(),
                        id,
                        record,
                        &uploaded,
                        chrono::Utc::now(),
                    )
                    .map(|outcome| PushedEvent {
                        event_id: record.meta.event_id,
                        outcome,
                    })
                })
                .collect::<Result<Vec<_>, _>>()
        });
        // Uploads are single-use, like the desktop file picker's grants.
        for token in &request.uploads {
            state.uploads.discard(*token);
        }
        let results = results?;
        let seq = procnote_app::execution::get_execution(state.executions.as_ref(), id)?.seq;
        Ok((results, seq))
    })
    .await?;
    if results
        .iter()
        .any(|result| result.outcome == PushOutcome::Recorded)
    {
        live_state.live.publish(id, seq);
    }
    Ok(Json(PushResponse { results }))
}

#[derive(Deserialize)]
struct CreateStartedRequest {
    /// Procedure (directory) name to file the execution under.
    procedure: String,
    /// The template it was started from, kept as its snapshot.
    template: ProcedureTemplate,
    /// Its first log lines, as `ExecutionState::start` produced them.
    events: Vec<serde_json::Value>,
}

/// Store an execution a device started itself (possibly offline). Creating
/// one that already exists with the same start is a retry and succeeds.
async fn create_started_execution(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    identity: Option<Extension<Identity>>,
    Json(request): Json<CreateStartedRequest>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    let identity = identity.map(|Extension(identity)| identity);
    let records: Vec<EventRecord> = parse_records(&request.events)?
        .into_iter()
        .map(|record| attribute_record(record, identity.as_ref()))
        .collect();
    let started_id = records.first().and_then(|record| match &record.event {
        procnote_core::event::types::Event::ExecutionStarted { execution_id, .. } => {
            Some(*execution_id)
        }
        _ => None,
    });
    if started_id != Some(id) {
        return Err(ApiError::bad_request(
            "the first event must start this execution",
        ));
    }
    let procedure = collab::procedure_name(&request.procedure).map_err(ApiError::bad_request)?;
    let snapshot =
        TemplateSnapshot::of_template(&request.template).map_err(ApiError::bad_request)?;
    blocking(move || {
        if let Ok(existing) = state.executions.records(id) {
            let same_start = existing
                .iter()
                .find(|record| {
                    matches!(
                        record.event,
                        procnote_core::event::types::Event::ExecutionStarted { .. }
                    )
                })
                .map(|record| &record.event)
                == records.first().map(|record| &record.event);
            return if same_start {
                procnote_app::execution::get_execution(state.executions.as_ref(), id)
            } else {
                Err(format!("execution {id} already exists"))
            };
        }
        let stored = sync::create_started(
            state.executions.as_ref(),
            procedure,
            snapshot,
            &records,
            env!("CARGO_PKG_VERSION").to_string(),
        )?;
        procnote_app::execution::summarize(&stored.state, stored.location.as_deref())
    })
    .await
    .map(Json)
}

async fn list_conflicts(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
) -> Result<Json<Vec<Conflict>>, ApiError> {
    blocking(move || state.executions.conflicts(id))
        .await
        .map(Json)
}

/// Mark a conflict as dealt with (re-recorded, or deliberately dropped).
async fn resolve_conflict(
    State(state): State<SharedState>,
    Path((id, event_id)): Path<(ExecutionId, uuid::Uuid)>,
    identity: Option<Extension<Identity>>,
    Json(request): Json<ResolveConflictRequest>,
) -> Result<Json<Vec<Conflict>>, ApiError> {
    let by = identity
        .map(|Extension(identity)| identity.email)
        .or(request.meta.recorded_by);
    blocking(move || {
        sync::resolve_conflict(
            state.executions.as_ref(),
            id,
            event_id,
            &Resolution {
                at: chrono::Utc::now(),
                by,
            },
        )?;
        state.executions.conflicts(id)
    })
    .await
    .map(Json)
}

#[derive(Deserialize)]
struct ResolveConflictRequest {
    #[serde(default)]
    meta: ActionMeta,
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
    blocking(move || procnote_app::execution::get_execution(state.executions.as_ref(), id))
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
        procnote_app::execution::attachment_preview_data_url(
            state.executions.as_ref(),
            id,
            &query.path,
        )
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
        let summary = procnote_app::execution::get_execution(state.executions.as_ref(), id)?;
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
    blocking(move || procnote_app::execution::get_execution(state.executions.as_ref(), id))
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
        let sources: Vec<_> = files
            .iter()
            .map(|file| (file.path.clone(), file.filename.clone()))
            .collect();
        let result =
            procnote_app::execution::receive_files(state.executions.as_ref(), id, &sources, meta);
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
