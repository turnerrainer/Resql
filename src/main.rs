use std::path::{Path, PathBuf};
use std::process::ExitCode;

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
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
    #[arg(short = 'c', long = "config", env = "RESQL_CONFIG", global = true)]
    config: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the HTTP service (default when no subcommand is given).
    Serve,
    /// Pre-boot health check — parse the config, run every safety
    /// check that `serve` runs, and exit non-zero if the deployment
    /// would fail to boot or would boot into a knowingly unsafe
    /// posture. Never binds a port, never opens a datasource pool,
    /// never mutates any external state.
    ///
    /// Exit code convention:
    ///   0 — every check passed
    ///   1 — one or more errors (config parse failure, semantic
    ///       validation, refuse-on-non-loopback-without-auth)
    ///   2 — no errors but one or more security warnings emitted;
    ///       only returned when `--strict` is passed. Without
    ///       `--strict`, warnings print but exit stays 0.
    Doctor {
        /// Elevate security warnings to failures (exit code 2).
        /// Off by default so operators can wire `doctor` into CI as a
        /// non-blocking check and still see the warning stream.
        #[arg(long)]
        strict: bool,
    },
}

const CONFIG_CANDIDATES: &[&str] = &[
    "/app/resql.yaml",
    "./resql.yaml",
    "./application.yml",
    "./application-prod.yml",
    "./application-dev.yml",
    "./application-test.yml",
];

fn resolve_config_path(explicit: Option<&PathBuf>) -> Result<PathBuf> {
    if let Some(explicit) = explicit {
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

fn main() -> ExitCode {
    let cli = Cli::parse();
    match cli.command.as_ref().unwrap_or(&Command::Serve) {
        Command::Serve => match tokio::runtime::Builder::new_multi_thread()
            .enable_all()
            .build()
            .expect("tokio runtime")
            .block_on(run_serve(cli.config.as_ref()))
        {
            Ok(()) => ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("Error: {e:?}");
                ExitCode::FAILURE
            }
        },
        Command::Doctor { strict } => doctor(cli.config.as_ref(), *strict),
    }
}

async fn run_serve(config: Option<&PathBuf>) -> Result<()> {
    let config_path = resolve_config_path(config)?;
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

/// Fleet stronghold §8.2 — pre-boot health check. Reads and parses
/// the config, runs every safety check that `serve` runs (semantic
/// validation via `Config::from_path` -> `validate()`, then
/// `validate_runtime_posture()` for fleet §3.1), and prints
/// compat-shim diagnostics in the same order `serve` would emit them.
/// Never binds a port, never opens a datasource pool — this is
/// deliberately side-effect-free so ops teams can point it at a
/// candidate config in staging without disturbing anything.
///
/// Output goes to stdout for easy piping / grep; exit code is the
/// contract with CI (see `Command::Doctor` docs for the mapping).
fn doctor(config: Option<&PathBuf>, strict: bool) -> ExitCode {
    let config_path = match resolve_config_path(config) {
        Ok(p) => p,
        Err(e) => {
            println!("ERROR: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("resql doctor — config: {}", config_path.display());
    let cfg = match Config::from_path(&config_path) {
        Ok(c) => c,
        Err(e) => {
            println!("ERROR: config parse / semantic validation failed: {e}");
            return ExitCode::FAILURE;
        }
    };
    println!("OK  : config parsed and semantically valid");

    // Compat-shim diagnostics — informational, don't gate the exit
    // code. Same output the operator sees at `serve` boot.
    for d in &cfg.compat_diagnostics {
        let level = match d.level {
            DiagLevel::Warn => "WARN",
            DiagLevel::Info => "INFO",
        };
        println!("{level}: [compat] {}: {}", d.source_field, d.message);
    }

    // NOTE: `Config::security_warnings()` (fleet §8.1, PR #40 stacked
    // on #32) is not on this branch. When those merge to dev this
    // doctor should loop `for w in cfg.security_warnings() { warnings
    // += 1; println!("WARN: [security] {}: {}", w.field, w.message); }`
    // right here. Stub kept explicit so the follow-up is a one-line
    // change.
    let warnings = 0usize;

    match cfg.validate_runtime_posture() {
        Ok(()) => {
            println!("OK  : runtime-posture check (fleet §3.1)");
        }
        Err(e) => {
            println!("ERROR: runtime-posture check (fleet §3.1) failed: {e}");
            return ExitCode::FAILURE;
        }
    }

    println!("\ndoctor summary: {warnings} warning(s), errors: 0. strict={strict}",);
    if warnings > 0 && strict {
        // Exit 2 (distinct from "hard error" 1) so CI can distinguish
        // "config is warning-clean" from "config actually refuses to
        // boot". Only fires with --strict; otherwise warnings are
        // informational.
        return ExitCode::from(2);
    }
    ExitCode::SUCCESS
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
