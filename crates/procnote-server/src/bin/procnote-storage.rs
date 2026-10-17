//! Move executions between a workspace directory and PostgreSQL, and export
//! shared templates as Markdown.

use std::path::PathBuf;

use clap::{ArgGroup, Parser, Subcommand};

use procnote_server::collab::{CollabConfig, CollabTemplates};
use procnote_server::postgres::PostgresStore;
use procnote_server::transfer::{
    Report, attachment_store, export_templates, export_workspace, import_workspace,
};

#[derive(Parser, Debug)]
#[command(
    name = "procnote-storage",
    version,
    about = "Import executions from a workspace into PostgreSQL, export them back, \
             or export shared templates as Markdown.",
    group(ArgGroup::new("attachment_store").required(true).args(["attachment_dir", "attachment_bucket"])),
)]
struct Args {
    #[command(subcommand)]
    command: Command,
    /// PostgreSQL URL, as for procnote-server.
    #[arg(long, env = "PROCNOTE_DATABASE_URL")]
    database_url: String,
    /// Directory for attachment content, as for procnote-server.
    #[arg(long)]
    attachment_dir: Option<PathBuf>,
    /// S3-compatible bucket for attachment content, as for procnote-server.
    #[arg(long)]
    attachment_bucket: Option<String>,
    /// Secret shared with the collab service (for `export-templates`).
    #[arg(long, env = "PROCNOTE_COLLAB_SECRET")]
    collab_secret: Option<String>,
    /// Internal URL of the collab service (for `export-templates`).
    #[arg(long, default_value = "http://127.0.0.1:5182")]
    collab_url: String,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Import every execution under WORKSPACE (read only). Executions already
    /// in the database are skipped.
    Import { workspace: PathBuf },
    /// Export every execution into WORKSPACE as `<procedure>/.executions/...`.
    /// Execution directories that already exist are skipped.
    Export { workspace: PathBuf },
    /// Write every shared template into WORKSPACE as
    /// `<procedure ID>/template.md`. Needs the collab service.
    ExportTemplates {
        workspace: PathBuf,
        /// Replace `template.md` files that already exist.
        #[arg(long)]
        overwrite: bool,
    },
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();
    let attachments = attachment_store(
        args.attachment_dir.as_deref(),
        args.attachment_bucket.as_deref(),
    )?
    .ok_or("set --attachment-dir or --attachment-bucket")?;
    let store = PostgresStore::connect(&args.database_url, attachments).await?;
    let report = match &args.command {
        Command::Import { workspace } => import_workspace(&store, workspace).await?,
        Command::Export { workspace } => export_workspace(&store, workspace).await?,
        Command::ExportTemplates {
            workspace,
            overwrite,
        } => {
            let secret = args
                .collab_secret
                .clone()
                .ok_or("set --collab-secret (or PROCNOTE_COLLAB_SECRET)")?;
            let collab = CollabTemplates::new(
                store.pool(),
                CollabConfig {
                    secret,
                    service_url: args.collab_url.clone(),
                },
            );
            export_templates(&collab, workspace, *overwrite).await?
        }
    };
    print_report(&report);
    if report.failed.is_empty() {
        Ok(())
    } else {
        Err(format!("{} failed", report.failed.len()).into())
    }
}

fn print_report(report: &Report) {
    let show = |label: &str, dirs: &[PathBuf]| {
        for dir in dirs {
            log::info!("{label}: {}", dir.display());
        }
    };
    show("transferred", &report.transferred);
    show("skipped (already present)", &report.skipped);
    for (dir, error) in &report.failed {
        log::error!("failed: {dir}: {error}");
    }
    log::info!(
        "{} transferred, {} skipped, {} failed",
        report.transferred.len(),
        report.skipped.len(),
        report.failed.len()
    );
}

#[cfg(test)]
mod tests {
    use clap::CommandFactory;

    #[test]
    fn the_command_line_is_well_formed() {
        super::Args::command().debug_assert();
    }
}
