//! Fleet stronghold §8.2: `resql doctor` — pre-boot health check.
//!
//! Spawn the compiled binary (via `env!("CARGO_BIN_EXE_resql")`) and
//! verify the exit-code contract:
//!   0 — every check passed
//!   1 — hard error (parse failure or runtime-posture refuse)
//!   2 — warnings only, `--strict` flag set
//!
//! No listener is opened. No datasource pool is created. Ops teams
//! wire this into CI as a non-blocking check (drop `--strict`) or as
//! a blocking gate (add `--strict`).

use std::io::Write;
use std::process::Command;

fn resql_bin() -> &'static str {
    env!("CARGO_BIN_EXE_resql")
}

fn write_config(contents: &str) -> tempfile::NamedTempFile {
    let mut f = tempfile::Builder::new()
        .prefix("resql-doctor-")
        .suffix(".yaml")
        .tempfile()
        .unwrap();
    f.write_all(contents.as_bytes()).unwrap();
    f.flush().unwrap();
    f
}

#[test]
fn doctor_exits_zero_on_loopback_bind() {
    let cfg = write_config("sql_dir: ./sql\nserver:\n  bind: \"127.0.0.1:8080\"\n");
    let out = Command::new(resql_bin())
        .args(["doctor", "-c"])
        .arg(cfg.path())
        .output()
        .expect("spawn doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(
        out.status.success(),
        "expected exit 0, got {}: stdout={stdout}",
        out.status
    );
    assert!(stdout.contains("OK  : config parsed"));
    assert!(stdout.contains("OK  : runtime-posture check"));
}

#[test]
fn doctor_exits_one_on_non_loopback_bind_without_auth() {
    // Fleet §3.1: this is the F-RES-3 shape. Doctor must fail 1.
    let cfg = write_config("sql_dir: ./sql\nserver:\n  bind: \"0.0.0.0:8080\"\n");
    let out = Command::new(resql_bin())
        .args(["doctor", "-c"])
        .arg(cfg.path())
        .output()
        .expect("spawn doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {}: stdout={stdout}",
        out.status
    );
    assert!(stdout.contains("ERROR: runtime-posture"));
    assert!(stdout.contains("non-loopback bind"));
}

#[test]
fn doctor_exits_one_on_config_parse_failure() {
    let cfg = write_config("not: valid: yaml: at: all: [\n");
    let out = Command::new(resql_bin())
        .args(["doctor", "-c"])
        .arg(cfg.path())
        .output()
        .expect("spawn doctor");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert_eq!(
        out.status.code(),
        Some(1),
        "expected exit 1, got {}: stdout={stdout}",
        out.status
    );
    assert!(stdout.contains("ERROR: config parse"));
}

#[test]
fn doctor_help_lists_subcommand() {
    // Regression pin: fleet §8.2 requires the subcommand to be
    // discoverable via `--help` so operators can grep for it.
    let out = Command::new(resql_bin())
        .arg("--help")
        .output()
        .expect("spawn help");
    let stdout = String::from_utf8_lossy(&out.stdout);
    assert!(stdout.contains("doctor"), "help missing 'doctor': {stdout}");
}
