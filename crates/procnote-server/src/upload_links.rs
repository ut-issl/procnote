use std::path::PathBuf;

use uuid::Uuid;

use procnote_core::event::types::ExecutionId;

/// Per-execution upload links (migration plan §7.2).
///
/// A link is a random token that lets a phone send files to one execution
/// without logging in. Each link is a file `<root>/<token>` holding the
/// execution ID, so links survive restarts and revoking one deletes it. Links
/// live in the server's state directory, never in the procedures directory,
/// because the token is a secret.
pub struct UploadLinks {
    root: PathBuf,
}

impl UploadLinks {
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// The execution a token belongs to, if the token is a live link.
    pub fn resolve(&self, token: &str) -> Option<ExecutionId> {
        let token = Uuid::parse_str(token).ok()?;
        let content = std::fs::read_to_string(self.root.join(token.to_string())).ok()?;
        Uuid::parse_str(content.trim()).ok()
    }

    /// The execution's current link token, if it has one.
    pub fn find(&self, execution_id: ExecutionId) -> Result<Option<String>, String> {
        let entries = std::fs::read_dir(&self.root).map_err(|e| e.to_string())?;
        for entry in entries {
            let entry = entry.map_err(|e| e.to_string())?;
            let Some(token) = entry.file_name().to_str().map(str::to_string) else {
                continue;
            };
            if self.resolve(&token) == Some(execution_id) {
                return Ok(Some(token));
            }
        }
        Ok(None)
    }

    /// The execution's link, created if it has none (one link per execution).
    pub fn get_or_create(&self, execution_id: ExecutionId) -> Result<String, String> {
        if let Some(token) = self.find(execution_id)? {
            return Ok(token);
        }
        let token = Uuid::new_v4().to_string();
        std::fs::write(self.root.join(&token), execution_id.to_string())
            .map_err(|e| e.to_string())?;
        Ok(token)
    }

    /// Revoke the execution's link, if any.
    pub fn revoke(&self, execution_id: ExecutionId) -> Result<(), String> {
        if let Some(token) = self.find(execution_id)? {
            std::fs::remove_file(self.root.join(token)).map_err(|e| e.to_string())?;
        }
        Ok(())
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    fn links() -> (tempfile::TempDir, UploadLinks) {
        let temp = tempfile::tempdir().unwrap();
        let links = UploadLinks::new(temp.path().join("links")).unwrap();
        (temp, links)
    }

    #[test]
    fn one_link_per_execution_that_resolves_to_it() {
        let (_temp, links) = links();
        let id = ExecutionId::new_v4();

        let token = links.get_or_create(id).unwrap();

        assert_eq!(links.get_or_create(id).unwrap(), token);
        assert_eq!(links.resolve(&token), Some(id));
        assert_eq!(links.find(id).unwrap(), Some(token));
    }

    #[test]
    fn revoked_and_unknown_tokens_do_not_resolve() {
        let (_temp, links) = links();
        let id = ExecutionId::new_v4();
        let token = links.get_or_create(id).unwrap();

        links.revoke(id).unwrap();

        assert_eq!(links.resolve(&token), None);
        assert_eq!(links.find(id).unwrap(), None);
        assert_eq!(links.resolve("not-a-token"), None);
        assert_eq!(links.resolve(&Uuid::new_v4().to_string()), None);
        assert_ne!(
            links.get_or_create(id).unwrap(),
            token,
            "a new link after revoking"
        );
    }

    #[test]
    fn tokens_cannot_reach_outside_the_link_directory() {
        let (temp, links) = links();
        std::fs::write(
            temp.path().join("secret"),
            ExecutionId::new_v4().to_string(),
        )
        .unwrap();

        assert_eq!(links.resolve("../secret"), None);
    }
}
