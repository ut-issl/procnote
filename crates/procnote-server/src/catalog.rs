//! The template catalog: one entry per procedure, joining the workspace's
//! template files with the shared (collaboratively edited) templates by
//! procedure ID.

use std::collections::HashSet;

use procnote_app::template::TemplateSummary;
use serde::Serialize;
use uuid::Uuid;

use crate::collab::CollabTemplateSummary;

/// A procedure's template, from the workspace, the server, or both.
#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct CatalogEntry {
    pub procedure_id: String,
    pub title: String,
    pub version: String,
    /// Path of the workspace's `template.md`, if there is one.
    pub file: Option<String>,
    /// The shared template, if there is one.
    pub shared: Option<SharedTemplate>,
    /// Whether the shared template no longer describes what the workspace
    /// file does. Only set by [`entries`]' caller, which reads both.
    pub differs_from_file: bool,
}

#[derive(Debug, Serialize, PartialEq, Eq)]
pub struct SharedTemplate {
    pub id: Uuid,
    /// ISO 8601 timestamp of the last stored change.
    pub updated_at: String,
}

/// The catalog of `files` and `shared` templates, by title.
///
/// A procedure's shared template is what it shows. Files of procedures whose
/// shared template was archived are left out, and so are further files of a
/// procedure ID already listed.
#[must_use]
pub fn entries(
    files: Vec<TemplateSummary>,
    shared: Vec<CollabTemplateSummary>,
    archived: &[String],
) -> Vec<CatalogEntry> {
    let mut entries: Vec<CatalogEntry> = shared
        .into_iter()
        .map(|template| CatalogEntry {
            procedure_id: template.procedure_id,
            title: template.title,
            version: template.version,
            file: None,
            shared: Some(SharedTemplate {
                id: template.id,
                updated_at: template.updated_at,
            }),
            differs_from_file: false,
        })
        .collect();
    let archived: HashSet<&str> = archived.iter().map(String::as_str).collect();
    for file in files {
        if archived.contains(file.id.as_str()) {
            continue;
        }
        match entries
            .iter_mut()
            .find(|entry| entry.procedure_id == file.id)
        {
            Some(entry) if entry.file.is_none() => {
                if entry.title.is_empty() {
                    entry.title = file.title;
                }
                entry.file = Some(file.path);
            }
            Some(_) => log::warn!(
                "template {} has the procedure ID {} of another template; not listed",
                file.path,
                file.id
            ),
            None => entries.push(CatalogEntry {
                procedure_id: file.id,
                title: file.title,
                version: file.version,
                file: Some(file.path),
                shared: None,
                differs_from_file: false,
            }),
        }
    }
    entries.sort_by(|a, b| {
        (a.title.as_str(), a.procedure_id.as_str())
            .cmp(&(b.title.as_str(), b.procedure_id.as_str()))
    });
    entries
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(id: &str, title: &str) -> TemplateSummary {
        TemplateSummary {
            id: id.to_string(),
            title: title.to_string(),
            version: "1".to_string(),
            path: format!("/procedures/{id}/template.md"),
        }
    }

    fn shared(id: Uuid, procedure_id: &str, title: &str) -> CollabTemplateSummary {
        CollabTemplateSummary {
            id,
            title: title.to_string(),
            procedure_id: procedure_id.to_string(),
            version: "2".to_string(),
            updated_at: "2026-10-22T00:00:00+00:00".to_string(),
        }
    }

    #[test]
    fn files_and_shared_templates_of_a_procedure_are_one_entry() {
        let id = Uuid::new_v4();
        let listed = entries(
            vec![file("TVT", "Thermal"), file("INSP", "Inspection")],
            vec![shared(id, "TVT", "Thermal (edited)")],
            &[],
        );

        assert_eq!(
            listed,
            vec![
                CatalogEntry {
                    procedure_id: "INSP".to_string(),
                    title: "Inspection".to_string(),
                    version: "1".to_string(),
                    file: Some("/procedures/INSP/template.md".to_string()),
                    shared: None,
                    differs_from_file: false,
                },
                CatalogEntry {
                    procedure_id: "TVT".to_string(),
                    title: "Thermal (edited)".to_string(),
                    version: "2".to_string(),
                    file: Some("/procedures/TVT/template.md".to_string()),
                    shared: Some(SharedTemplate {
                        id,
                        updated_at: "2026-10-22T00:00:00+00:00".to_string(),
                    }),
                    differs_from_file: false,
                },
            ]
        );
    }

    #[test]
    fn files_of_archived_procedures_and_repeated_ids_are_left_out() {
        let listed = entries(
            vec![file("OLD", "Old"), file("A", "First"), file("A", "Second")],
            vec![],
            &["OLD".to_string()],
        );

        assert_eq!(
            listed
                .iter()
                .map(|entry| entry.title.as_str())
                .collect::<Vec<_>>(),
            ["First"]
        );
    }

    #[test]
    fn shared_templates_without_a_file_are_listed() {
        let id = Uuid::new_v4();
        let listed = entries(vec![], vec![shared(id, "NEW", "")], &[]);

        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0].file, None);
        assert_eq!(listed[0].shared.as_ref().map(|s| s.id), Some(id));
    }
}
