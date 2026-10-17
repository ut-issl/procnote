use std::net::SocketAddr;
use std::path::PathBuf;

use clap::Parser;

use procnote_server::{ServerConfig, router};

/// Arguments accepted by the API server.
#[derive(Parser, Debug)]
#[command(
    name = "procnote-server",
    version,
    about = "procnote API server for using the procnote UI from a browser."
)]
struct Args {
    /// Workspace directory containing procedure subdirectories.
    #[arg(default_value = ".")]
    workspace: PathBuf,
    /// Address to listen on. Use 0.0.0.0 to accept connections from other devices.
    #[arg(long, default_value = "127.0.0.1:5181")]
    listen: SocketAddr,
    /// Built frontend directory to serve (e.g. `build/`).
    #[arg(long)]
    static_dir: Option<PathBuf>,
    /// Directory for server state such as upload links. Defaults to
    /// `$XDG_STATE_HOME/procnote-server` or `~/.local/state/procnote-server`.
    #[arg(long)]
    state_dir: Option<PathBuf>,
}

fn default_state_dir() -> PathBuf {
    let base = std::env::var_os("XDG_STATE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".local/state")))
        .unwrap_or_else(std::env::temp_dir);
    base.join("procnote-server")
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();
    let args = Args::parse();

    let procedures_dir = args.workspace.canonicalize().map_err(|e| {
        format!(
            "workspace directory does not exist: {}: {e}",
            args.workspace.display()
        )
    })?;
    let upload_dir =
        std::env::temp_dir().join(format!("procnote-uploads-{}", uuid::Uuid::new_v4()));
    let app = router(ServerConfig {
        procedures_dir: procedures_dir.clone(),
        upload_dir,
        static_dir: args.static_dir,
        state_dir: args.state_dir.unwrap_or_else(default_state_dir),
    })?;

    let listener = tokio::net::TcpListener::bind(args.listen).await?;
    log::info!(
        "serving {} on http://{}",
        procedures_dir.display(),
        listener.local_addr()?
    );
    axum::serve(listener, app).await?;
    Ok(())
}
