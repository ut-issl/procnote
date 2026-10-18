//! The server's device API (`/api/device/*`), called with a device token.

use procnote_app::action::AttachmentSource;
use procnote_app::sync::PushOutcome;
use procnote_core::event::types::{EventRecord, ExecutionId};
use procnote_core::template::types::ProcedureTemplate;
use serde::Deserialize;
use serde::de::DeserializeOwned;

use crate::link::ServerLink;

/// Why a request didn't get an answer from the server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The network or a gateway in front of the server (try again later).
    Unreachable(String),
    /// The device token was refused (revoked): connect again.
    Unauthorized,
    /// The server refused the request, with its reason.
    Refused(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Unreachable(e) => write!(f, "the server can't be reached: {e}"),
            Self::Unauthorized => write!(f, "this device's token was refused; connect again"),
            Self::Refused(e) => write!(f, "{e}"),
        }
    }
}

/// Gateway errors, which mean the server itself didn't answer.
const GATEWAY_STATUSES: [u16; 12] = [502, 503, 504, 520, 521, 522, 523, 524, 525, 526, 527, 530];

/// One pushed event's outcome.
#[derive(Debug, Deserialize)]
pub struct PushedEvent {
    #[serde(flatten)]
    pub outcome: PushOutcome,
}

#[derive(Debug, Deserialize)]
struct PushResponse {
    results: Vec<PushedEvent>,
}

/// Log lines from a position on, and the server's total.
#[derive(Debug, Deserialize)]
pub struct Pulled {
    #[serde(deserialize_with = "records")]
    pub records: Vec<EventRecord>,
    pub count: usize,
}

/// An execution as the server keeps it.
#[derive(Debug, Deserialize)]
pub struct RemoteExecution {
    pub procedure: String,
    pub template_format: String,
    pub template: String,
    #[serde(deserialize_with = "records")]
    pub records: Vec<EventRecord>,
}

fn records<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<Vec<EventRecord>, D::Error> {
    let values = Vec::<serde_json::Value>::deserialize(deserializer)?;
    values
        .iter()
        .map(|value| EventRecord::from_value(value).map_err(serde::de::Error::custom))
        .collect()
}

#[derive(Debug, Deserialize)]
struct UploadLink {
    token: Option<String>,
}

pub struct DeviceClient {
    http: reqwest::Client,
    link: ServerLink,
}

impl DeviceClient {
    #[must_use]
    pub fn new(link: ServerLink) -> Self {
        Self {
            http: reqwest::Client::new(),
            link,
        }
    }

    #[must_use]
    pub const fn link(&self) -> &ServerLink {
        &self.link
    }

    fn url(&self, path: &str) -> String {
        format!(
            "{}/api/device{path}",
            self.link.server.trim_end_matches('/')
        )
    }

    async fn send(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<reqwest::Response, ClientError> {
        let response = request
            .bearer_auth(&self.link.token)
            .send()
            .await
            .map_err(|e| ClientError::Unreachable(e.to_string()))?;
        let status = response.status();
        if status.is_success() {
            return Ok(response);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED {
            return Err(ClientError::Unauthorized);
        }
        let body = response.text().await.unwrap_or_default();
        let message = serde_json::from_str::<serde_json::Value>(&body)
            .ok()
            .and_then(|json| json.get("error")?.as_str().map(str::to_string));
        match message {
            Some(message) => Err(ClientError::Refused(message)),
            None if GATEWAY_STATUSES.contains(&status.as_u16()) => {
                Err(ClientError::Unreachable(format!("HTTP {status}")))
            }
            None => Err(ClientError::Refused(format!("HTTP {status}"))),
        }
    }

    async fn json<T: DeserializeOwned>(
        &self,
        request: reqwest::RequestBuilder,
    ) -> Result<T, ClientError> {
        self.send(request)
            .await?
            .json()
            .await
            .map_err(|e| ClientError::Unreachable(e.to_string()))
    }

    /// Check the token: who this device syncs as.
    pub async fn me(&self) -> Result<serde_json::Value, ClientError> {
        self.json(self.http.get(self.url("/me"))).await
    }

    /// The server's executions as raw JSON summaries.
    pub async fn execution_summaries(&self) -> Result<Vec<serde_json::Value>, ClientError> {
        self.json(self.http.get(self.url("/executions"))).await
    }

    pub async fn execution(&self, id: ExecutionId) -> Result<RemoteExecution, ClientError> {
        self.json(self.http.get(self.url(&format!("/executions/{id}"))))
            .await
    }

    pub async fn register(
        &self,
        id: ExecutionId,
        procedure: &str,
        template: &ProcedureTemplate,
        events: &[EventRecord],
    ) -> Result<(), ClientError> {
        self.send(
            self.http
                .put(self.url(&format!("/executions/{id}")))
                .json(&serde_json::json!({
                    "procedure": procedure,
                    "template": template,
                    "events": events,
                })),
        )
        .await
        .map(|_| ())
    }

    pub async fn pull(&self, id: ExecutionId, after: usize) -> Result<Pulled, ClientError> {
        self.json(
            self.http
                .get(self.url(&format!("/executions/{id}/events")))
                .query(&[("after", after)]),
        )
        .await
    }

    pub async fn push(
        &self,
        id: ExecutionId,
        events: &[EventRecord],
        uploads: &[String],
    ) -> Result<Vec<PushedEvent>, ClientError> {
        self.json::<PushResponse>(
            self.http
                .post(self.url(&format!("/executions/{id}/events")))
                .json(&serde_json::json!({ "events": events, "uploads": uploads })),
        )
        .await
        .map(|response| response.results)
    }

    /// Upload files; returns their upload tokens, in order.
    pub async fn upload(&self, files: Vec<(String, Vec<u8>)>) -> Result<Vec<String>, ClientError> {
        let mut form = reqwest::multipart::Form::new();
        for (name, content) in files {
            form = form.part(
                "file",
                reqwest::multipart::Part::bytes(content).file_name(name),
            );
        }
        let sources: Vec<AttachmentSource> = self
            .json(self.http.post(self.url("/uploads")).multipart(form))
            .await?;
        Ok(sources.into_iter().map(|source| source.path).collect())
    }

    pub async fn attachment(&self, id: ExecutionId, path: &str) -> Result<Vec<u8>, ClientError> {
        self.send(
            self.http
                .get(self.url(&format!("/executions/{id}/attachments")))
                .query(&[("path", path)]),
        )
        .await?
        .bytes()
        .await
        .map(|bytes| bytes.to_vec())
        .map_err(|e| ClientError::Unreachable(e.to_string()))
    }

    pub async fn conflicts(&self, id: ExecutionId) -> Result<Vec<serde_json::Value>, ClientError> {
        self.json(
            self.http
                .get(self.url(&format!("/executions/{id}/conflicts"))),
        )
        .await
    }

    pub async fn resolve_conflict(
        &self,
        id: ExecutionId,
        event_id: uuid::Uuid,
    ) -> Result<Vec<serde_json::Value>, ClientError> {
        self.json(
            self.http
                .post(self.url(&format!("/executions/{id}/conflicts/{event_id}/resolve")))
                .json(&serde_json::json!({})),
        )
        .await
    }

    /// The execution's phone upload link token, created if `create`.
    pub async fn upload_link(
        &self,
        id: ExecutionId,
        create: bool,
    ) -> Result<Option<String>, ClientError> {
        let url = self.url(&format!("/executions/{id}/upload-link"));
        let request = if create {
            self.http.post(url).json(&serde_json::json!({}))
        } else {
            self.http.get(url)
        };
        self.json::<UploadLink>(request)
            .await
            .map(|link| link.token)
    }

    pub async fn revoke_upload_link(&self, id: ExecutionId) -> Result<(), ClientError> {
        self.send(
            self.http
                .delete(self.url(&format!("/executions/{id}/upload-link"))),
        )
        .await
        .map(|_| ())
    }
}
