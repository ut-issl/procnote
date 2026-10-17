use std::path::{Path, PathBuf};

use uuid::Uuid;

use procnote_app::action::ExecutionAction;
use procnote_app::attachment::sanitize_attachment_filename;
use procnote_app::execution::infer_attachment_content_type;

/// Staging area for files uploaded by browsers before they are attached.
///
/// Each upload lives in `<root>/<token>/<filename>`. The token is handed to the
/// browser in place of a filesystem path, and an attachment action may only
/// reference files through such tokens. This mirrors the desktop shell, where
/// only paths granted by the native file picker can be attached.
pub struct UploadStore {
    root: PathBuf,
}

impl UploadStore {
    pub fn new(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(&root)?;
        Ok(Self { root })
    }

    /// Reserve a staging location for one uploaded file.
    ///
    /// Returns the token and the path the upload should be written to.
    pub fn reserve(&self, filename: &str) -> Result<(Uuid, PathBuf), String> {
        let token = Uuid::new_v4();
        let dir = self.root.join(token.to_string());
        std::fs::create_dir(&dir).map_err(|e| e.to_string())?;
        Ok((token, dir.join(sanitize_attachment_filename(filename))))
    }

    /// Replace the upload tokens in `action` with the staged files they refer to.
    ///
    /// Returns the tokens that were consumed so the caller can discard them
    /// once the action has been recorded.
    pub fn authorize(&self, action: &mut ExecutionAction) -> Result<Vec<Uuid>, String> {
        action
            .attachment_sources_mut()
            .into_iter()
            .map(|source| {
                let token = Uuid::parse_str(source.path)
                    .map_err(|_| "attachment path must be an upload token".to_string())?;
                let file = self.staged_file(token)?;
                *source.filename = file
                    .file_name()
                    .and_then(|name| name.to_str())
                    .ok_or_else(|| "staged upload has no filename".to_string())?
                    .to_string();
                *source.content_type = infer_attachment_content_type(source.filename);
                *source.path = file.to_string_lossy().to_string();
                Ok(token)
            })
            .collect()
    }

    /// Delete a staged upload.
    pub fn discard(&self, token: Uuid) {
        let dir = self.root.join(token.to_string());
        if let Err(e) = std::fs::remove_dir_all(&dir)
            && e.kind() != std::io::ErrorKind::NotFound
        {
            log::warn!("failed to remove staged upload {}: {e}", dir.display());
        }
    }

    fn staged_file(&self, token: Uuid) -> Result<PathBuf, String> {
        let dir = self.root.join(token.to_string());
        let mut files = std::fs::read_dir(&dir)
            .map_err(|_| format!("unknown upload token: {token}"))?
            .map(|entry| entry.map(|entry| entry.path()))
            .collect::<Result<Vec<_>, _>>()
            .map_err(|e| e.to_string())?;
        match (files.pop(), files.is_empty()) {
            (Some(file), true) if Path::is_file(&file) => Ok(file),
            _ => Err(format!("upload {token} is not a single staged file")),
        }
    }
}

#[cfg(test)]
#[expect(clippy::unwrap_used, reason = "unwrap is acceptable in tests")]
mod tests {
    use super::*;
    use procnote_app::action::AttachmentSource;

    fn add_attachments(paths: &[&str]) -> ExecutionAction {
        ExecutionAction::AddAttachments {
            step_id: "step-0".to_string(),
            input_id: "evidence".to_string(),
            files: paths
                .iter()
                .map(|path| AttachmentSource {
                    filename: "ignored".to_string(),
                    path: (*path).to_string(),
                    content_type: "ignored".to_string(),
                })
                .collect(),
        }
    }

    #[test]
    fn authorize_resolves_tokens_to_staged_files() {
        let temp = tempfile::tempdir().unwrap();
        let store = UploadStore::new(temp.path().to_path_buf()).unwrap();
        let (token, path) = store.reserve("photo.png").unwrap();
        std::fs::write(&path, b"png").unwrap();
        let mut action = add_attachments(&[&token.to_string()]);

        let consumed = store.authorize(&mut action).unwrap();

        assert_eq!(consumed, [token]);
        let ExecutionAction::AddAttachments { files, .. } = action else {
            unreachable!()
        };
        assert_eq!(files[0].filename, "photo.png");
        assert_eq!(files[0].content_type, "image/png");
        assert_eq!(files[0].path, path.to_string_lossy());
    }

    #[test]
    fn authorize_rejects_plain_paths() {
        let temp = tempfile::tempdir().unwrap();
        let store = UploadStore::new(temp.path().to_path_buf()).unwrap();
        let mut action = add_attachments(&["/etc/passwd"]);

        assert!(store.authorize(&mut action).is_err());
    }

    #[test]
    fn authorize_rejects_unknown_tokens() {
        let temp = tempfile::tempdir().unwrap();
        let store = UploadStore::new(temp.path().to_path_buf()).unwrap();
        let mut action = add_attachments(&[&Uuid::new_v4().to_string()]);

        assert!(store.authorize(&mut action).is_err());
    }

    #[test]
    fn discard_removes_the_staged_upload() {
        let temp = tempfile::tempdir().unwrap();
        let store = UploadStore::new(temp.path().to_path_buf()).unwrap();
        let (token, path) = store.reserve("a.txt").unwrap();
        std::fs::write(&path, b"a").unwrap();

        store.discard(token);

        assert!(!path.exists());
        let mut action = add_attachments(&[&token.to_string()]);
        assert!(store.authorize(&mut action).is_err());
    }
}
