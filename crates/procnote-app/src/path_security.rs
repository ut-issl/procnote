use std::path::{Path, PathBuf};

/// Resolve a frontend-supplied template path to a canonical `template.md` file
/// directly inside one procedure directory under `procedures_dir`.
pub fn resolve_template_path(procedures_dir: &Path, requested: &Path) -> Result<PathBuf, String> {
    let procedures_root = procedures_dir
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize procedures directory: {e}"))?;
    let template_path = requested
        .canonicalize()
        .map_err(|e| format!("failed to canonicalize template path: {e}"))?;

    if !template_path.starts_with(&procedures_root) {
        return Err("template path is outside the procedures directory".to_string());
    }
    if template_path.file_name().and_then(|name| name.to_str()) != Some("template.md") {
        return Err("template path must point to a template.md file".to_string());
    }
    let Some(template_dir) = template_path.parent() else {
        return Err("template path has no parent directory".to_string());
    };
    if template_dir.parent() != Some(procedures_root.as_path()) {
        return Err("template.md must be directly inside a procedure directory".to_string());
    }

    Ok(template_path)
}
