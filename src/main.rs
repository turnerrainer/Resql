use std::path::PathBuf;

use anyhow::{Context, Result};
use clap::Parser;
use tokio::net::TcpListener;
use tokio::signal;
use tracing::info;

use resql::config::Config;
use resql::server;

#[derive(Debug, Parser)]
#[command(
    name = "resql",
    version,
    about = "SQL-files-as-REST-endpoints microservice"
)]
struct Cli {
    /// Path to the YAML config file.
    #[arg(
        short = 'c',
        long = "config",
        env = "RESQL_CONFIG",
        default_value = "/app/resql.yaml"
    )]
    config: PathBuf,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();
    let cfg = Config::from_path(&cli.config)
        .with_context(|| format!("loading config from {}", cli.config.display()))?;

    init_tracing(&cfg.logging.level, &cfg.logging.format);

    info!(
        version = env!("CARGO_PKG_VERSION"),
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
    // RESQL_LOG env overrides YAML level directive if set.
    let env_directive = std::env::var("RESQL_LOG").unwrap_or_else(|_| level.to_string());
    let filter = tracing_subscriber::EnvFilter::try_new(env_directive)
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info"));

    use tracing_subscriber::layer::SubscriberExt;
    use tracing_subscriber::util::SubscriberInitExt;

    let base = tracing_subscriber::registry().with(filter);
    match format {
        "json" => base
            .with(tracing_subscriber::fmt::layer().json())
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
