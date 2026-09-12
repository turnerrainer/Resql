use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::{info, warn};

use resql::config::Config;
use resql::config_compat::DiagLevel;
use resql::server;

#[derive(Debug, Parser)]
#[command(
    name = "resql",
    version,
    about = "SQL-files-as-REST-endpoints microservice"
)]
struct Cli {
    /// Path to the YAML config file. When omitted, the following candidates
    /// are tried in order (Rust-canonical first, Java-canonical second):
    /// `/app/resql.yaml`, `./resql.yaml`, `./application.yml` (Java default),
    /// `./application-prod.yml`, `./application-dev.yml`, `./application-test.yml`.
    /// The first existing candidate wins.
    #[arg(short = 'c', long = "config", env = "RESQL_CONFIG")]
    config: Option<PathBuf>,
}

const CONFIG_CANDIDATES: &[&str] = &[
    "/app/resql.yaml",
    "./resql.yaml",
    "./application.yml",
    "./application-prod.yml",
    "./application-dev.yml",
    "./application-test.yml",
];

fn resolve_config_path(cli: &Cli) -> Result<PathBuf> {
    if let Some(explicit) = &cli.config {
        return Ok(explicit.clone());
    }
    for c in CONFIG_CANDIDATES {
        let p = Path::new(c);
        if p.exists() {
            return Ok(p.to_path_buf());
        }
    }
    anyhow::bail!(
        "no config file found; searched {} and none exist. Set -c/--config or RESQL_CONFIG.",
        CONFIG_CANDIDATES.join(", ")
    )
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let config_path = resolve_config_path(&cli)?;
    let cfg = Config::from_path(&config_path)
        .with_context(|| format!("loading config from {}", config_path.display()))?;

    init_tracing(&cfg.logging.level, &cfg.logging.format);

    // Boot-time diagnostic pass (REFACTO-REQUIREMENTS §6.2): every Java-shape
    // field the compat shim recognised is announced here so the operator can
    // determine "am I depending on anything the target doesn't implement?"
    // from a single boot-log read.
    for d in &cfg.compat_diagnostics {
        match d.level {
            DiagLevel::Warn => warn!(field = %d.source_field, "{}", d.message),
            DiagLevel::Info => info!(field = %d.source_field, "{}", d.message),
        }
    }

    // Fleet stronghold §3.1: fail fast before we bind a public
    // interface with no authentication story. `validate()` handles
    // semantic checks; this method covers the deployment posture and
    // is intentionally separate so a compat-shim parse still works.
    cfg.validate_runtime_posture()
        .context("boot-time security-posture check")?;

    info!(
        version = env!("CARGO_PKG_VERSION"),
        config = %config_path.display(),
        bind = %cfg.server.bind,
        sql_dir = %cfg.sql_dir.display(),
        datasources = cfg.datasources.len(),
        "starting Resql"
    );

    let state = server::init(cfg.clone())
        .await
        .context("initialising app state")?;

    info!(
        endpoints = state.index.len(),
        datasources = state.registry.names().len(),
        "app state ready"
    );

    let app = server::router(state);
    let listener = TcpListener::bind(&cfg.server.bind)
        .await
        .with_context(|| format!("binding {}", cfg.server.bind))?;
    let addr = listener.local_addr().context("querying local addr")?;
    info!(%addr, "listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("axum serve")?;
    info!("bye");
    Ok(())
}

fn init_tracing(level: &str, format: &str) {
    // RESQL_LOG env overrides YAML level; RESQL_LOG_FORMAT overrides
    // YAML format so an operator can flip a running container's log
    // shape (text ↔ json) without editing the config file.
    let env_directive = std::env::var("RESQL_LOG").unwrap_or_else(|_| level.to_string());
    let filter = tracing_subscriber::EnvFilter::try_new(env_directive)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));
    let effective_format = std::env::var("RESQL_LOG_FORMAT")
        .ok()
        .map(|s| s.to_ascii_lowercase())
        .unwrap_or_else(|| format.to_ascii_lowercase());

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let base = tracing_subscriber::registry().with(filter);
    match effective_format.as_str() {
        "json" => base
            .with(
                tracing_subscriber::fmt::layer()
                    .json()
                    .with_current_span(true)
                    .with_span_list(false),
            )
            .try_init()
            .ok(),
        _ => base.with(tracing_subscriber::fmt::layer()).try_init().ok(),
    };
}

async fn shutdown_signal() {
    let ctrl_c = async {
        signal::ctrl_c().await.ok();
    };
    #[cfg(unix)]
    let terminate = async {
        signal::unix::signal(signal::unix::SignalKind::terminate())
            .expect("register SIGTERM handler")
            .recv()
            .await;
    };
    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    info!("shutdown signal received");
}
