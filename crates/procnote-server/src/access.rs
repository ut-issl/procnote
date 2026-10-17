//! Who is calling, as established by Cloudflare Access (migration plan §3.4).
//!
//! procnote has no login of its own. It is published through Cloudflare
//! Tunnel behind Cloudflare Access, which signs in users (Google accounts on
//! an allowlist) and forwards each request with a signed application token.
//! The server verifies that token on every request and records the token's
//! `email` as the author of what the request records.
//!
//! See <https://developers.cloudflare.com/cloudflare-one/access-controls/applications/http-apps/authorization-cookie/validating-json/>.

use std::sync::RwLock;
use std::time::{Duration, Instant};

use axum::http::HeaderMap;
use jsonwebtoken::jwk::JwkSet;
use jsonwebtoken::{Algorithm, DecodingKey, Validation, decode, decode_header};
use serde::{Deserialize, Serialize};

/// Header Access adds to every request it forwards to the origin.
pub const TOKEN_HEADER: &str = "Cf-Access-Jwt-Assertion";
/// Cookie holding the same token for browser requests.
pub const TOKEN_COOKIE: &str = "CF_Authorization";

/// Minimum time between refetches of the signing keys on an unknown `kid`.
const KEY_REFRESH_INTERVAL: Duration = Duration::from_secs(30);

/// The Access application this server sits behind.
#[derive(Debug, Clone)]
pub struct AccessConfig {
    /// Team domain, e.g. `https://lab.cloudflareaccess.com` (the token's `iss`).
    pub team_domain: String,
    /// The Access application's Audience (AUD) tag.
    pub audience: String,
}

impl AccessConfig {
    fn certs_url(&self) -> String {
        format!(
            "{}/cdn-cgi/access/certs",
            self.team_domain.trim_end_matches('/')
        )
    }
}

/// The signed-in user making a request.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct Identity {
    pub email: String,
}

#[derive(Debug, PartialEq, Eq)]
pub enum AuthError {
    /// No token in the header or the cookie.
    Missing,
    /// The token's header could not be read or names no key.
    Malformed,
    /// The token was signed with a key that is not (yet) known.
    UnknownKey(String),
    /// Signature, audience, issuer, or expiry check failed.
    Invalid(String),
    /// A valid token without a user email (e.g. a service token).
    NoEmail,
}

impl std::fmt::Display for AuthError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Missing => write!(f, "no Cloudflare Access token"),
            Self::Malformed => write!(f, "malformed Cloudflare Access token"),
            Self::UnknownKey(kid) => write!(f, "token signed with unknown key {kid}"),
            Self::Invalid(reason) => write!(f, "invalid Cloudflare Access token: {reason}"),
            Self::NoEmail => write!(f, "Cloudflare Access token has no user email"),
        }
    }
}

#[derive(Deserialize)]
struct Claims {
    #[serde(default)]
    email: Option<String>,
}

/// The Access token of a request: the header if present, else the cookie.
pub fn token_from_headers(headers: &HeaderMap) -> Option<&str> {
    if let Some(token) = headers.get(TOKEN_HEADER).and_then(|v| v.to_str().ok()) {
        return Some(token);
    }
    headers
        .get_all(axum::http::header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|cookies| cookies.split(';'))
        .find_map(|cookie| {
            let (name, value) = cookie.split_once('=')?;
            (name.trim() == TOKEN_COOKIE).then(|| value.trim())
        })
}

/// Verify an Access application token against the team's signing keys.
///
/// Checks the RS256 signature with the key named by the token's `kid`, the
/// audience, the issuer, and the expiry, all of which must be present.
pub fn verify_token(
    token: &str,
    keys: &JwkSet,
    config: &AccessConfig,
) -> Result<Identity, AuthError> {
    let header = decode_header(token).map_err(|_| AuthError::Malformed)?;
    let kid = header.kid.ok_or(AuthError::Malformed)?;
    let jwk = keys
        .find(&kid)
        .ok_or_else(|| AuthError::UnknownKey(kid.clone()))?;
    let key = DecodingKey::from_jwk(jwk).map_err(|e| AuthError::Invalid(e.to_string()))?;

    let mut validation = Validation::new(Algorithm::RS256);
    validation.set_audience(&[&config.audience]);
    validation.set_issuer(&[config.team_domain.trim_end_matches('/')]);
    validation.set_required_spec_claims(&["exp", "iss", "aud"]);

    let claims = decode::<Claims>(token, &key, &validation)
        .map_err(|e| AuthError::Invalid(e.to_string()))?
        .claims;
    match claims.email {
        Some(email) if !email.is_empty() => Ok(Identity { email }),
        _ => Err(AuthError::NoEmail),
    }
}

/// Verifies tokens, fetching the team's signing keys as needed.
///
/// Keys are fetched on first use and again when a token names an unknown key
/// (Access rotates keys every six weeks), at most once per
/// [`KEY_REFRESH_INTERVAL`]. Requests that need keys while a fetch is in
/// flight wait for it and use its result.
pub struct AccessVerifier {
    config: AccessConfig,
    client: reqwest::Client,
    keys: RwLock<JwkSet>,
    /// Held while fetching; records when the last fetch started.
    fetch: tokio::sync::Mutex<Option<Instant>>,
}

impl AccessVerifier {
    #[must_use]
    pub fn new(config: AccessConfig) -> Self {
        Self {
            config,
            client: reqwest::Client::new(),
            keys: RwLock::new(JwkSet::default()),
            fetch: tokio::sync::Mutex::new(None),
        }
    }

    pub async fn verify(&self, token: &str) -> Result<Identity, AuthError> {
        let kid = match self.verify_with_current_keys(token) {
            Err(AuthError::UnknownKey(kid)) => kid,
            result => return result,
        };
        let mut last_fetch = self.fetch.lock().await;
        // Another request may have fetched the keys while this one waited.
        match self.verify_with_current_keys(token) {
            Err(AuthError::UnknownKey(_)) => {}
            result => return result,
        }
        if last_fetch.is_some_and(|at| at.elapsed() < KEY_REFRESH_INTERVAL) {
            return Err(AuthError::UnknownKey(kid));
        }
        *last_fetch = Some(Instant::now());
        if let Err(e) = self.fetch_keys().await {
            log::warn!("failed to fetch Cloudflare Access keys: {e}");
        }
        drop(last_fetch);
        self.verify_with_current_keys(token)
    }

    fn verify_with_current_keys(&self, token: &str) -> Result<Identity, AuthError> {
        let keys = self
            .keys
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        verify_token(token, &keys, &self.config)
    }

    async fn fetch_keys(&self) -> Result<(), String> {
        let keys: JwkSet = self
            .client
            .get(self.config.certs_url())
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|e| e.to_string())?
            .json()
            .await
            .map_err(|e| e.to_string())?;
        *self
            .keys
            .write()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = keys;
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use jsonwebtoken::{EncodingKey, Header, encode};

    const SIGNER: &str = include_str!("../tests/fixtures/access-test-signer.pem");
    const IMPOSTOR: &str = include_str!("../tests/fixtures/access-test-impostor.pem");
    const JWKS: &str = include_str!("../tests/fixtures/access-test-jwks.json");
    const TEAM: &str = "https://lab.cloudflareaccess.com";
    const AUD: &str = "test-aud-tag";

    fn config() -> AccessConfig {
        AccessConfig {
            team_domain: TEAM.to_string(),
            audience: AUD.to_string(),
        }
    }

    fn keys() -> JwkSet {
        serde_json::from_str(JWKS).unwrap()
    }

    fn now() -> i64 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs()
            .try_into()
            .unwrap()
    }

    fn sign(pem: &str, kid: &str, claims: &serde_json::Value) -> String {
        let mut header = Header::new(Algorithm::RS256);
        header.kid = Some(kid.to_string());
        encode(
            &header,
            claims,
            &EncodingKey::from_rsa_pem(pem.as_bytes()).unwrap(),
        )
        .unwrap()
    }

    /// Claims shaped like Cloudflare's documented application token.
    fn claims() -> serde_json::Value {
        serde_json::json!({
            "aud": [AUD],
            "email": "operator@example.ac.jp",
            "exp": now() + 600,
            "iat": now(),
            "nbf": now(),
            "iss": TEAM,
            "type": "app",
            "identity_nonce": "nonce",
            "sub": "user-id",
            "country": "JP",
        })
    }

    fn with(mut value: serde_json::Value, key: &str, new: serde_json::Value) -> serde_json::Value {
        value[key] = new;
        value
    }

    #[test]
    fn accepts_a_valid_token_and_returns_its_email() {
        let token = sign(SIGNER, "test-key-1", &claims());

        assert_eq!(
            verify_token(&token, &keys(), &config()),
            Ok(Identity {
                email: "operator@example.ac.jp".to_string()
            })
        );
    }

    #[test]
    fn accepts_a_team_domain_with_a_trailing_slash() {
        let token = sign(SIGNER, "test-key-1", &claims());
        let config = AccessConfig {
            team_domain: format!("{TEAM}/"),
            ..config()
        };

        assert!(verify_token(&token, &keys(), &config).is_ok());
    }

    #[test]
    fn rejects_tokens_for_another_application_or_team() {
        let other_app = sign(
            SIGNER,
            "test-key-1",
            &with(claims(), "aud", serde_json::json!(["other"])),
        );
        let other_team = sign(
            SIGNER,
            "test-key-1",
            &with(
                claims(),
                "iss",
                serde_json::json!("https://evil.cloudflareaccess.com"),
            ),
        );

        assert!(matches!(
            verify_token(&other_app, &keys(), &config()),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            verify_token(&other_team, &keys(), &config()),
            Err(AuthError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_expired_tokens_and_tokens_missing_required_claims() {
        let expired = sign(
            SIGNER,
            "test-key-1",
            &with(claims(), "exp", serde_json::json!(now() - 3600)),
        );
        let mut no_aud = claims();
        no_aud.as_object_mut().unwrap().remove("aud");
        let no_aud = sign(SIGNER, "test-key-1", &no_aud);

        assert!(matches!(
            verify_token(&expired, &keys(), &config()),
            Err(AuthError::Invalid(_))
        ));
        assert!(matches!(
            verify_token(&no_aud, &keys(), &config()),
            Err(AuthError::Invalid(_))
        ));
    }

    #[test]
    fn rejects_a_token_signed_by_another_key_under_a_known_kid() {
        let forged = sign(IMPOSTOR, "test-key-1", &claims());

        assert!(matches!(
            verify_token(&forged, &keys(), &config()),
            Err(AuthError::Invalid(_))
        ));
    }

    #[test]
    fn reports_unknown_keys_so_they_can_be_refetched() {
        let token = sign(SIGNER, "rotated-key", &claims());

        assert_eq!(
            verify_token(&token, &keys(), &config()),
            Err(AuthError::UnknownKey("rotated-key".to_string()))
        );
    }

    #[test]
    fn rejects_tokens_without_an_email() {
        let mut service = claims();
        service.as_object_mut().unwrap().remove("email");
        let service = sign(SIGNER, "test-key-1", &service);

        assert_eq!(
            verify_token(&service, &keys(), &config()),
            Err(AuthError::NoEmail)
        );
        assert_eq!(
            verify_token("not.a.jwt", &keys(), &config()),
            Err(AuthError::Malformed)
        );
    }

    #[test]
    fn takes_the_token_from_the_header_before_the_cookie() {
        let mut headers = HeaderMap::new();
        headers.insert(
            "cookie",
            "theme=dark; CF_Authorization=from-cookie; x=1"
                .parse()
                .unwrap(),
        );
        assert_eq!(token_from_headers(&headers), Some("from-cookie"));

        headers.insert(TOKEN_HEADER, "from-header".parse().unwrap());
        assert_eq!(token_from_headers(&headers), Some("from-header"));

        assert_eq!(token_from_headers(&HeaderMap::new()), None);
    }
}
