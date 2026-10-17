//! The API side of collaborative template editing.
//!
//! Templates are Yjs documents edited through the collab service
//! (Hocuspocus, `collab/`), which stores them in the `templates` table. This
//! server creates templates, lists them, and issues the short-lived tokens
//! the collab service accepts, so Cloudflare Access is checked in one place.

use std::time::{Duration, SystemTime, UNIX_EPOCH};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use hmac::{Hmac, KeyInit, Mac};
use procnote_core::template::types::ProcedureTemplate;
use serde::{Deserialize, Serialize};
use sha2::Sha256;
use sqlx::postgres::PgPool;
use uuid::Uuid;

/// How long a collab token is accepted. The editor fetches a new one
/// whenever it (re)connects.
pub const TOKEN_LIFETIME: Duration = Duration::from_mins(5);

/// Header carrying the shared secret on the collab service's internal API.
pub const INTERNAL_AUTH_HEADER: &str = "authorization";

pub struct CollabConfig {
    /// Secret shared with the collab service, for tokens and its internal API.
    pub secret: String,
    /// Base URL of the collab service's internal API, e.g. `http://127.0.0.1:5182`.
    pub service_url: String,
}

/// What a collab token grants: editing `doc` as `sub` until `exp`.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
struct Claims {
    doc: String,
    sub: String,
    /// Expiry, in seconds since the Unix epoch.
    exp: u64,
}

/// A token for editing `document` as `subject` until `expires_at` (seconds
/// since the Unix epoch): `base64url(claims JSON).base64url(HMAC-SHA256)`.
#[must_use]
pub fn issue_token(secret: &[u8], document: &str, subject: &str, expires_at: u64) -> String {
    let claims = Claims {
        doc: document.to_string(),
        sub: subject.to_string(),
        exp: expires_at,
    };
    let payload = URL_SAFE_NO_PAD
        .encode(serde_json::to_vec(&claims).expect("claims always serialize to JSON"));
    let mut mac = Hmac::<Sha256>::new_from_slice(secret).expect("HMAC takes keys of any size");
    mac.update(payload.as_bytes());
    let signature = URL_SAFE_NO_PAD.encode(mac.finalize().into_bytes());
    format!("{payload}.{signature}")
}

/// A collaboratively edited template, for listing.
#[derive(Debug, Serialize)]
pub struct CollabTemplateSummary {
    pub id: Uuid,
    pub title: String,
    pub procedure_id: String,
    pub version: String,
    /// ISO 8601 timestamp of the last stored change.
    pub updated_at: String,
}

/// The fields of the document's metadata map used for listing.
#[derive(Debug, Default, Deserialize)]
struct ListedMetadata {
    #[serde(default)]
    id: String,
    #[serde(default)]
    title: String,
    #[serde(default)]
    version: String,
}

pub struct CollabTemplates {
    pool: PgPool,
    config: CollabConfig,
    client: reqwest::Client,
}

impl CollabTemplates {
    #[must_use]
    pub fn new(pool: PgPool, config: CollabConfig) -> Self {
        Self {
            pool,
            config,
            client: reqwest::Client::new(),
        }
    }

    pub async fn list(&self) -> Result<Vec<CollabTemplateSummary>, String> {
        let rows: Vec<(
            Uuid,
            Option<serde_json::Value>,
            chrono::DateTime<chrono::Utc>,
        )> = sqlx::query_as(
            "SELECT id, metadata, updated_at FROM templates ORDER BY updated_at DESC",
        )
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("database error: {e}"))?;
        Ok(rows
            .into_iter()
            .map(|(id, metadata, updated_at)| {
                let metadata: ListedMetadata = metadata
                    .and_then(|value| serde_json::from_value(value).ok())
                    .unwrap_or_default();
                CollabTemplateSummary {
                    id,
                    title: metadata.title,
                    procedure_id: metadata.id,
                    version: metadata.version,
                    updated_at: updated_at.to_rfc3339(),
                }
            })
            .collect())
    }

    /// Create a template whose document starts as `template`.
    pub async fn create(&self, template: &ProcedureTemplate) -> Result<Uuid, String> {
        let id = Uuid::new_v4();
        sqlx::query("INSERT INTO templates (id) VALUES ($1)")
            .bind(id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("database error: {e}"))?;
        if let Err(e) = self.seed(id, template).await {
            // Don't leave a template the collab service never filled.
            if let Err(cleanup) = sqlx::query("DELETE FROM templates WHERE id = $1")
                .bind(id)
                .execute(&self.pool)
                .await
            {
                log::warn!("failed to remove unseeded template {id}: {cleanup}");
            }
            return Err(e);
        }
        Ok(id)
    }

    async fn seed(&self, id: Uuid, template: &ProcedureTemplate) -> Result<(), String> {
        let url = format!(
            "{}/internal/templates/{id}",
            self.config.service_url.trim_end_matches('/')
        );
        let response = self
            .client
            .post(url)
            .header(
                INTERNAL_AUTH_HEADER,
                format!("Bearer {}", self.config.secret),
            )
            .json(template)
            .send()
            .await
            .map_err(|e| format!("collab service is unreachable: {e}"))?;
        if response.status().is_success() {
            Ok(())
        } else {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            Err(format!(
                "collab service refused the template ({status}): {body}"
            ))
        }
    }

    /// A token for editing template `id` as `subject`, if the template exists.
    pub async fn token(&self, id: Uuid, subject: &str) -> Result<Option<String>, String> {
        let exists: Option<i32> = sqlx::query_scalar("SELECT 1 FROM templates WHERE id = $1")
            .bind(id)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("database error: {e}"))?;
        let expires_at = (SystemTime::now() + TOKEN_LIFETIME)
            .duration_since(UNIX_EPOCH)
            .map_err(|e| e.to_string())?
            .as_secs();
        Ok(exists.map(|_| {
            issue_token(
                self.config.secret.as_bytes(),
                &id.to_string(),
                subject,
                expires_at,
            )
        }))
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    /// Shared with `collab/token.test.ts`, which must accept this token.
    const KNOWN_TOKEN: &str = "eyJkb2MiOiJkb2MtMSIsInN1YiI6Im9wZXJhdG9yQGV4YW1wbGUuYWMuanAiLCJleHAiOjQxMDI0NDQ4MDB9.6_FwxfiXy8ujUSIeVjceHXnMRdduoe4fENSCDCEtyyg";

    #[test]
    fn a_token_carries_its_claims_and_a_signature_over_them() {
        let token = issue_token(
            b"test-secret",
            "doc-1",
            "operator@example.ac.jp",
            4_102_444_800,
        );

        let (payload, signature) = token.split_once('.').unwrap();
        let claims: Claims =
            serde_json::from_slice(&URL_SAFE_NO_PAD.decode(payload).unwrap()).unwrap();
        assert_eq!(
            claims,
            Claims {
                doc: "doc-1".to_string(),
                sub: "operator@example.ac.jp".to_string(),
                exp: 4_102_444_800,
            }
        );
        let mut mac = Hmac::<Sha256>::new_from_slice(b"test-secret").unwrap();
        mac.update(payload.as_bytes());
        mac.verify_slice(&URL_SAFE_NO_PAD.decode(signature).unwrap())
            .unwrap();
        assert_eq!(token, KNOWN_TOKEN);
    }

    #[test]
    fn tokens_differ_by_secret() {
        assert_ne!(
            issue_token(b"a", "doc", "x", 1),
            issue_token(b"b", "doc", "x", 1)
        );
    }
}
