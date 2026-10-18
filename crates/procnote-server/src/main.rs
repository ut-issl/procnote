use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use clap::{ArgGroup, Parser};
use object_store::ObjectStore;
use object_store::aws::AmazonS3Builder;
use object_store::local::LocalFileSystem;

use procnote_app::persistence::execution_store::ExecutionStore;
use procnote_app::repository::ExecutionRepository;
use procnote_server::access::AccessConfig;
use procnote_server::postgres::PostgresStore;
use procnote_server::{Auth, ServerConfig, router};

/// Arguments accepted by the API server.
#[derive(Parser, Debug)]
#[command(
    name = "procnote-server",
    version,
    about = "procnote API server for using the procnote UI from a browser.",
    group(ArgGroup::new("attachment_store").args(["attachment_dir", "attachment_bucket"])),
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
    /// Cloudflare Access team domain, e.g. `https://lab.cloudflareaccess.com`.
    #[arg(long, requires = "access_aud")]
    access_team_domain: Option<String>,
    /// Audience (AUD) tag of the Cloudflare Access application for this server.
    #[arg(long, requires = "access_team_domain")]
    access_aud: Option<String>,
    /// Run without authentication: recorders are whatever clients send.
    /// For local development only.
    #[arg(long, conflicts_with = "access_team_domain")]
    insecure_no_auth: bool,
    /// PostgreSQL URL to store executions in, instead of the workspace
    /// directory. Templates are still read from the workspace.
    #[arg(long, env = "PROCNOTE_DATABASE_URL", requires = "attachment_store")]
    database_url: Option<String>,
    /// Directory for attachment content when storing executions in PostgreSQL.
    #[arg(long, requires = "database_url")]
    attachment_dir: Option<PathBuf>,
    /// S3-compatible bucket (e.g. Cloudflare R2) for attachment content when
    /// storing executions in PostgreSQL. Configured by the `AWS_*` environment
    /// variables; for R2, `AWS_ENDPOINT=https://<account>.r2.cloudflarestorage.com`,
    /// `AWS_DEFAULT_REGION=auto`, `AWS_ACCESS_KEY_ID` and `AWS_SECRET_ACCESS_KEY`.
    #[arg(long, requires = "database_url")]
    attachment_bucket: Option<String>,
}

impl Args {
    /// Where attachment content goes when executions are in PostgreSQL.
    fn attachment_store(&self) -> Result<Option<Arc<dyn ObjectStore>>, String> {
        match (&self.attachment_dir, &self.attachment_bucket) {
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
}

impl Args {
    fn auth(&self) -> Result<Auth, String> {
        match (&self.access_team_domain, &self.access_aud) {
            (Some(team_domain), Some(audience)) => Ok(Auth::Access(AccessConfig {
                team_domain: team_domain.clone(),
                audience: audience.clone(),
            })),
            _ if self.insecure_no_auth => Ok(Auth::Disabled),
            _ => Err(
                "set --access-team-domain and --access-aud to run behind Cloudflare Access, \
                 or pass --insecure-no-auth for local development"
                    .to_string(),
            ),
        }
    }
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
    let auth = args.auth()?;
    match &auth {
        Auth::Access(config) => {
            log::info!(
                "authenticating with Cloudflare Access ({})",
                config.team_domain
            );
            if !args.listen.ip().is_loopback() {
                log::warn!(
                    "listening on {} with Cloudflare Access: requests that reach this address \
                     without going through Access are rejected, but bind to 127.0.0.1 behind \
                     Cloudflare Tunnel so the origin is not reachable directly",
                    args.listen
                );
            }
        }
        Auth::Disabled => log::warn!("authentication is DISABLED (--insecure-no-auth)"),
    }

    let procedures_dir = args.workspace.canonicalize().map_err(|e| {
        format!(
            "workspace directory does not exist: {}: {e}",
            args.workspace.display()
        )
    })?;
    let upload_dir =
        std::env::temp_dir().join(format!("procnote-uploads-{}", uuid::Uuid::new_v4()));
    let executions: Box<dyn ExecutionRepository> =
        match (&args.database_url, args.attachment_store()?) {
            (Some(url), Some(attachments)) => {
                log::info!("storing executions in PostgreSQL");
                Box::new(PostgresStore::connect(url, attachments).await?)
            }
            _ => Box::new(ExecutionStore::new(procedures_dir.clone())),
        };
    let app = router(ServerConfig {
        procedures_dir: procedures_dir.clone(),
        executions,
        upload_dir,
        static_dir: args.static_dir,
        state_dir: args.state_dir.unwrap_or_else(default_state_dir),
        auth,
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
