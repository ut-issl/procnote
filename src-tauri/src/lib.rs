mod action;
mod commands;
mod drop_point;
mod path_security;
mod persistence;
mod state;

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use clap::Parser;
use tauri::Manager;

use commands::drop_point::{
    cancel_attachment_drop_point_session, import_attachment_drop_point_upload,
    is_drop_point_configured, poll_attachment_drop_point_session,
    start_attachment_drop_point_session,
};
use commands::execution::{
    get_attachment_preview_data_url, get_execution_state, list_executions, pick_attachment_sources,
    record_action, reveal_execution_dir, start_execution,
};
use commands::template::list_templates;
use drop_point::{DropPointClient, DropPointConfig, DropPointSessions, cleanup_persisted_sessions};
use state::AppState;

/// Arguments accepted by the desktop executable.
#[derive(Parser, Debug)]
#[command(
    name = "procnote",
    version,
    about = "procnote - Procedure execution tool for hardware testing."
)]
struct Args {
    /// Workspace directory containing procedure subdirectories.
    /// Defaults to the current working directory.
    #[arg(default_value = ".")]
    workspace: PathBuf,
}

/// Parses the optional workspace argument and starts the desktop application.
#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let args = Args::parse();
    run_with_workspace(&args.workspace);
}

fn run_with_workspace(workspace: &Path) {
    // Canonicalize early so relative paths like "." resolve correctly.
    let procedures_dir = workspace.canonicalize().unwrap_or_else(|_| {
        panic!(
            "workspace directory does not exist: {}",
            workspace.display()
        )
    });

    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .setup(move |app| {
            if cfg!(debug_assertions) {
                app.handle().plugin(
                    tauri_plugin_log::Builder::default()
                        .level(log::LevelFilter::Debug)
                        .target(tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::Stdout,
                        ))
                        .target(tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::LogDir { file_name: None },
                        ))
                        .target(tauri_plugin_log::Target::new(
                            tauri_plugin_log::TargetKind::Webview,
                        ))
                        .build(),
                )?;
            }

            let drop_point_config = match DropPointConfig::from_env() {
                Ok(config) => config,
                Err(e) => {
                    log::warn!("DropPoint disabled: {e}");
                    None
                }
            };

            let drop_point_client = drop_point_config.clone().map(DropPointClient::new);
            if let Some(client) = drop_point_client.clone() {
                let cleanup_dir = procedures_dir.clone();
                tauri::async_runtime::spawn(async move {
                    cleanup_persisted_sessions(&cleanup_dir, &client).await;
                });
            }

            app.manage(AppState {
                procedures_dir,
                drop_point_config,
                drop_point_client,
                attachment_grants: Mutex::new(HashSet::new()),
            });
            app.manage(DropPointSessions::default());

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_templates,
            start_execution,
            record_action,
            get_execution_state,
            list_executions,
            get_attachment_preview_data_url,
            pick_attachment_sources,
            reveal_execution_dir,
            is_drop_point_configured,
            start_attachment_drop_point_session,
            poll_attachment_drop_point_session,
            import_attachment_drop_point_upload,
            cancel_attachment_drop_point_session,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
