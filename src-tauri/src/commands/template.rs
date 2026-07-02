use std::path::Path;

use serde::Serialize;
use tauri::State;

use crate::path_security::resolve_template_path;
use crate::state::AppState;
use procnote_core::template::{ProcedureTemplate, parse_template};

/// Summary of a template for listing.
#[derive(Debug, Serialize, ts_rs::TS)]
#[ts(export)]
pub struct TemplateSummary {
    pub id: String,
    pub title: String,
    pub version: String,
    pub path: String,
}

/// List all procedure templates found in the procedures directory.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn list_templates(state: State<'_, AppState>) -> Result<Vec<TemplateSummary>, String> {
    let dir = &state.procedures_dir;
    log::info!("list_templates: scanning directory {}", dir.display());
    if !dir.exists() {
        log::warn!(
            "list_templates: directory does not exist: {}",
            dir.display()
        );
        return Ok(Vec::new());
    }

    let mut summaries = Vec::new();
    let entries = std::fs::read_dir(dir).map_err(|e| e.to_string())?;

    for entry in entries {
        let entry = entry.map_err(|e| e.to_string())?;
        let subdir = entry.path();
        if !subdir.is_dir() {
            continue;
        }
        let template_path = subdir.join("template.md");
        if !template_path.exists() {
            log::debug!(
                "list_templates: skipping directory without template.md: {}",
                subdir.display()
            );
            continue;
        }
        log::info!(
            "list_templates: parsing template {}",
            template_path.display()
        );
        let source = std::fs::read_to_string(&template_path).map_err(|e| e.to_string())?;
        match parse_template(&source) {
            Ok(template) => {
                log::info!(
                    "list_templates: OK — id={}, title={}, steps={}",
                    template.metadata.id,
                    template.metadata.title,
                    template.steps.len()
                );
                summaries.push(TemplateSummary {
                    id: template.metadata.id,
                    title: template.metadata.title,
                    version: template.metadata.version,
                    path: template_path.to_string_lossy().to_string(),
                });
            }
            Err(e) => {
                log::warn!(
                    "list_templates: skipping invalid template {}: {e}",
                    template_path.display()
                );
            }
        }
    }

    log::info!("list_templates: returning {} templates", summaries.len());
    Ok(summaries)
}

/// Load and parse a specific procedure template.
#[tauri::command]
#[expect(
    clippy::needless_pass_by_value,
    reason = "Tauri command handlers require owned parameters"
)]
pub fn load_template(
    state: State<'_, AppState>,
    path: String,
) -> Result<ProcedureTemplate, String> {
    let template_path = resolve_template_path(&state.procedures_dir, Path::new(&path))?;
    let source = std::fs::read_to_string(&template_path).map_err(|e| e.to_string())?;
    parse_template(&source).map_err(|e| e.to_string())
}
