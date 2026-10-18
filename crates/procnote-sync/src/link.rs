//! Which server a workspace syncs with, and as whom.
//!
//! Links are kept per workspace in one file outside the workspaces (the
//! desktop app's config directory), since a workspace may be in git and the
//! token is a secret.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// A workspace's link to a server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerLink {
    /// The server's base URL, e.g. `https://procnote.example.ac.jp`.
    pub server: String,
    /// The device token (`pnd_…`) its owner issued for this app.
    pub token: String,
    /// The token's owner, recorded as the author of what this app syncs.
    pub email: String,
    /// This app's device name, as shown on the server.
    pub device: String,
}

/// The links of every workspace, by canonical workspace path.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Links {
    #[serde(default)]
    workspaces: BTreeMap<PathBuf, ServerLink>,
}

impl Links {
    /// Read the links file; a missing file means no links.
    pub fn load(path: &Path) -> Result<Self, String> {
        match std::fs::read(path) {
            Ok(json) => serde_json::from_slice(&json)
                .map_err(|e| format!("invalid {}: {e}", path.display())),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(e.to_string()),
        }
    }

    /// Write the links file, readable only by this user where that's possible.
    pub fn save(&self, path: &Path) -> Result<(), String> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        let json = serde_json::to_vec_pretty(self).map_err(|e| e.to_string())?;
        let temp = path.with_extension("tmp");
        std::fs::write(&temp, json).map_err(|e| e.to_string())?;
        restrict_to_owner(&temp)?;
        std::fs::rename(&temp, path).map_err(|e| e.to_string())
    }

    #[must_use]
    pub fn get(&self, workspace: &Path) -> Option<&ServerLink> {
        self.workspaces.get(workspace)
    }

    pub fn set(&mut self, workspace: &Path, link: ServerLink) {
        self.workspaces.insert(workspace.to_path_buf(), link);
    }

    /// Remove the workspace's link; returns it if there was one.
    pub fn remove(&mut self, workspace: &Path) -> Option<ServerLink> {
        self.workspaces.remove(workspace)
    }
}

#[cfg(unix)]
fn restrict_to_owner(path: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn restrict_to_owner(_path: &Path) -> Result<(), String> {
    // On Windows the user's profile directory is already private.
    Ok(())
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;

    fn link(server: &str) -> ServerLink {
        ServerLink {
            server: server.to_string(),
            token: "pnd_x".to_string(),
            email: "me@example.ac.jp".to_string(),
            device: "bench PC".to_string(),
        }
    }

    #[test]
    fn links_are_kept_per_workspace_and_survive_a_reload() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config/servers.json");
        let mut links = Links::load(&file).unwrap();
        links.set(Path::new("/lab/a"), link("https://a"));
        links.set(Path::new("/lab/b"), link("https://b"));
        links.save(&file).unwrap();

        let mut reloaded = Links::load(&file).unwrap();
        assert_eq!(reloaded.get(Path::new("/lab/a")), Some(&link("https://a")));
        assert_eq!(
            reloaded.remove(Path::new("/lab/b")),
            Some(link("https://b"))
        );
        assert_eq!(reloaded.get(Path::new("/lab/b")), None);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
    }
}
