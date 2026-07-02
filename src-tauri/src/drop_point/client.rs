use serde::{Deserialize, Serialize};
use url::Url;

const ENV_URL: &str = "PROCNOTE_DROPPOINT_URL";
const ENV_API_TOKEN: &str = "PROCNOTE_DROPPOINT_API_TOKEN";
const ENV_TTL_SECONDS: &str = "PROCNOTE_DROPPOINT_TTL_SECONDS";
const ENV_MAX_BYTES: &str = "PROCNOTE_DROPPOINT_MAX_BYTES";
const CLIENT_NAME: &str = "procnote";

#[derive(Clone)]
pub struct DropPointConfig {
    pub base_url: Url,
    api_token: String,
    ttl_seconds: Option<u64>,
    max_bytes: Option<u64>,
}

impl DropPointConfig {
    pub fn from_env() -> Result<Option<Self>, String> {
        let raw_url = optional_env(ENV_URL);
        let api_token = optional_env(ENV_API_TOKEN);
        match (raw_url, api_token) {
            (None, None) => Ok(None),
            (Some(_), None) | (None, Some(_)) => Err(format!(
                "both {ENV_URL} and {ENV_API_TOKEN} must be set to enable DropPoint"
            )),
            (Some(raw_url), Some(api_token)) => {
                let base_url = parse_base_url(&raw_url)?;
                let ttl_seconds = optional_positive_u64_env(ENV_TTL_SECONDS)?;
                let max_bytes = optional_positive_u64_env(ENV_MAX_BYTES)?;
                Ok(Some(Self {
                    base_url,
                    api_token,
                    ttl_seconds,
                    max_bytes,
                }))
            }
        }
    }
}

fn optional_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn optional_positive_u64_env(name: &str) -> Result<Option<u64>, String> {
    optional_env(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|e| format!("{name} must be a positive integer: {e}"))
                .and_then(|parsed| match parsed {
                    0 => Err(format!("{name} must be positive")),
                    _ => Ok(parsed),
                })
        })
        .transpose()
}

fn parse_base_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|e| format!("{ENV_URL} is invalid: {e}"))?;
    if url.scheme().is_empty() || url.host_str().is_none() {
        return Err(format!("{ENV_URL} must include scheme and host"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{ENV_URL} must not include query or fragment"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{ENV_URL} must not include user info"));
    }
    Ok(url)
}

#[derive(Debug, thiserror::Error)]
pub enum DropPointClientError {
    #[error("invalid DropPoint endpoint URL: {0}")]
    InvalidUrl(#[from] url::ParseError),
    #[error("DropPoint request failed: {0}")]
    Request(#[from] reqwest::Error),
    #[error("DropPoint returned HTTP {status}: {body}")]
    Http {
        status: reqwest::StatusCode,
        body: String,
    },
}

#[derive(Debug, Serialize)]
struct CreateDropPointRequest {
    client_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
    single_use: bool,
}

#[derive(Debug, Deserialize)]
pub struct CreateDropPointResponse {
    pub drop_point_id: String,
    pub display_name: String,
    pub drop_link: String,
    pub pickup_token: String,
    pub expires_at: String,
    pub max_bytes: u64,
}

#[derive(Debug, Deserialize)]
pub struct DropPointStatusResponse {
    pub status: String,
    pub display_name: String,
    pub encrypted_size: u64,
    pub dropped_at: Option<String>,
    pub first_picked_up_at: Option<String>,
    pub expires_at: String,
}

#[derive(Clone)]
pub struct DropPointClient {
    config: DropPointConfig,
    http: reqwest::Client,
}

impl DropPointClient {
    #[must_use]
    pub fn new(config: DropPointConfig) -> Self {
        Self {
            config,
            http: reqwest::Client::new(),
        }
    }

    pub async fn create_drop_point(&self) -> Result<CreateDropPointResponse, DropPointClientError> {
        let request = CreateDropPointRequest {
            client_name: CLIENT_NAME.to_string(),
            ttl_seconds: self.config.ttl_seconds,
            max_bytes: self.config.max_bytes,
            single_use: true,
        };
        let response = self
            .http
            .post(self.endpoint("/api/drop-points")?)
            .bearer_auth(&self.config.api_token)
            .json(&request)
            .send()
            .await?;
        Ok(ensure_success(response).await?.json().await?)
    }

    pub async fn status(
        &self,
        drop_point_id: &str,
        pickup_token: &str,
    ) -> Result<DropPointStatusResponse, DropPointClientError> {
        let response = self
            .http
            .get(self.endpoint(&format!("/api/drop-points/{drop_point_id}/status"))?)
            .bearer_auth(pickup_token)
            .send()
            .await?;
        Ok(ensure_success(response).await?.json().await?)
    }

    pub async fn pickup(
        &self,
        drop_point_id: &str,
        pickup_token: &str,
    ) -> Result<(String, Vec<u8>), DropPointClientError> {
        let response = self
            .http
            .get(self.endpoint(&format!("/api/drop-points/{drop_point_id}/pickup"))?)
            .bearer_auth(pickup_token)
            .send()
            .await?;
        let response = ensure_success(response).await?;
        let content_type = response
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string();
        let body = response.bytes().await?.to_vec();
        Ok((content_type, body))
    }

    pub async fn close(
        &self,
        drop_point_id: &str,
        pickup_token: &str,
    ) -> Result<(), DropPointClientError> {
        let response = self
            .http
            .delete(self.endpoint(&format!("/api/drop-points/{drop_point_id}"))?)
            .bearer_auth(pickup_token)
            .send()
            .await?;
        ensure_success(response).await.map(|_| ())
    }

    fn endpoint(&self, path: &str) -> Result<Url, DropPointClientError> {
        let base = self.config.base_url.as_str().trim_end_matches('/');
        Ok(Url::parse(&format!("{base}{path}"))?)
    }
}

async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, DropPointClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = response.bytes().await.map_or_else(
        |e| format!("<failed to read response body: {e}>"),
        |bytes| String::from_utf8_lossy(&bytes).into_owned(),
    );
    Err(DropPointClientError::Http { status, body })
}
