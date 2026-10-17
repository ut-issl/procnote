//! HTTP API server exposing the same operations as the desktop IPC commands.
//!
//! Storage is the same procedures directory layout the desktop app uses, so a
//! workspace can be opened from both at once. There is no authentication yet;
//! run it only on a trusted network.

mod uploads;

use std::path::PathBuf;
use std::sync::Arc;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde::Deserialize;
use tokio::io::AsyncWriteExt;
use tower_http::services::{ServeDir, ServeFile};

use procnote_app::action::{AttachmentSource, ExecutionAction};
use procnote_app::execution::{ExecutionSummary, infer_attachment_content_type};
use procnote_app::template::TemplateSummary;
use procnote_core::event::types::ExecutionId;
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
}

struct AppState {
    procedures_dir: PathBuf,
    uploads: UploadStore,
}

type SharedState = Arc<AppState>;

/// Build the application router.
pub fn router(config: ServerConfig) -> std::io::Result<Router> {
    let state = Arc::new(AppState {
        procedures_dir: config.procedures_dir,
        uploads: UploadStore::new(config.upload_dir)?,
    });

    let api = Router::new()
        .route("/templates", get(list_templates))
        .route("/executions", get(list_executions).post(start_execution))
        .route("/executions/{id}", get(get_execution))
        .route("/executions/{id}/actions", post(record_action))
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
struct StartExecutionRequest {
    template_path: String,
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

async fn record_action(
    State(state): State<SharedState>,
    Path(id): Path<ExecutionId>,
    Json(mut action): Json<ExecutionAction>,
) -> Result<Json<ExecutionSummary>, ApiError> {
    blocking(move || {
        let tokens = state.uploads.authorize(&mut action)?;
        let result = procnote_app::execution::record_action(&state.procedures_dir, id, action);
        // Uploads are single-use, like the desktop file picker's grants.
        for token in tokens {
            state.uploads.discard(token);
        }
        result
    })
    .await
    .map(Json)
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

/// Stage uploaded files and return references usable in attachment actions.
///
/// Every multipart field with a filename is treated as one file.
async fn upload(
    State(state): State<SharedState>,
    mut multipart: Multipart,
) -> Result<Json<Vec<AttachmentSource>>, ApiError> {
    let mut sources = Vec::new();
    while let Some(mut field) = multipart
        .next_field()
        .await
        .map_err(|e| ApiError::bad_request(e.to_string()))?
    {
        let Some(filename) = field.file_name().map(str::to_string) else {
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
        sources.push(AttachmentSource {
            content_type: infer_attachment_content_type(&staged_name),
            filename: staged_name,
            path: token.to_string(),
        });
    }
    if sources.is_empty() {
        return Err(ApiError::bad_request("no files in upload"));
    }
    Ok(Json(sources))
}
