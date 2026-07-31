use serde::Serialize;
use std::time::{SystemTime, UNIX_EPOCH};

/// Version reported by /health and /healthz. Baked at build time from
/// CARGO_PKG_VERSION so consumers cannot see any drift between the image
/// tag and the runtime response.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

pub const APP_NAME: &str = "resql";

#[derive(Debug, Clone, Copy)]
pub struct StartTime(pub u64);

impl StartTime {
    pub fn now() -> Self {
        Self(now_ms())
    }
}

#[derive(Serialize)]
pub struct HealthResponse {
    #[serde(rename = "appName")]
    pub app_name: &'static str,
    pub version: &'static str,
    #[serde(rename = "appStartTime")]
    pub app_start_time: u64,
    #[serde(rename = "serverTime")]
    pub server_time: u64,
    pub status: &'static str,
}

pub fn build(start: StartTime) -> HealthResponse {
    HealthResponse {
        app_name: APP_NAME,
        version: VERSION,
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
    fn version_is_a_semver_string() {
        assert!(!VERSION.is_empty());
        assert!(VERSION.chars().next().unwrap().is_ascii_digit());
    }

    #[test]
    fn build_response_has_all_fields() {
        let r = build(StartTime(1000));
        assert_eq!(r.app_name, "resql");
        assert_eq!(r.app_start_time, 1000);
        assert_eq!(r.status, "UP");
        assert!(r.server_time >= 1000);
    }
}
