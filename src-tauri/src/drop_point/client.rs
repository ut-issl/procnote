use serde::{Deserialize, Serialize};
use url::Url;

const ENV_URL: &str = "PROCNOTE_DROPPOINT_URL";
const ENV_API_TOKEN: &str = "PROCNOTE_DROPPOINT_API_TOKEN";
const ENV_TTL_SECONDS: &str = "PROCNOTE_DROPPOINT_TTL_SECONDS";
const ENV_MAX_BYTES: &str = "PROCNOTE_DROPPOINT_MAX_BYTES";
const CLIENT_NAME: &str = "procnote";
const DEFAULT_PICKUP_BODY_LIMIT: u64 = 100 * 1024 * 1024;
const MAX_ERROR_BODY_BYTES: u64 = 8 * 1024;

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
    if url.scheme() != "https" && !is_loopback_http_url(&url) {
        return Err(format!(
            "{ENV_URL} must use https (http is only allowed for localhost)"
        ));
    }
    Ok(url)
}

fn is_loopback_http_url(url: &Url) -> bool {
    url.scheme() == "http"
        && matches!(
            url.host_str(),
            Some("localhost" | "127.0.0.1" | "::1" | "[::1]")
        )
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
    #[error("DropPoint response body exceeded {limit} bytes")]
    BodyTooLarge { limit: u64 },
    #[error("DropPoint server returned an invalid drop_point_id")]
    InvalidDropPointId,
    #[error("DropPoint base URL cannot be used as a URL base")]
    InvalidBaseUrl,
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
            .post(self.endpoint(&["api", "drop-points"])?)
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
            .get(self.drop_point_endpoint(drop_point_id, Some("status"))?)
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
            .get(self.drop_point_endpoint(drop_point_id, Some("pickup"))?)
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
        let body = read_body_limited(response, self.pickup_body_limit()).await?;
        Ok((content_type, body))
    }

    pub async fn close(
        &self,
        drop_point_id: &str,
        pickup_token: &str,
    ) -> Result<(), DropPointClientError> {
        let response = self
            .http
            .delete(self.drop_point_endpoint(drop_point_id, None)?)
            .bearer_auth(pickup_token)
            .send()
            .await?;
        ensure_success(response).await.map(|_| ())
    }

    fn pickup_body_limit(&self) -> u64 {
        self.config.max_bytes.unwrap_or(DEFAULT_PICKUP_BODY_LIMIT)
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, DropPointClientError> {
        let mut url = self.config.base_url.clone();
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| DropPointClientError::InvalidBaseUrl)?;
            path.pop_if_empty();
            path.extend(segments);
        }
        Ok(url)
    }

    fn drop_point_endpoint(
        &self,
        drop_point_id: &str,
        suffix: Option<&str>,
    ) -> Result<Url, DropPointClientError> {
        validate_drop_point_id(drop_point_id)?;
        let mut segments = vec!["api", "drop-points", drop_point_id];
        if let Some(suffix) = suffix {
            segments.push(suffix);
        }
        self.endpoint(&segments)
    }
}

fn validate_drop_point_id(drop_point_id: &str) -> Result<(), DropPointClientError> {
    (!drop_point_id.is_empty()
        && drop_point_id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')))
    .then_some(())
    .ok_or(DropPointClientError::InvalidDropPointId)
}

async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, DropPointClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let body = match read_body_limited(response, MAX_ERROR_BODY_BYTES).await {
        Ok(bytes) => String::from_utf8_lossy(&bytes).into_owned(),
        Err(DropPointClientError::BodyTooLarge { .. }) => {
            format!("<error body exceeded {MAX_ERROR_BODY_BYTES} bytes>")
        }
        Err(e) => format!("<failed to read response body: {e}>"),
    };
    Err(DropPointClientError::Http { status, body })
}

async fn read_body_limited(
    mut response: reqwest::Response,
    limit: u64,
) -> Result<Vec<u8>, DropPointClientError> {
    if response
        .content_length()
        .is_some_and(|content_length| content_length > limit)
    {
        return Err(DropPointClientError::BodyTooLarge { limit });
    }

    let capacity = response
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default();
    let mut body = Vec::with_capacity(capacity);
    while let Some(chunk) = response.chunk().await? {
        let next_len = body.len().saturating_add(chunk.len());
        if u64::try_from(next_len).map_or(true, |next_len| next_len > limit) {
            return Err(DropPointClientError::BodyTooLarge { limit });
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}
