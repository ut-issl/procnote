mod action;
mod commands;
mod persistence;
mod state;

use std::path::{Path, PathBuf};

use clap::Parser;
use tauri::Manager;

use commands::execution::{get_execution_state, list_executions, record_action, start_execution};
use commands::template::{list_templates, load_template};
use state::AppState;

/// Command-line arguments shared by both binary crates.
#[derive(Parser, Debug)]
#[command(
    version,
    about = "procnote - Procedure execution tool for hardware testing."
)]
pub struct Args {
    /// Workspace directory containing procedure subdirectories.
    /// Defaults to the current working directory.
    #[arg(default_value = ".")]
    pub workspace: PathBuf,
}

/// Entry point used by both `procnote-cli` and `procnote-tauri` binaries.
/// Parses CLI arguments and hands off to [`run`].
pub fn run_cli() {
    let args = Args::parse();
    run(&args.workspace);
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run(workspace: &Path) {
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

            app.manage(AppState { procedures_dir });

            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            list_templates,
            load_template,
            start_execution,
            record_action,
            get_execution_state,
            list_executions,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
