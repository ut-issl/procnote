use std::fmt;
use std::time::Duration;

use reqwest::header::{ACCEPT, CONTENT_TYPE};
use serde::{Deserialize, Serialize};
use unicode_general_category::{GeneralCategory, get_general_category};
use url::Url;
use zeroize::Zeroizing;

use crate::drop_point::manifest::validate_rfc3339_timestamp;

const ENV_URL: &str = "PROCNOTE_DROPPOINT_URL";
const ENV_API_TOKEN: &str = "PROCNOTE_DROPPOINT_API_TOKEN";
const ENV_TTL_SECONDS: &str = "PROCNOTE_DROPPOINT_TTL_SECONDS";
const ENV_MAX_BYTES: &str = "PROCNOTE_DROPPOINT_MAX_BYTES";
const CLIENT_NAME: &str = "procnote";
const USER_AGENT: &str = "procnote-droppoint-receiver/2";
const JSON_BODY_LIMIT: u64 = 64 * 1024;
const MAX_ERROR_BODY_BYTES: u64 = 8 * 1024;
const MAX_ENVELOPE_BYTES: u64 = 1 << 20;
const MAX_MULTIPART_OVERHEAD_BYTES: u64 = 64 * 1024;
const MAX_PROTOCOL_BYTES: u64 = 1 << 40;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const REQUEST_TIMEOUT: Duration = Duration::from_mins(2);

#[derive(Clone)]
pub struct DropPointConfig {
    pub base_url: Url,
    api_token: Option<Zeroizing<String>>,
    ttl_seconds: Option<u64>,
    max_bytes: Option<u64>,
}

impl DropPointConfig {
    pub fn from_env() -> Result<Option<Self>, String> {
        let raw_url = optional_trimmed_env(ENV_URL);
        let api_token = optional_secret_env(ENV_API_TOKEN);
        match (raw_url, api_token) {
            (None, None) => Ok(None),
            (Some(_), None) | (None, Some(_)) => Err(format!(
                "both {ENV_URL} and {ENV_API_TOKEN} must be set to enable DropPoint"
            )),
            (Some(raw_url), Some(api_token)) => {
                let api_token = Zeroizing::new(api_token);
                let base_url = parse_base_url(&raw_url)?;
                validate_capability(&api_token, "api_")
                    .map_err(|()| format!("{ENV_API_TOKEN} has an invalid token format"))?;
                let ttl_seconds = optional_positive_u64_env(ENV_TTL_SECONDS)?;
                let max_bytes = optional_positive_u64_env(ENV_MAX_BYTES)?;
                if max_bytes.is_some_and(|value| value > MAX_PROTOCOL_BYTES) {
                    return Err(format!(
                        "{ENV_MAX_BYTES} must not exceed {MAX_PROTOCOL_BYTES}"
                    ));
                }
                Ok(Some(Self {
                    base_url,
                    api_token: Some(api_token),
                    ttl_seconds,
                    max_bytes,
                }))
            }
        }
    }

    #[cfg(test)]
    pub fn for_test(base_url: Url) -> Self {
        Self {
            base_url,
            api_token: Some(Zeroizing::new("api_test".to_string())),
            ttl_seconds: None,
            max_bytes: None,
        }
    }
}

fn optional_trimmed_env(name: &str) -> Option<String> {
    std::env::var(name)
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn optional_secret_env(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

fn optional_positive_u64_env(name: &str) -> Result<Option<u64>, String> {
    optional_trimmed_env(name)
        .map(|value| {
            value
                .parse::<u64>()
                .map_err(|error| format!("{name} must be a positive integer: {error}"))
                .and_then(|parsed| match parsed {
                    0 => Err(format!("{name} must be positive")),
                    _ => Ok(parsed),
                })
        })
        .transpose()
}

pub fn parse_base_url(raw: &str) -> Result<Url, String> {
    let url = Url::parse(raw).map_err(|error| format!("{ENV_URL} is invalid: {error}"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(format!("{ENV_URL} must be an HTTP(S) origin"));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(format!("{ENV_URL} must not include user info"));
    }
    if !matches!(url.path(), "" | "/") {
        return Err(format!("{ENV_URL} must not include a path prefix"));
    }
    if url.query().is_some() || url.fragment().is_some() {
        return Err(format!("{ENV_URL} must not include query or fragment"));
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteTerminal {
    Closed,
    Expired,
    Failed,
    NotFound,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ApiErrorCode {
    DropNotReady,
    DropPointClosed,
    DropPointExpired,
    DropPointFailed,
    DropPointNotFound,
    PayloadUnavailable,
    Other,
    Missing,
}

impl fmt::Display for ApiErrorCode {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::DropNotReady => formatter.write_str("drop_not_ready"),
            Self::DropPointClosed => formatter.write_str("drop_point_closed"),
            Self::DropPointExpired => formatter.write_str("drop_point_expired"),
            Self::DropPointFailed => formatter.write_str("drop_point_failed"),
            Self::DropPointNotFound => formatter.write_str("drop_point_not_found"),
            Self::PayloadUnavailable => formatter.write_str("payload_unavailable"),
            Self::Other => formatter.write_str("unknown_error"),
            Self::Missing => formatter.write_str("unparsable_error"),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DropPointClientError {
    #[error("DropPoint transport failed: {0}")]
    Transport(String),
    #[error("DropPoint returned HTTP {status} ({code})")]
    Http {
        status: reqwest::StatusCode,
        code: ApiErrorCode,
    },
    #[error("DropPoint response body exceeded {limit} bytes")]
    BodyTooLarge { limit: u64 },
    #[error("DropPoint response body could not be allocated within its bound")]
    BodyAllocation,
    #[error("DropPoint response is invalid: {0}")]
    InvalidResponse(String),
    #[error("DropPoint client is not configured to create drop points")]
    CreateNotConfigured,
    #[error("DropPoint server returned an invalid drop_point_id")]
    InvalidDropPointId,
    #[error("DropPoint endpoint could not be constructed")]
    InvalidBaseUrl,
}

impl DropPointClientError {
    #[must_use]
    pub fn is_retryable(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Http { status, .. } => {
                status.is_server_error()
                    || matches!(
                        *status,
                        reqwest::StatusCode::REQUEST_TIMEOUT
                            | reqwest::StatusCode::TOO_MANY_REQUESTS
                    )
            }
            Self::BodyTooLarge { .. }
            | Self::BodyAllocation
            | Self::InvalidResponse(_)
            | Self::CreateNotConfigured
            | Self::InvalidDropPointId
            | Self::InvalidBaseUrl => false,
        }
    }

    #[must_use]
    pub fn is_not_ready(&self) -> bool {
        matches!(
            self,
            Self::Http {
                status: reqwest::StatusCode::CONFLICT,
                code: ApiErrorCode::DropNotReady,
            }
        )
    }

    #[must_use]
    pub fn terminal(&self) -> Option<RemoteTerminal> {
        match self {
            Self::Http {
                status: reqwest::StatusCode::NOT_FOUND,
                code: ApiErrorCode::DropPointNotFound | ApiErrorCode::Missing | ApiErrorCode::Other,
            } => Some(RemoteTerminal::NotFound),
            Self::Http {
                status: reqwest::StatusCode::GONE,
                code: ApiErrorCode::DropPointClosed,
            } => Some(RemoteTerminal::Closed),
            Self::Http {
                status: reqwest::StatusCode::GONE,
                code: ApiErrorCode::DropPointExpired,
            } => Some(RemoteTerminal::Expired),
            Self::Http {
                status: reqwest::StatusCode::GONE,
                code: ApiErrorCode::DropPointFailed,
            } => Some(RemoteTerminal::Failed),
            _ => None,
        }
    }
}

#[derive(Debug, Serialize)]
struct CreateDropPointRequest {
    client_name: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ttl_seconds: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    max_bytes: Option<u64>,
    single_use: bool,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CreateDropPointResponse {
    pub drop_point_id: String,
    pub display_name: String,
    pub drop_link: String,
    pub pickup_token: Zeroizing<String>,
    pub expires_at: String,
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RelayStatus {
    Open,
    Receiving,
    Ready,
    Closed,
    Expired,
    Failed,
}

impl RelayStatus {
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Receiving => "receiving",
            Self::Ready => "ready",
            Self::Closed => "closed",
            Self::Expired => "expired",
            Self::Failed => "failed",
        }
    }

    #[must_use]
    pub const fn terminal(self) -> Option<RemoteTerminal> {
        match self {
            Self::Closed => Some(RemoteTerminal::Closed),
            Self::Expired => Some(RemoteTerminal::Expired),
            Self::Failed => Some(RemoteTerminal::Failed),
            Self::Open | Self::Receiving | Self::Ready => None,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DropPointStatusResponse {
    pub status: RelayStatus,
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
    pub fn new(config: DropPointConfig) -> Result<Self, String> {
        let http = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(REQUEST_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| format!("failed to initialize DropPoint HTTP client: {error}"))?;
        Ok(Self { config, http })
    }

    pub fn for_receiver(base_url: &str) -> Result<Self, String> {
        let base_url = parse_base_url(base_url)?;
        Self::new(DropPointConfig {
            base_url,
            api_token: None,
            ttl_seconds: None,
            max_bytes: None,
        })
    }

    #[must_use]
    pub const fn base_url(&self) -> &Url {
        &self.config.base_url
    }

    pub async fn create_drop_point(&self) -> Result<CreateDropPointResponse, DropPointClientError> {
        let api_token = self
            .config
            .api_token
            .as_deref()
            .ok_or(DropPointClientError::CreateNotConfigured)?;
        let request = CreateDropPointRequest {
            client_name: CLIENT_NAME,
            ttl_seconds: self.config.ttl_seconds,
            max_bytes: self.config.max_bytes,
            single_use: true,
        };
        let response = self
            .http
            .post(self.endpoint(&["api", "drop-points"])?)
            .bearer_auth(api_token.as_str())
            .header(ACCEPT, "application/json")
            .json(&request)
            .send()
            .await
            .map_err(transport_error)?;
        let response = ensure_success(response).await?;
        require_json_content_type(&response)?;
        let bytes = Zeroizing::new(read_body_limited(response, JSON_BODY_LIMIT).await?);
        let created: CreateDropPointResponse = parse_json(&bytes, "create response")?;
        validate_create_response(&created, &self.config.base_url)?;
        Ok(created)
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
            .header(ACCEPT, "application/json")
            .send()
            .await
            .map_err(transport_error)?;
        let response = ensure_success(response).await?;
        require_json_content_type(&response)?;
        let bytes = Zeroizing::new(read_body_limited(response, JSON_BODY_LIMIT).await?);
        let status: DropPointStatusResponse = parse_json(&bytes, "status response")?;
        validate_status_response(&status)?;
        Ok(status)
    }

    pub async fn pickup(
        &self,
        drop_point_id: &str,
        pickup_token: &str,
        max_payload_bytes: u64,
    ) -> Result<(String, Vec<u8>), DropPointClientError> {
        if max_payload_bytes == 0 || max_payload_bytes > MAX_PROTOCOL_BYTES {
            return Err(DropPointClientError::InvalidResponse(
                "persisted max_bytes is outside the protocol range".to_string(),
            ));
        }
        let response = self
            .http
            .get(self.drop_point_endpoint(drop_point_id, Some("pickup"))?)
            .bearer_auth(pickup_token)
            .header(ACCEPT, "multipart/mixed")
            .send()
            .await
            .map_err(transport_error)?;
        let response = ensure_success(response).await?;
        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .ok_or_else(|| {
                DropPointClientError::InvalidResponse(
                    "pickup response is missing a valid Content-Type".to_string(),
                )
            })?
            .to_string();
        let body_limit = max_payload_bytes
            .checked_add(MAX_ENVELOPE_BYTES)
            .and_then(|limit| limit.checked_add(MAX_MULTIPART_OVERHEAD_BYTES))
            .ok_or_else(|| {
                DropPointClientError::InvalidResponse(
                    "pickup response limit arithmetic overflowed".to_string(),
                )
            })?;
        let body = read_body_limited(response, body_limit).await?;
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
            .await
            .map_err(transport_error)?;
        let response = ensure_success(response).await?;
        if response.status() != reqwest::StatusCode::NO_CONTENT {
            return Err(DropPointClientError::InvalidResponse(
                "close response must use HTTP 204".to_string(),
            ));
        }
        Ok(())
    }

    fn endpoint(&self, segments: &[&str]) -> Result<Url, DropPointClientError> {
        let mut url = self.config.base_url.clone();
        url.set_path("/");
        {
            let mut path = url
                .path_segments_mut()
                .map_err(|()| DropPointClientError::InvalidBaseUrl)?;
            path.clear();
            path.extend(segments);
        }
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }

    fn drop_point_endpoint(
        &self,
        drop_point_id: &str,
        suffix: Option<&str>,
    ) -> Result<Url, DropPointClientError> {
        validate_capability(drop_point_id, "dp_")
            .map_err(|()| DropPointClientError::InvalidDropPointId)?;
        let mut segments = vec!["api", "drop-points", drop_point_id];
        if let Some(suffix) = suffix {
            segments.push(suffix);
        }
        self.endpoint(&segments)
    }
}

fn validate_create_response(
    created: &CreateDropPointResponse,
    base_url: &Url,
) -> Result<(), DropPointClientError> {
    validate_capability(&created.drop_point_id, "dp_").map_err(|()| {
        DropPointClientError::InvalidResponse("create response has an invalid ID".to_string())
    })?;
    validate_capability(&created.pickup_token, "pick_").map_err(|()| {
        DropPointClientError::InvalidResponse(
            "create response has an invalid pickup capability".to_string(),
        )
    })?;
    validate_display_name(&created.display_name)?;
    validate_timestamp(&created.expires_at, "expires_at")?;
    if created.max_bytes == 0 || created.max_bytes > MAX_PROTOCOL_BYTES {
        return Err(DropPointClientError::InvalidResponse(
            "create response max_bytes is outside the protocol range".to_string(),
        ));
    }
    validate_drop_link(&created.drop_link, base_url)
}

fn validate_status_response(status: &DropPointStatusResponse) -> Result<(), DropPointClientError> {
    validate_display_name(&status.display_name)?;
    validate_timestamp(&status.expires_at, "expires_at")?;
    if let Some(value) = &status.dropped_at {
        validate_timestamp(value, "dropped_at")?;
    }
    if let Some(value) = &status.first_picked_up_at {
        validate_timestamp(value, "first_picked_up_at")?;
    }
    Ok(())
}

fn validate_display_name(value: &str) -> Result<(), DropPointClientError> {
    if value.trim().is_empty()
        || value.len() > 128
        || value.chars().any(|character| {
            matches!(
                get_general_category(character),
                GeneralCategory::Control | GeneralCategory::Format
            )
        })
    {
        return Err(DropPointClientError::InvalidResponse(
            "response has an invalid display_name".to_string(),
        ));
    }
    Ok(())
}

fn validate_timestamp(value: &str, field: &str) -> Result<(), DropPointClientError> {
    validate_rfc3339_timestamp(value).map_err(|_| {
        DropPointClientError::InvalidResponse(format!(
            "response field {field} is not an RFC3339 timestamp"
        ))
    })
}

fn validate_drop_link(value: &str, base_url: &Url) -> Result<(), DropPointClientError> {
    let link = Url::parse(value).map_err(|_| {
        DropPointClientError::InvalidResponse("create response drop_link is invalid".to_string())
    })?;
    if !link.username().is_empty()
        || link.password().is_some()
        || link.query().is_some()
        || link.fragment().is_some()
        || !same_origin(base_url, &link)
    {
        return Err(DropPointClientError::InvalidResponse(
            "create response drop_link is not a fragment-free link on the relay origin".to_string(),
        ));
    }
    let segments = link
        .path_segments()
        .map(Iterator::collect::<Vec<_>>)
        .unwrap_or_default();
    if segments.len() != 2
        || segments.first().copied() != Some("drop")
        || segments
            .get(1)
            .is_none_or(|token| validate_capability(token, "drop_").is_err())
    {
        return Err(DropPointClientError::InvalidResponse(
            "create response drop_link has an invalid path".to_string(),
        ));
    }
    Ok(())
}

pub fn same_origin(expected: &Url, actual: &Url) -> bool {
    expected.scheme() == actual.scheme()
        && expected.host_str() == actual.host_str()
        && expected.port_or_known_default() == actual.port_or_known_default()
}

fn validate_capability(value: &str, prefix: &str) -> Result<(), ()> {
    value
        .strip_prefix(prefix)
        .filter(|suffix| {
            !suffix.is_empty()
                && suffix
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
        })
        .map(|_| ())
        .ok_or(())
}

async fn ensure_success(
    response: reqwest::Response,
) -> Result<reqwest::Response, DropPointClientError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    let code = read_body_limited(response, MAX_ERROR_BODY_BYTES)
        .await
        .map_or(ApiErrorCode::Missing, |bytes| {
            parse_api_error_code(&Zeroizing::new(bytes))
        });
    Err(DropPointClientError::Http { status, code })
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiErrorEnvelope {
    error: ApiErrorBody,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ApiErrorBody {
    code: Zeroizing<String>,
    message: Zeroizing<String>,
}

fn parse_api_error_code(bytes: &[u8]) -> ApiErrorCode {
    let Ok(envelope) = serde_json::from_slice::<ApiErrorEnvelope>(bytes) else {
        return ApiErrorCode::Missing;
    };
    let _ = &envelope.error.message;
    match envelope.error.code.as_str() {
        "drop_not_ready" => ApiErrorCode::DropNotReady,
        "drop_point_closed" => ApiErrorCode::DropPointClosed,
        "drop_point_expired" => ApiErrorCode::DropPointExpired,
        "drop_point_failed" => ApiErrorCode::DropPointFailed,
        "drop_point_not_found" => ApiErrorCode::DropPointNotFound,
        "payload_unavailable" => ApiErrorCode::PayloadUnavailable,
        _ => ApiErrorCode::Other,
    }
}

fn require_json_content_type(response: &reqwest::Response) -> Result<(), DropPointClientError> {
    let content_type = response
        .headers()
        .get(CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .ok_or_else(|| {
            DropPointClientError::InvalidResponse(
                "JSON response is missing a valid Content-Type".to_string(),
            )
        })?;
    let media_type = content_type
        .split_once(';')
        .map_or(content_type, |(media_type, _)| media_type)
        .trim();
    if media_type.eq_ignore_ascii_case("application/json") {
        Ok(())
    } else {
        Err(DropPointClientError::InvalidResponse(
            "response Content-Type must be application/json".to_string(),
        ))
    }
}

fn parse_json<T: for<'de> Deserialize<'de>>(
    bytes: &[u8],
    label: &str,
) -> Result<T, DropPointClientError> {
    serde_json::from_slice(bytes)
        .map_err(|_| DropPointClientError::InvalidResponse(format!("{label} is not strict JSON")))
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

    let initial_capacity = response
        .content_length()
        .map(|length| length.min(1024 * 1024))
        .and_then(|length| usize::try_from(length).ok())
        .unwrap_or_default();
    let mut body = Vec::new();
    body.try_reserve(initial_capacity)
        .map_err(|_| DropPointClientError::BodyAllocation)?;
    while let Some(chunk) = response.chunk().await.map_err(transport_error)? {
        let next_len = body
            .len()
            .checked_add(chunk.len())
            .ok_or(DropPointClientError::BodyTooLarge { limit })?;
        if u64::try_from(next_len).map_or(true, |next_len| next_len > limit) {
            return Err(DropPointClientError::BodyTooLarge { limit });
        }
        body.try_reserve(chunk.len())
            .map_err(|_| DropPointClientError::BodyAllocation)?;
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

fn transport_error(error: reqwest::Error) -> DropPointClientError {
    DropPointClientError::Transport(redact_capabilities(&error.without_url().to_string()))
}

pub fn redact_capabilities(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut output = String::with_capacity(value.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if let Some(prefix_length) = capability_prefix_length(&bytes[cursor..]) {
            let mut end = cursor + prefix_length;
            let token_start = end;
            while end < bytes.len() {
                if is_capability_byte(bytes[end]) {
                    end += 1;
                } else if percent_encoded_byte(&bytes[end..]).is_some_and(is_capability_byte) {
                    end += 3;
                } else {
                    break;
                }
            }
            if end > token_start {
                output.push_str("<redacted-capability>");
                cursor = end;
                continue;
            }
        }
        let character = value[cursor..]
            .chars()
            .next()
            .expect("cursor always points to a character boundary");
        output.push(character);
        cursor += character.len_utf8();
    }
    output
}

fn capability_prefix_length(value: &[u8]) -> Option<usize> {
    [b"drop".as_slice(), b"pick".as_slice(), b"api".as_slice()]
        .into_iter()
        .find_map(|prefix| {
            let remainder = value.strip_prefix(prefix)?;
            if remainder.starts_with(b"_") {
                Some(prefix.len() + 1)
            } else if remainder
                .get(..3)
                .and_then(percent_encoded_byte)
                .is_some_and(|byte| byte == b'_')
            {
                Some(prefix.len() + 3)
            } else {
                None
            }
        })
}

fn percent_encoded_byte(value: &[u8]) -> Option<u8> {
    if value.first().copied()? != b'%' {
        return None;
    }
    let high = hex_value(*value.get(1)?)?;
    let low = hex_value(*value.get(2)?)?;
    Some((high << 4) | low)
}

const fn hex_value(value: u8) -> Option<u8> {
    match value {
        b'0'..=b'9' => Some(value - b'0'),
        b'a'..=b'f' => Some(value - b'a' + 10),
        b'A'..=b'F' => Some(value - b'A' + 10),
        _ => None,
    }
}

const fn is_capability_byte(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_')
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    #[test]
    fn accepts_only_root_relay_origins() {
        assert!(parse_base_url("https://drop.example.com").is_ok());
        assert!(parse_base_url("https://drop.example.com/").is_ok());
        assert!(parse_base_url("http://localhost:8080").is_ok());
        for invalid in [
            "ftp://drop.example.com",
            "http://drop.example.com",
            "https://user@drop.example.com",
            "https://drop.example.com/prefix",
            "https://drop.example.com/?query=yes",
            "https://drop.example.com/#fragment",
        ] {
            assert!(parse_base_url(invalid).is_err(), "accepted {invalid}");
        }
    }

    #[test]
    fn receiver_only_client_does_not_require_or_allow_an_api_token() {
        let client = DropPointClient::for_receiver("http://127.0.0.1:1").unwrap();
        let result = tauri::async_runtime::block_on(client.create_drop_point());
        assert!(matches!(
            result,
            Err(DropPointClientError::CreateNotConfigured)
        ));
    }

    #[test]
    fn constructs_endpoints_from_the_origin_root() {
        let config = DropPointConfig::for_test(Url::parse("https://drop.example.com/").unwrap());
        let client = DropPointClient::new(config).unwrap();
        assert_eq!(
            client
                .drop_point_endpoint("dp_example", Some("pickup"))
                .unwrap()
                .as_str(),
            "https://drop.example.com/api/drop-points/dp_example/pickup"
        );
        assert!(client.drop_point_endpoint("../secret", None).is_err());
    }

    #[test]
    fn create_request_omits_defaults_and_never_emits_null_or_false() {
        let request = CreateDropPointRequest {
            client_name: CLIENT_NAME,
            ttl_seconds: None,
            max_bytes: None,
            single_use: true,
        };
        let value = serde_json::to_value(request).unwrap();
        assert_eq!(value["client_name"], CLIENT_NAME);
        assert_eq!(value["single_use"], true);
        assert!(value.get("ttl_seconds").is_none());
        assert!(value.get("max_bytes").is_none());
    }

    #[test]
    fn parses_all_six_statuses_and_rejects_unknown_status() {
        for status in ["open", "receiving", "ready", "closed", "expired", "failed"] {
            let json = format!(
                r#"{{"status":"{status}","display_name":"calm-otter","encrypted_size":0,"dropped_at":null,"first_picked_up_at":null,"expires_at":"2026-06-30T12:15:00Z"}}"#
            );
            let parsed: DropPointStatusResponse = parse_json(json.as_bytes(), "status").unwrap();
            assert_eq!(parsed.status.as_str(), status);
        }
        let unknown = br#"{"status":"waiting","display_name":"calm-otter","encrypted_size":0,"dropped_at":null,"first_picked_up_at":null,"expires_at":"2026-06-30T12:15:00Z"}"#;
        assert!(parse_json::<DropPointStatusResponse>(unknown, "status").is_err());
    }

    #[test]
    fn strict_api_response_parsing_rejects_unknown_duplicate_and_wrong_types() {
        let cases: &[&[u8]] = &[
            br#"{"status":"open","status":"ready","display_name":"calm-otter","encrypted_size":0,"dropped_at":null,"first_picked_up_at":null,"expires_at":"2026-06-30T12:15:00Z"}"#,
            br#"{"status":"open","display_name":"calm-otter","encrypted_size":0,"dropped_at":null,"first_picked_up_at":null,"expires_at":"2026-06-30T12:15:00Z","extra":1}"#,
            br#"{"status":"open","display_name":"calm-otter","encrypted_size":true,"dropped_at":null,"first_picked_up_at":null,"expires_at":"2026-06-30T12:15:00Z"}"#,
        ];
        for case in cases {
            assert!(parse_json::<DropPointStatusResponse>(case, "status").is_err());
        }
    }

    #[test]
    fn validates_create_response_fields_and_drop_link() {
        let base = Url::parse("https://drop.example.com").unwrap();
        let response = CreateDropPointResponse {
            drop_point_id: "dp_example".to_string(),
            display_name: "calm-otter".to_string(),
            drop_link: "https://drop.example.com/drop/drop_example".to_string(),
            pickup_token: Zeroizing::new("pick_example".to_string()),
            expires_at: "2026-06-30T12:15:00Z".to_string(),
            max_bytes: 1024,
        };
        assert!(validate_create_response(&response, &base).is_ok());
    }

    #[test]
    fn classifies_documented_endpoint_outcomes() {
        let not_ready = DropPointClientError::Http {
            status: reqwest::StatusCode::CONFLICT,
            code: ApiErrorCode::DropNotReady,
        };
        assert!(not_ready.is_not_ready());
        let failed = DropPointClientError::Http {
            status: reqwest::StatusCode::GONE,
            code: ApiErrorCode::DropPointFailed,
        };
        assert_eq!(failed.terminal(), Some(RemoteTerminal::Failed));
        let unavailable = DropPointClientError::Http {
            status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
            code: ApiErrorCode::PayloadUnavailable,
        };
        assert!(unavailable.is_retryable());
    }

    #[test]
    fn redacts_capabilities_in_plain_encoded_and_malformed_urls() {
        let sensitive = concat!(
            "https://api_user@pick_secret.example/drop/drop_abc?api_token=api_xyz#drop_fragment ",
            "https://example.invalid/drop%5Fencoded/more pick_tail"
        );
        let redacted = redact_capabilities(sensitive);
        for secret in [
            "api_user",
            "pick_secret",
            "drop_abc",
            "api_xyz",
            "drop%5Fencoded",
            "pick_tail",
            "drop_fragment",
        ] {
            assert!(!redacted.contains(secret));
        }
        assert!(redacted.contains("<redacted-capability>"));
    }

    #[test]
    fn never_echoes_api_error_messages() {
        let body =
            br#"{"error":{"code":"drop_point_failed","message":"pick_secret and a private key"}}"#;
        assert_eq!(parse_api_error_code(body), ApiErrorCode::DropPointFailed);
        let error = DropPointClientError::Http {
            status: reqwest::StatusCode::GONE,
            code: parse_api_error_code(body),
        };
        let rendered = error.to_string();
        assert!(!rendered.contains("pick_secret"));
        assert!(!rendered.contains("private key"));
    }
}
