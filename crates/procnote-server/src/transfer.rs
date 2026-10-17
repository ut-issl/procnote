//! Moving executions between a workspace directory and [`PostgresStore`].
//!
//! Import reads `.executions/` without modifying it; export writes the same
//! directory layout, so the desktop app can open exported executions.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;
use procnote_app::persistence::event_log::EventLog;
use procnote_app::persistence::execution_store::{
    execution_dir_name, execution_dirs, write_execution_snapshot_durable,
};
use procnote_app::repository::{StagedFile, TemplateSnapshot};
use procnote_core::event::types::ExecutionId;
use procnote_core::execution::ExecutionState;

use crate::postgres::{PostgresStore, TransferredExecution, referenced_files};

/// The attachment store selected by `--attachment-dir` or
/// `--attachment-bucket` (an S3-compatible bucket configured by `AWS_*`
/// variables).
pub fn attachment_store(
    dir: Option<&Path>,
    bucket: Option<&str>,
) -> Result<Option<Arc<dyn ObjectStore>>, String> {
    match (dir, bucket) {
        (Some(dir), _) => {
            std::fs::create_dir_all(dir).map_err(|e| {
                format!(
                    "failed to create attachment directory {}: {e}",
                    dir.display()
                )
            })?;
            let store = LocalFileSystem::new_with_prefix(dir)
                .map_err(|e| e.to_string())?
                .with_fsync(true);
            Ok(Some(Arc::new(store)))
        }
        (None, Some(bucket)) => {
            let store = AmazonS3Builder::from_env()
                .with_bucket_name(bucket)
                .build()
                .map_err(|e| format!("invalid attachment bucket configuration: {e}"))?;
            Ok(Some(Arc::new(store)))
        }
        (None, None) => Ok(None),
    }
}

/// What happened to each execution.
#[derive(Debug, Default)]
pub struct Report {
    pub transferred: Vec<PathBuf>,
    /// Already present at the destination; left as is.
    pub skipped: Vec<PathBuf>,
    /// The execution (its directory, or its ID when exporting) and the error.
    pub failed: Vec<(String, String)>,
}

/// Import every execution under `procedures_dir`.
pub async fn import_workspace(
    store: &PostgresStore,
    procedures_dir: &Path,
) -> Result<Report, String> {
    let mut report = Report::default();
    for dir in execution_dirs(procedures_dir)? {
        match import_execution(store, &dir).await {
            Ok(true) => report.transferred.push(dir),
            Ok(false) => report.skipped.push(dir),
            Err(e) => report.failed.push((dir.display().to_string(), e)),
        }
    }
    Ok(report)
}

async fn import_execution(store: &PostgresStore, dir: &Path) -> Result<bool, String> {
    let records = EventLog::new(dir.join("events.jsonl"))
        .read_records()
        .map_err(|e| e.to_string())?;
    let procedure = dir
        .parent()
        .and_then(Path::parent)
        .and_then(Path::file_name)
        .and_then(|name| name.to_str())
        .ok_or("execution directory is not inside a procedure directory")?
        .to_string();
    let template = read_snapshot(dir)?;
    let files = referenced_files(&records)
        .into_iter()
        .map(|record| StagedFile {
            source: dir.join(&record.path),
            record,
        })
        .collect();
    store
        .import(TransferredExecution {
            procedure,
            template,
            records,
            files,
        })
        .await
}

/// The template snapshot of an execution directory: `template.md`, or
/// `template.json` for executions of shared templates.
fn read_snapshot(dir: &Path) -> Result<TemplateSnapshot, String> {
    let read = |name: &str| std::fs::read_to_string(dir.join(name)).map_err(|e| e.to_string());
    if dir.join(TemplateSnapshot::JSON_FILE).exists() {
        read(TemplateSnapshot::JSON_FILE).map(TemplateSnapshot::Json)
    } else {
        read(TemplateSnapshot::MARKDOWN_FILE).map(TemplateSnapshot::Markdown)
    }
}

/// Export every stored execution into `procedures_dir`, as
/// `<procedure>/.executions/<started>-<id>/`.
pub async fn export_workspace(
    store: &PostgresStore,
    procedures_dir: &Path,
) -> Result<Report, String> {
    let mut report = Report::default();
    for id in store.execution_ids().await? {
        match export_execution(store, id, procedures_dir).await {
            Ok(Exported::Written(dir)) => report.transferred.push(dir),
            Ok(Exported::AlreadyPresent(dir)) => report.skipped.push(dir),
            Err(e) => report.failed.push((id.to_string(), e)),
        }
    }
    Ok(report)
}

enum Exported {
    Written(PathBuf),
    AlreadyPresent(PathBuf),
}

async fn export_execution(
    store: &PostgresStore,
    id: ExecutionId,
    procedures_dir: &Path,
) -> Result<Exported, String> {
    let execution = store.export(id).await?;
    let state = replay_records(&execution)?;
    let started_at = state
        .started_at
        .ok_or_else(|| format!("execution {id} has no start time"))?;
    let dir = procedures_dir
        .join(&execution.procedure)
        .join(".executions")
        .join(execution_dir_name(&started_at, id));
    if dir.exists() {
        return Ok(Exported::AlreadyPresent(dir));
    }
    write_execution(&dir, &execution, &state).map_err(|e| format!("{}: {e}", dir.display()))?;
    Ok(Exported::Written(dir))
}

fn replay_records<F>(execution: &TransferredExecution<F>) -> Result<ExecutionState, String> {
    let events: Vec<_> = execution
        .records
        .iter()
        .map(|record| record.event.clone())
        .collect();
    ExecutionState::from_events(&events).map_err(|e| e.to_string())
}

fn write_execution(
    dir: &Path,
    execution: &TransferredExecution<(String, Vec<u8>)>,
    state: &ExecutionState,
) -> Result<(), String> {
    let io = |e: std::io::Error| e.to_string();
    std::fs::create_dir_all(dir).map_err(io)?;
    std::fs::write(
        dir.join(execution.template.file_name()),
        execution.template.text(),
    )
    .map_err(io)?;
    for (path, content) in &execution.files {
        let destination = safe_join(dir, path)?;
        if let Some(parent) = destination.parent() {
            std::fs::create_dir_all(parent).map_err(io)?;
        }
        std::fs::write(destination, content).map_err(io)?;
    }
    EventLog::new(dir.join("events.jsonl"))
        .create_with_events_durable(&execution.records)
        .map_err(|e| e.to_string())?;
    write_execution_snapshot_durable(dir, state)
}

/// `dir/relative`, where `relative` must stay inside `dir`.
fn safe_join(dir: &Path, relative: &str) -> Result<PathBuf, String> {
    let path = Path::new(relative);
    if path
        .components()
        .all(|component| matches!(component, std::path::Component::Normal(_)))
    {
        Ok(dir.join(path))
    } else {
        Err(format!("invalid attachment path: {relative}"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attachment_paths_must_stay_inside_the_execution() {
        let dir = Path::new("/exec");
        assert_eq!(
            safe_join(dir, "attachments/a.txt"),
            Ok(PathBuf::from("/exec/attachments/a.txt"))
        );
        assert!(safe_join(dir, "../a.txt").is_err());
        assert!(safe_join(dir, "/etc/passwd").is_err());
        assert!(safe_join(dir, "attachments/./a.txt").is_ok_and(|p| p.ends_with("a.txt")));
    }
}
