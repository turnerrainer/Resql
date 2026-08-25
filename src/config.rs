use serde::Deserialize;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::config_compat::{preprocess, Diagnostic};
use crate::error::ResqlError;

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default)]
    pub server: ServerConfig,
    #[serde(default = "default_sql_dir")]
    pub sql_dir: PathBuf,
    #[serde(default)]
    pub project_datasource_map: HashMap<String, String>,
    #[serde(default = "default_allow_header")]
    pub allow_datasource_header: bool,
    /// If set, batch requests hitting the Java-legacy `POST /{name}/batch`
    /// URL shape use this datasource (Java hardcoded "byk"). If unset, the
    /// legacy shape is rejected with an actionable error.
    #[serde(default)]
    pub default_datasource: Option<String>,
    #[serde(default)]
    pub datasources: Vec<DatasourceConfig>,
    #[serde(default)]
    pub cors: CorsConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
    #[serde(default)]
    pub openapi: OpenApiConfig,
    /// Diagnostics collected by the compat shim. Populated in
    /// `from_yaml_str`; consumed at boot by `main.rs`. Never serialized.
    #[serde(skip)]
    pub compat_diagnostics: Vec<Diagnostic>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ServerConfig {
    #[serde(default = "default_bind")]
    pub bind: String,
    #[serde(default = "default_max_body")]
    pub max_body_bytes: usize,
    #[serde(default = "default_timeout")]
    pub request_timeout_seconds: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: default_bind(),
            max_body_bytes: default_max_body(),
            request_timeout_seconds: default_timeout(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DatasourceConfig {
    pub name: String,
    pub url: String,
    #[serde(default)]
    pub username: String,
    /// Env-var name to read the DB password from (Rust-canonical form).
    /// Prefer this over `password` for security posture.
    #[serde(default)]
    pub password_env: String,
    /// Plaintext password (Java-compat form). If set, `password_env` must
    /// be empty; validation rejects both being present. The compat shim
    /// emits a WARN whenever this field is used.
    #[serde(default)]
    pub password: Option<String>,
    #[serde(default = "default_max_conns")]
    pub max_connections: u32,
    #[serde(default = "default_acquire_timeout")]
    pub acquire_timeout_seconds: u64,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CorsConfig {
    #[serde(default = "default_cors")]
    pub allowed_origins: String,
}

impl Default for CorsConfig {
    fn default() -> Self {
        Self {
            allowed_origins: default_cors(),
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_format")]
    pub format: String,

    /// Emit one INFO line per completed HTTP request with method,
    /// route, status, duration, project, client, trace_id. On by
    /// default — the operational access log every service should have.
    #[serde(default = "default_true")]
    pub access_log: bool,

    /// When a request returns an error response, include the error's
    /// `source()` chain on the WARN log line. Off by default — top-
    /// level `Display` is usually enough and chains can leak schema
    /// details from the underlying driver.
    #[serde(default)]
    pub print_stack_trace: bool,

    /// Cap on any body content included in a log line. Defaults to
    /// 2 KiB — enough to identify the shape without shipping full
    /// payloads to the log store.
    #[serde(default = "default_max_body_bytes")]
    pub max_body_bytes: usize,

    /// JSON body field names replaced with `"[REDACTED]"` in any
    /// logged body. Case-insensitive, applied at every nesting depth.
    /// Defaults cover common secret-bearing field names.
    #[serde(default = "default_redact_body_fields")]
    pub redact_body_fields: Vec<String>,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            format: default_log_format(),
            access_log: true,
            print_stack_trace: false,
            max_body_bytes: default_max_body_bytes(),
            redact_body_fields: default_redact_body_fields(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_max_body_bytes() -> usize {
    2048
}

fn default_redact_body_fields() -> Vec<String> {
    vec![
        "password".into(),
        "pass".into(),
        "secret".into(),
        "token".into(),
        "access_token".into(),
        "refresh_token".into(),
        "api_key".into(),
        "authorization".into(),
    ]
}

/// Optional customisation for the generated OpenAPI 3.1 spec exposed
/// at `/openapi.json`. Every field is optional; the generator falls
/// back to sensible defaults so operators can ship without touching
/// this block at all.
#[derive(Debug, Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct OpenApiConfig {
    #[serde(default = "default_openapi_title")]
    pub title: String,
    #[serde(default = "default_openapi_description")]
    pub description: String,
    #[serde(default = "default_openapi_server_url")]
    pub server_url: String,
}

impl Default for OpenApiConfig {
    fn default() -> Self {
        Self {
            title: default_openapi_title(),
            description: default_openapi_description(),
            server_url: default_openapi_server_url(),
        }
    }
}

fn default_openapi_title() -> String {
    "Resql".into()
}
fn default_openapi_description() -> String {
    "SQL-files-as-REST endpoints. Spec generated from per-file declarations.".into()
}
fn default_openapi_server_url() -> String {
    "/".into()
}

fn default_bind() -> String {
    "0.0.0.0:8080".into()
}
fn default_sql_dir() -> PathBuf {
    // Match the Java default (`sqlms.saved-queries-dir: ./templates/`) so a
    // Java operator's tree is discovered without them setting `sql_dir:`.
    PathBuf::from("./templates/")
}
fn default_max_body() -> usize {
    1_048_576
}
fn default_timeout() -> u64 {
    30
}
fn default_allow_header() -> bool {
    true
}
fn default_max_conns() -> u32 {
    10
}
fn default_acquire_timeout() -> u64 {
    5
}
fn default_cors() -> String {
    "*".into()
}
fn default_log_level() -> String {
    "info,resql=debug".into()
}
fn default_log_format() -> String {
    "text".into()
}

impl Config {
    pub fn from_path(path: &Path) -> Result<Self, ResqlError> {
        let text = std::fs::read_to_string(path).map_err(|e| ResqlError::InvalidDirectory {
            path: path.to_path_buf(),
            reason: format!("cannot read config: {e}"),
        })?;
        Self::from_yaml_str(&text)
    }

    pub fn from_yaml_str(text: &str) -> Result<Self, ResqlError> {
        // 1. Parse to a raw Value tree.
        let raw: serde_yaml_ng::Value = serde_yaml_ng::from_str(text)
            .map_err(|e| ResqlError::Internal(format!("invalid config YAML: {e}")))?;
        // 2. Run the Java-compat preprocessor. This rewrites the Value tree
        //    into the Rust canonical shape and collects diagnostics.
        let (normalized, diags) = preprocess(raw);
        // 3. Deserialize the normalized shape into the Config struct.
        let mut cfg: Config = serde_yaml_ng::from_value(normalized)
            .map_err(|e| ResqlError::Internal(format!("invalid config YAML: {e}")))?;
        cfg.compat_diagnostics = diags;
        cfg.validate()?;
        Ok(cfg)
    }

    fn validate(&self) -> Result<(), ResqlError> {
        let mut seen = std::collections::HashSet::new();
        for ds in &self.datasources {
            if !seen.insert(ds.name.clone()) {
                return Err(ResqlError::Internal(format!(
                    "duplicate datasource name '{}'",
                    ds.name
                )));
            }
            let has_plaintext = ds.password.as_deref().is_some_and(|s| !s.is_empty());
            if has_plaintext && !ds.password_env.is_empty() {
                return Err(ResqlError::Internal(format!(
                    "datasource '{}' sets BOTH `password` (plaintext) and `password_env`; use exactly one",
                    ds.name
                )));
            }
            if !ds.username.is_empty() && ds.password_env.is_empty() && !has_plaintext {
                return Err(ResqlError::Internal(format!(
                    "datasource '{}' sets username but no password / password_env — refusing to start with an unauthenticated connection",
                    ds.name
                )));
            }
            if !ds.password_env.is_empty() && std::env::var(&ds.password_env).is_err() {
                return Err(ResqlError::Internal(format!(
                    "datasource '{}' references env var {} which is not set",
                    ds.name, ds.password_env
                )));
            }
        }
        for (project, ds_name) in &self.project_datasource_map {
            if !self.datasources.iter().any(|d| &d.name == ds_name) {
                return Err(ResqlError::Internal(format!(
                    "project_datasource_map maps project '{project}' to '{ds_name}' which is not in datasources"
                )));
            }
        }
        if let Some(ref ds_name) = self.default_datasource {
            if !self.datasources.iter().any(|d| &d.name == ds_name) {
                return Err(ResqlError::Internal(format!(
                    "default_datasource '{ds_name}' is not in datasources"
                )));
            }
        }
        Ok(())
    }

    /// Resolve project → datasource name using the map, falling back to project name.
    pub fn datasource_for_project(&self, project: &str) -> String {
        self.project_datasource_map
            .get(project)
            .cloned()
            .unwrap_or_else(|| project.to_string())
    }
}

impl DatasourceConfig {
    /// Return the URL with the password interpolated in the userinfo section.
    /// Prefers `password_env` (Rust-canonical, env-var indirection); falls
    /// back to the plaintext `password` field if only that is set (Java-compat
    /// shape). If neither is set, returns the URL unchanged.
    pub fn resolved_url(&self) -> Result<String, ResqlError> {
        let (pw, source) = if !self.password_env.is_empty() {
            let value = std::env::var(&self.password_env).map_err(|_| {
                ResqlError::Internal(format!(
                    "env var {} referenced by datasource '{}' is not set",
                    self.password_env, self.name
                ))
            })?;
            (value, "password_env")
        } else if let Some(plain) = self.password.as_deref().filter(|s| !s.is_empty()) {
            (plain.to_string(), "password")
        } else {
            return Ok(self.url.clone());
        };
        let _ = source; // named for readability, no runtime effect
        Ok(inject_userinfo(&self.url, &self.username, &pw))
    }
}

fn inject_userinfo(url: &str, user: &str, pw: &str) -> String {
    // Only touches the URL if it has a `scheme://` prefix. Leaves urls that
    // already carry userinfo alone (assume the caller knows better).
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if rest.contains('@') {
            return url.to_string();
        }
        let user_enc = urlencode(user);
        let pw_enc = urlencode(pw);
        format!("{scheme}{user_enc}:{pw_enc}@{rest}")
    } else {
        url.to_string()
    }
}

fn urlencode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.chars() {
        if c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.' | '~') {
            out.push(c);
        } else {
            for b in c.to_string().bytes() {
                out.push_str(&format!("%{b:02X}"));
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_when_only_sql_dir_specified() {
        let cfg = Config::from_yaml_str("sql_dir: ./sql\n").unwrap();
        assert_eq!(cfg.server.bind, "0.0.0.0:8080");
        assert_eq!(cfg.server.max_body_bytes, 1_048_576);
        assert!(cfg.allow_datasource_header);
        assert!(cfg.datasources.is_empty());
    }

    #[test]
    fn deny_unknown_top_level_field() {
        let err = Config::from_yaml_str("sql_dir: ./sql\ntypo: 1\n").unwrap_err();
        assert!(err
            .to_string()
            .to_lowercase()
            .contains("invalid config yaml"));
    }

    #[test]
    fn datasource_username_without_env_rejected() {
        let yaml = r#"
sql_dir: ./sql
datasources:
  - name: x
    url: "sqlite::memory:"
    username: u
"#;
        let err = Config::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("no password"), "err = {err:?}");
    }

    #[test]
    fn duplicate_datasource_names_rejected() {
        let yaml = r#"
sql_dir: ./sql
datasources:
  - name: x
    url: "sqlite::memory:"
  - name: x
    url: "sqlite::memory:"
"#;
        let err = Config::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("duplicate"));
    }

    #[test]
    fn project_map_referencing_unknown_datasource_rejected() {
        let yaml = r#"
sql_dir: ./sql
project_datasource_map:
  crm: nope
datasources:
  - name: x
    url: "sqlite::memory:"
"#;
        let err = Config::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("nope"));
    }

    #[test]
    fn datasource_for_project_defaults_to_project_name() {
        let cfg = Config::from_yaml_str("sql_dir: ./sql\n").unwrap();
        assert_eq!(cfg.datasource_for_project("crm"), "crm");
    }

    #[test]
    fn datasource_for_project_uses_map() {
        let yaml = r#"
sql_dir: ./sql
project_datasource_map:
  crm: db1
datasources:
  - name: db1
    url: "sqlite::memory:"
"#;
        let cfg = Config::from_yaml_str(yaml).unwrap();
        assert_eq!(cfg.datasource_for_project("crm"), "db1");
        assert_eq!(cfg.datasource_for_project("other"), "other");
    }

    #[test]
    fn inject_userinfo_only_when_absent() {
        assert_eq!(
            inject_userinfo("postgres://host:5432/db", "u", "p"),
            "postgres://u:p@host:5432/db"
        );
        assert_eq!(
            inject_userinfo("postgres://already:set@host/db", "u", "p"),
            "postgres://already:set@host/db"
        );
    }

    #[test]
    fn urlencode_percent_encodes_special_chars() {
        assert_eq!(urlencode("p@ss w/ord!"), "p%40ss%20w%2Ford%21");
        assert_eq!(urlencode("simple"), "simple");
    }
}
