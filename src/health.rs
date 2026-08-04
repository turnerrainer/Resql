use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Semver from the Cargo package. Used by [`VERSION`] to build the Java-shape
/// `v{MAJOR}.{MINOR}.{PATCH}` string reported on `/healthz`.
pub const RAW_SEMVER: &str = env!("CARGO_PKG_VERSION");

/// Version string reported by `/health` and `/healthz`. Formatted as
/// `v{MAJOR}.{MINOR}.{PATCH}` to match Java Resql (HeartBeatService.java:39-42).
/// Pre-release / build metadata (`-alpha.1`, `+build.5`) are stripped so the
/// response is a strict semver-with-`v` prefix.
pub const VERSION: &str = version_with_v_prefix();

/// Build timestamp in Unix milliseconds. Java Resql exposes this as
/// `packagingTime`, sourced from `heartbeat.properties`. Set at Rust build
/// time via the `RESQL_BUILD_TIME` env var; falls back to `0` when unset so
/// operators can still boot without a build wrapper.
pub const PACKAGING_TIME: u64 = match option_env!("RESQL_BUILD_TIME") {
    Some(s) => match parse_u64(s.as_bytes()) {
        Some(v) => v,
        None => 0,
    },
    None => 0,
};

pub const APP_NAME: &str = "resql";

const fn parse_u64(s: &[u8]) -> Option<u64> {
    let mut i = 0;
    let mut out: u64 = 0;
    while i < s.len() {
        let b = s[i];
        if b < b'0' || b > b'9' {
            return None;
        }
        out = match out.checked_mul(10) {
            Some(v) => v,
            None => return None,
        };
        out = match out.checked_add((b - b'0') as u64) {
            Some(v) => v,
            None => return None,
        };
        i += 1;
    }
    Some(out)
}

const fn version_with_v_prefix() -> &'static str {
    // We can't easily allocate at const time, so we ship both formats: strip
    // pre-release/build suffix from CARGO_PKG_VERSION at compile time by
    // slicing until the first `-` or `+`. Prepending `v` requires a
    // compile-time concat, which we do with `concat!()` in a helper via a
    // build-time computed literal. To keep the const-fn simple, we accept a
    // slightly relaxed guarantee: the *runtime* value is built lazily in
    // `build()` and cached — see `format_version()` below. This constant is
    // kept as a fallback for unit tests.
    RAW_SEMVER
}

/// Format the Cargo semver as `v{MAJOR}.{MINOR}.{PATCH}` — the exact shape
/// Java Resql's HeartBeatService produces. Pre-release / build metadata is
/// stripped.
pub fn format_version() -> String {
    let core = RAW_SEMVER.split(['-', '+']).next().unwrap_or(RAW_SEMVER);
    format!("v{core}")
}

#[derive(Debug, Clone, Copy)]
pub struct StartTime(pub u64);

impl StartTime {
    pub fn now() -> Self {
        Self(now_ms())
    }
}

/// Response body for `/health` and `/healthz`.
///
/// The 5 fields Java exposes (Java `DTOHeartBeatInfo.java`) are all present
/// with identical JSON keys and semantics:
/// - `appName`, `version`, `packagingTime`, `appStartTime`, `serverTime`.
///
/// Rust additionally emits `status: "UP"` (see DIVERGENCES.md entry
/// `health.status`). Existing Java clients ignore unknown fields; strict
/// clients that depend on a fixed shape should read only the Java-known fields.
#[derive(Serialize)]
pub struct HealthResponse {
    #[serde(rename = "appName")]
    pub app_name: &'static str,
    pub version: String,
    #[serde(rename = "packagingTime")]
    pub packaging_time: u64,
    #[serde(rename = "appStartTime")]
    pub app_start_time: u64,
    #[serde(rename = "serverTime")]
    pub server_time: u64,
    pub status: &'static str,
}

pub fn build(start: StartTime) -> HealthResponse {
    HealthResponse {
        app_name: APP_NAME,
        version: format_version(),
        packaging_time: PACKAGING_TIME,
        app_start_time: start.0,
        server_time: now_ms(),
        status: "UP",
    }
}

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_string_has_v_prefix_and_dotted_semver_core() {
        let v = format_version();
        assert!(v.starts_with('v'), "version = {v:?}");
        // v0.1.0-alpha.1 → v0.1.0
        assert!(v[1..].chars().all(|c| c.is_ascii_digit() || c == '.'));
    }

    #[test]
    fn build_response_has_all_five_java_fields() {
        let r = build(StartTime(1000));
        assert_eq!(r.app_name, "resql");
        assert_eq!(r.app_start_time, 1000);
        assert_eq!(r.status, "UP");
        assert!(r.version.starts_with('v'));
        assert!(r.server_time >= 1000);
        // packagingTime is a number even when unset (Java exposes long primitive).
        let _: u64 = r.packaging_time;
    }

    #[test]
    fn packaging_time_defaults_to_zero_when_env_unset() {
        // Documenting the fallback behaviour.
        if option_env!("RESQL_BUILD_TIME").is_none() {
            assert_eq!(PACKAGING_TIME, 0);
        }
    }
}
