//! Tokens that let a desktop app sync with the server on someone's behalf.
//!
//! Someone signed in through Cloudflare Access issues a token for their
//! desktop app (`/device/connect`); the app then calls `/api/device/*` with
//! it, which is an Access Bypass. Like upload links, tokens are files in the
//! server's state directory, never in the procedures directory. Only the
//! token's SHA-256 is stored, so the files don't reveal usable tokens.

use std::path::PathBuf;

use chrono::{DateTime, Utc};
use procnote_app::attachment::sha256_hex;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Prefix of every device token, so they are recognizable (e.g. by secret scanners).
const TOKEN_PREFIX: &str = "pnd_";

/// What a token stands for. The token itself is never stored.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeviceToken {
    /// Names the token for listing and revoking (not the secret).
    pub id: Uuid,
    /// Who issued it; what the device records is attributed to them.
    pub email: String,
    /// The device's name, as the desktop app reported it.
    pub device: String,
    pub created_at: DateTime<Utc>,
}

pub struct DeviceTokens {
    root: PathBuf,
}

impl DeviceTokens {
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    fn path_of(&self, token: &str) -> PathBuf {
        self.root.join(sha256_hex(token.as_bytes()))
    }

    /// Issue a token for `email`'s `device`. Returns the secret token, which
    /// is shown once and never stored, and what it stands for.
    pub fn issue(
        &self,
        email: &str,
        device: &str,
        now: DateTime<Utc>,
    ) -> Result<(String, DeviceToken), String> {
        // Two v4 UUIDs: 244 random bits.
        let token = format!(
            "{TOKEN_PREFIX}{}{}",
            Uuid::new_v4().simple(),
            Uuid::new_v4().simple()
        );
        let info = DeviceToken {
            id: Uuid::new_v4(),
            email: email.to_string(),
            device: device.trim().to_string(),
            created_at: now,
        };
        let json = serde_json::to_vec(&info).map_err(|e| e.to_string())?;
        std::fs::write(self.path_of(&token), json).map_err(|e| e.to_string())?;
        Ok((token, info))
    }

    /// What a live token stands for, if `token` is one.
    #[must_use]
    pub fn resolve(&self, token: &str) -> Option<DeviceToken> {
        if !token.starts_with(TOKEN_PREFIX) {
            return None;
        }
        let json = std::fs::read(self.path_of(token)).ok()?;
        serde_json::from_slice(&json).ok()
    }

    fn all(&self) -> Result<Vec<(PathBuf, DeviceToken)>, String> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| e.to_string())?;
        Ok(entries
            .flatten()
            .filter_map(|entry| {
                let json = std::fs::read(entry.path()).ok()?;
                Some((entry.path(), serde_json::from_slice(&json).ok()?))
            })
            .collect())
    }

    /// `email`'s tokens, newest first.
    pub fn list(&self, email: &str) -> Result<Vec<DeviceToken>, String> {
        let mut tokens: Vec<DeviceToken> = self
            .all()?
            .into_iter()
            .map(|(_, info)| info)
            .filter(|info| info.email == email)
            .collect();
        tokens.sort_by_key(|info| std::cmp::Reverse(info.created_at));
        Ok(tokens)
    }

    /// Revoke `email`'s token `id`. Returns whether there was one.
    pub fn revoke(&self, email: &str, id: Uuid) -> Result<bool, String> {
        for (path, info) in self.all()? {
            if info.id == id && info.email == email {
                std::fs::remove_file(path).map_err(|e| e.to_string())?;
                return Ok(true);
            }
        }
        Ok(false)
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    fn tokens() -> (tempfile::TempDir, DeviceTokens) {
        let temp = tempfile::tempdir().unwrap();
        let tokens = DeviceTokens::new(temp.path().join("device-tokens")).unwrap();
        (temp, tokens)
    }

    #[test]
    fn an_issued_token_resolves_to_who_issued_it() {
        let (temp, tokens) = tokens();

        let (token, info) = tokens
            .issue("operator@example.ac.jp", " bench PC ", Utc::now())
            .unwrap();

        assert!(token.starts_with("pnd_"));
        assert_eq!(tokens.resolve(&token), Some(info.clone()));
        assert_eq!(info.device, "bench PC");
        // Only the hash is on disk.
        let stored = std::fs::read_dir(temp.path().join("device-tokens"))
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect::<Vec<_>>();
        assert_eq!(stored, [sha256_hex(token.as_bytes())]);
        assert!(
            !std::fs::read_to_string(temp.path().join("device-tokens").join(&stored[0]))
                .unwrap()
                .contains(&token)
        );
    }

    #[test]
    fn people_list_and_revoke_only_their_own_tokens() {
        let (_temp, tokens) = tokens();
        let (mine, my_info) = tokens.issue("me@example.ac.jp", "a", Utc::now()).unwrap();
        let (theirs, their_info) = tokens.issue("you@example.ac.jp", "b", Utc::now()).unwrap();

        assert_eq!(
            tokens.list("me@example.ac.jp").unwrap(),
            std::slice::from_ref(&my_info)
        );
        assert!(!tokens.revoke("me@example.ac.jp", their_info.id).unwrap());
        assert!(tokens.resolve(&theirs).is_some());
        assert!(tokens.revoke("me@example.ac.jp", my_info.id).unwrap());
        assert!(tokens.resolve(&mine).is_none());
    }

    #[test]
    fn unknown_and_malformed_tokens_resolve_to_nothing() {
        let (_temp, tokens) = tokens();

        assert!(tokens.resolve("pnd_unknown").is_none());
        assert!(tokens.resolve("../../etc/passwd").is_none());
        assert!(tokens.resolve("").is_none());
    }
}
