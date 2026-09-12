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
    /// Per-project allowlist for `X-Datasource` overrides. Consulted
    /// only when `allow_datasource_header` is true. Any (project,
    /// header value) not present here → 403. An entry listed here that
    /// names a datasource which is not in `datasources` → validation
    /// error at boot (fail fast). Absence of a project key means "no
    /// overrides allowed for that project"; the default when
    /// `allow_datasource_header` is on and no entries exist is that
    /// every header override is denied.
    #[serde(default)]
    pub datasource_header_allowlist: HashMap<String, Vec<String>>,
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
    #[serde(default)]
    pub admin: AdminConfig,
    #[serde(default)]
    pub security: SecurityConfig,
    /// Diagnostics collected by the compat shim. Populated in
    /// `from_yaml_str`; consumed at boot by `main.rs`. Never serialized.
    #[serde(skip)]
    pub compat_diagnostics: Vec<Diagnostic>,
}

/// Optional inter-service authentication surface. Default is OFF —
/// Resql's design is "internal-only, sitting behind Ruuter", and every
/// enabled flag here documents an intentional deviation from that
/// posture. See the F-RES-3 finding in the h2ck.me v1 runtime
/// break-test: a Resql that's ever directly reachable (dev, staging
/// with a misconfigured proxy, or an unguarded Ruuter DSL) is one
/// `curl` away from unauth SQL execution against every registered
/// datasource. This block lets operators opt into a shared-secret
/// bearer gate for the query endpoints.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct SecurityConfig {
    /// Name of the env var whose value is the required inter-service
    /// bearer token. When set, every request except `/health` and
    /// `/healthz` MUST carry `Authorization: Bearer <value-of-env-var>`
    /// or receive 401. Missing env var, empty env-var value, or the
    /// field left unset all mean "no bearer required" (the default).
    ///
    /// Comparison is constant-time on equal-length inputs and length
    /// mismatch rejects up front — a length side-channel is not
    /// exploitable because the token length is not a secret; the
    /// content is.
    ///
    /// Design note: the token lives in an env var rather than the YAML
    /// file itself so the same posture as `password_env` applies —
    /// secrets never sit in files that end up in image layers or
    /// backups.
    #[serde(default)]
    pub inter_service_token_env: String,
    /// Opt-out for the boot-time refuse-on-non-loopback check (fleet
    /// stronghold §3.1). By default, if `server.bind` is a non-loopback
    /// address AND no bearer gate is configured, boot fails with an
    /// actionable message — the "public bind, no auth" posture is the
    /// exact shape F-RES-3 was filed against and there is no reason to
    /// start a service in that state without a conscious operator
    /// decision.
    ///
    /// Set to `true` when a reverse proxy (Ruuter in Bürokratt, or an
    /// nginx / Traefik / Envoy in front) authenticates every request
    /// before it reaches Resql. That's the standard delegate-auth
    /// posture — Resql sees only pre-authenticated traffic and can
    /// safely bind to a non-loopback address. When the proxy is missing
    /// (dev without Ruuter; container exposed directly), leave this
    /// false and configure `inter_service_token_env` instead.
    #[serde(default)]
    pub trust_network: bool,
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

/// Admin-facing behaviour toggles. Every field defaults to the safer
/// (more locked-down) posture so a stock deployment surfaces the
/// minimum reconnaissance surface. Operators lift restrictions
/// explicitly per site.
#[derive(Debug, Clone, Deserialize, Default)]
#[serde(deny_unknown_fields)]
pub struct AdminConfig {
    /// When true, `GET /datasources` returns the (redacted) list. When
    /// false (default), the endpoint returns 404 — indistinguishable
    /// from a non-mounted endpoint, so unauth callers can't tell whether
    /// the service is Resql at all.
    #[serde(default)]
    pub datasources_public: bool,
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
    // Default OFF: header-driven datasource routing is a lateral-move
    // lane inside the service trust boundary. Operators who need it
    // must opt in explicitly and populate `datasource_header_allowlist`.
    false
}
fn default_max_conns() -> u32 {
    10
}
fn default_acquire_timeout() -> u64 {
    5
}
fn default_cors() -> String {
    // Default DENY: no CORS layer attached means no
    // `Access-Control-Allow-Origin` header, so browsers refuse
    // cross-origin responses. Operators who need cross-origin must
    // set it explicitly.
    "".into()
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
        if self.server.request_timeout_seconds == 0 {
            return Err(ResqlError::Internal(
                "server.request_timeout_seconds must be > 0 — a zero value would allow slow queries to exhaust the connection pool".into(),
            ));
        }
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
        for (project, allowed) in &self.datasource_header_allowlist {
            for ds_name in allowed {
                if !self.datasources.iter().any(|d| &d.name == ds_name) {
                    return Err(ResqlError::Internal(format!(
                        "datasource_header_allowlist for project '{project}' names '{ds_name}' which is not in datasources"
                    )));
                }
            }
        }
        // F-RES-3: if the operator wired a bearer-gate env var name,
        // fail fast at boot if the env var is missing or empty. The
        // failure mode we want to avoid is a config that names an env
        // var, the operator forgets to set it, and the service silently
        // boots with the gate disabled — matching the pre-fix posture
        // rather than the intended locked-down one.
        if !self.security.inter_service_token_env.is_empty() {
            let key = &self.security.inter_service_token_env;
            match std::env::var(key) {
                Err(_) => {
                    return Err(ResqlError::Internal(format!(
                        "security.inter_service_token_env references env var {key} which is not set"
                    )));
                }
                Ok(v) if v.is_empty() => {
                    return Err(ResqlError::Internal(format!(
                        "security.inter_service_token_env references env var {key} but its value is empty"
                    )));
                }
                Ok(_) => {}
            }
        }
        Ok(())
    }

    /// Runtime-posture check separate from the semantic `validate()`.
    /// `validate()` covers field shapes and cross-references — anything
    /// a parser needs to succeed. This method covers the deployment
    /// posture the operator is starting the process into. Called from
    /// `main.rs` before any listener binds; not called from
    /// `from_yaml_str` so that a compat-shim or fixture parse works
    /// even on a config whose bind posture would refuse to boot.
    ///
    /// Fleet stronghold §3.1: refuse to start on a non-loopback bind
    /// that has no authentication story. The three ways to satisfy
    /// this check are:
    ///   (a) bind to loopback (127.0.0.1 / ::1 / localhost) and let a
    ///       reverse proxy forward to it,
    ///   (b) enable the built-in bearer gate via
    ///       `security.inter_service_token_env`,
    ///   (c) set `security.trust_network: true` to certify that a
    ///       reverse proxy already authenticates every request.
    /// Any other posture is one HTTP request away from unauth SQL
    /// execution — the exact F-RES-3 shape — and starting the service
    /// silently is a worse operator experience than a loud, actionable
    /// boot failure.
    pub fn validate_runtime_posture(&self) -> Result<(), ResqlError> {
        if is_non_loopback_bind(&self.server.bind)
            && self.security.inter_service_token_env.is_empty()
            && !self.security.trust_network
        {
            return Err(ResqlError::Internal(format!(
                "refusing to start on non-loopback bind {bind} without an authentication story. \
                 Do one of: (a) bind to 127.0.0.1:<port> and let a reverse proxy forward, \
                 (b) set security.inter_service_token_env to enable the built-in bearer gate, \
                 or (c) set security.trust_network: true to certify that a reverse proxy \
                 authenticates every request before it reaches Resql.",
                bind = self.server.bind
            )));
        }
        Ok(())
    }

    /// Resolve the configured bearer token from its env var, or
    /// `None` if the gate is not enabled. Called by the auth
    /// middleware on every request — the env-var value is cached in
    /// the AppState so we don't hit `getenv` per request.
    pub fn resolved_inter_service_token(&self) -> Option<String> {
        if self.security.inter_service_token_env.is_empty() {
            return None;
        }
        match std::env::var(&self.security.inter_service_token_env) {
            Ok(v) if !v.is_empty() => Some(v),
            _ => None,
        }
    }

    /// Resolve project → datasource name using the map, falling back to project name.
    pub fn datasource_for_project(&self, project: &str) -> String {
        self.project_datasource_map
            .get(project)
            .cloned()
            .unwrap_or_else(|| project.to_string())
    }
}

/// True when `bind` is not a loopback / localhost address. Handles the
/// three shapes an operator is likely to write: `0.0.0.0:PORT`,
/// `[::]:PORT`, and `HOST:PORT` where HOST is a hostname or public IP.
/// Loopback bindings (`127.0.0.1`, `::1`, `localhost`) return `false`.
/// See fleet stronghold §3.1 for why boot refuses on non-loopback
/// without an auth story.
fn is_non_loopback_bind(bind: &str) -> bool {
    let host = if let Some(stripped) = bind.strip_prefix('[') {
        match stripped.find(']') {
            Some(end) => &stripped[..end],
            None => bind,
        }
    } else {
        match bind.rfind(':') {
            Some(idx) => &bind[..idx],
            None => bind,
        }
    };
    !matches!(host, "127.0.0.1" | "::1" | "localhost")
}

impl DatasourceConfig {
    /// Return the URL with the password interpolated in the userinfo section.
    /// Prefers `password_env` (Rust-canonical, env-var indirection); falls
    /// back to the plaintext `password` field if only that is set (Java-compat
    /// shape). If neither is set, returns the URL unchanged.
    ///
    /// When the URL already carries a `user:pw@` userinfo component *and*
    /// a password is being injected, the config-supplied credential wins —
    /// the URL's userinfo is stripped and replaced. A WARN log line names
    /// the collision so an operator who set `password_env` and then edited
    /// the URL to a new password (a natural rotation instinct) notices that
    /// the config value is now the source of truth, not the URL. Issue #27.
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
        if url_has_userinfo(&self.url) {
            tracing::warn!(
                datasource = %self.name,
                source = source,
                "datasource url carries embedded user:password@ and `{source}` is also set; \
                 config-supplied credentials override the URL's userinfo. \
                 Remove the credentials from the url to silence this warning."
            );
        }
        Ok(inject_userinfo(&self.url, &self.username, &pw))
    }
}

/// True iff a `scheme://user[:pw]@host/...` URL carries a userinfo
/// component. Used to detect the `password_env` + URL-userinfo collision
/// that silently defeats env-var injection.
fn url_has_userinfo(url: &str) -> bool {
    let Some(scheme_end) = url.find("://") else {
        return false;
    };
    let after_scheme = &url[scheme_end + 3..];
    // Only a `@` before the first `/`, `?`, or `#` counts as userinfo —
    // a `@` in the path/query/fragment is not a credential.
    let authority_end = after_scheme
        .find(['/', '?', '#'])
        .unwrap_or(after_scheme.len());
    after_scheme[..authority_end].contains('@')
}

fn inject_userinfo(url: &str, user: &str, pw: &str) -> String {
    // Only touches the URL if it has a `scheme://` prefix. Any existing
    // userinfo in the URL is stripped — the caller supplies the credential
    // that ends up on the wire. See `resolved_url` for the rationale (issue
    // #27) and the operator-facing WARN.
    let Some(scheme_end) = url.find("://") else {
        return url.to_string();
    };
    let (scheme, rest) = url.split_at(scheme_end + 3);
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let (authority, tail) = rest.split_at(authority_end);
    let host_and_port = match authority.rfind('@') {
        Some(at) => &authority[at + 1..],
        None => authority,
    };
    let user_enc = urlencode(user);
    let pw_enc = urlencode(pw);
    format!("{scheme}{user_enc}:{pw_enc}@{host_and_port}{tail}")
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
        // Fleet stronghold §3.1: the default `server.bind` (0.0.0.0:8080)
        // now trips the boot-refuse check. Set `trust_network: true` to
        // certify the delegate-auth posture so this test focuses on the
        // remaining defaults rather than the auth-story check.
        let yaml = r#"
sql_dir: ./sql
security:
  trust_network: true
"#;
        let cfg = Config::from_yaml_str(yaml).unwrap();
        assert_eq!(cfg.server.bind, "0.0.0.0:8080");
        assert_eq!(cfg.server.max_body_bytes, 1_048_576);
        assert!(
            !cfg.allow_datasource_header,
            "header routing must default OFF (R1)"
        );
        assert!(cfg.datasources.is_empty());
        assert!(cfg.datasource_header_allowlist.is_empty());
        assert_eq!(
            cfg.cors.allowed_origins, "",
            "CORS must default to empty (R2)"
        );
    }

    #[test]
    fn allowlist_referencing_unknown_datasource_rejected() {
        let yaml = r#"
sql_dir: ./sql
allow_datasource_header: true
datasource_header_allowlist:
  crm: [ghost]
datasources:
  - name: crm
    url: "sqlite::memory:"
"#;
        let err = Config::from_yaml_str(yaml).unwrap_err();
        assert!(err.to_string().contains("ghost"), "err = {err:?}");
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
        // Loopback bind → skip the fleet §3.1 boot-refuse check.
        let cfg =
            Config::from_yaml_str("sql_dir: ./sql\nserver:\n  bind: \"127.0.0.1:8080\"\n").unwrap();
        assert_eq!(cfg.datasource_for_project("crm"), "crm");
    }

    #[test]
    fn datasource_for_project_uses_map() {
        let yaml = r#"
sql_dir: ./sql
server:
  bind: "127.0.0.1:8080"
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
    fn inject_userinfo_writes_when_absent() {
        assert_eq!(
            inject_userinfo("postgres://host:5432/db", "u", "p"),
            "postgres://u:p@host:5432/db"
        );
    }

    #[test]
    fn inject_userinfo_overrides_existing_userinfo() {
        // Config-supplied credential wins over URL-embedded userinfo.
        // Previously (pre-#27) the URL was returned unchanged, silently
        // defeating `password_env`.
        assert_eq!(
            inject_userinfo("postgres://old:stale@host/db", "u", "p"),
            "postgres://u:p@host/db"
        );
        // Rotation scenario: URL was edited to a new password, but
        // password_env was also set. env-supplied password wins.
        assert_eq!(
            inject_userinfo("postgres://user:oldpass@host:5432/db", "user", "newpass"),
            "postgres://user:newpass@host:5432/db"
        );
    }

    #[test]
    fn inject_userinfo_ignores_at_in_path() {
        // A `@` inside the path is not userinfo; must not be mistaken
        // for authority. sqlite is a `scheme:` (no `//`) form so it
        // takes the no-scheme branch — covered separately.
        assert_eq!(
            inject_userinfo("postgres://host/db?opt=a@b", "u", "p"),
            "postgres://u:p@host/db?opt=a@b"
        );
    }

    #[test]
    fn url_has_userinfo_detects_authority_at() {
        assert!(url_has_userinfo("postgres://u:p@host/db"));
        assert!(url_has_userinfo("postgres://u@host/db"));
        assert!(!url_has_userinfo("postgres://host/db"));
        assert!(!url_has_userinfo("postgres://host/db?opt=a@b"));
        assert!(!url_has_userinfo("sqlite::memory:"));
    }

    #[test]
    fn resolved_url_password_env_beats_url_userinfo() {
        // Regression for issue #27: setting `password_env` on a
        // datasource whose URL already carries user:pw@ must produce a
        // URL that uses the env-supplied password, not the URL's.
        let key = "RESQL_TEST_ISSUE_27_PW";
        std::env::set_var(key, "env-secret");
        let ds = DatasourceConfig {
            name: "t".into(),
            url: "postgres://user:url-secret@host:5432/db".into(),
            username: "user".into(),
            password_env: key.into(),
            password: None,
            max_connections: 1,
            acquire_timeout_seconds: 1,
        };
        let resolved = ds.resolved_url().unwrap();
        std::env::remove_var(key);
        assert_eq!(resolved, "postgres://user:env-secret@host:5432/db");
    }

    #[test]
    fn urlencode_percent_encodes_special_chars() {
        assert_eq!(urlencode("p@ss w/ord!"), "p%40ss%20w%2Ford%21");
        assert_eq!(urlencode("simple"), "simple");
    }

    #[test]
    fn inter_service_token_env_missing_var_rejected_at_boot() {
        // F-RES-3: an operator who wires the config but forgets the env
        // var must fail loud at boot — silent-fallback to "no bearer
        // required" is the pre-fix posture we're closing.
        let key = "RESQL_TEST_TOKEN_MUST_NOT_EXIST";
        std::env::remove_var(key);
        let yaml = format!("sql_dir: ./sql\nsecurity:\n  inter_service_token_env: \"{key}\"\n");
        let err = Config::from_yaml_str(&yaml).unwrap_err();
        assert!(err.to_string().contains(key), "err = {err:?}");
    }

    #[test]
    fn inter_service_token_env_empty_value_rejected_at_boot() {
        let key = "RESQL_TEST_TOKEN_EMPTY";
        std::env::set_var(key, "");
        let yaml = format!("sql_dir: ./sql\nsecurity:\n  inter_service_token_env: \"{key}\"\n");
        let err = Config::from_yaml_str(&yaml).unwrap_err();
        std::env::remove_var(key);
        assert!(err.to_string().contains("empty"), "err = {err:?}");
    }

    #[test]
    fn resolved_inter_service_token_reads_env() {
        let key = "RESQL_TEST_TOKEN_OK";
        std::env::set_var(key, "s3cret");
        let yaml = format!("sql_dir: ./sql\nsecurity:\n  inter_service_token_env: \"{key}\"\n");
        let cfg = Config::from_yaml_str(&yaml).unwrap();
        assert_eq!(
            cfg.resolved_inter_service_token().as_deref(),
            Some("s3cret")
        );
        std::env::remove_var(key);
    }

    #[test]
    fn resolved_inter_service_token_none_when_unset() {
        // Default bind (0.0.0.0:8080) trips the fleet §3.1 refuse-check,
        // so pin the bind to loopback so this test focuses on the
        // resolved-token behaviour and not the boot-refuse posture.
        let cfg =
            Config::from_yaml_str("sql_dir: ./sql\nserver:\n  bind: \"127.0.0.1:8080\"\n").unwrap();
        assert!(cfg.resolved_inter_service_token().is_none());
    }

    #[test]
    fn runtime_posture_refuses_non_loopback_without_auth() {
        // Fleet stronghold §3.1: bind 0.0.0.0:8080 with no bearer +
        // no `trust_network` opt-in is the F-RES-3 shape. Semantic
        // parse succeeds (so a compat-shim reload still works); the
        // runtime-posture check is what refuses to boot.
        let yaml = "sql_dir: ./sql\nserver:\n  bind: \"0.0.0.0:8080\"\n";
        let cfg = Config::from_yaml_str(yaml).unwrap();
        let err = cfg.validate_runtime_posture().unwrap_err();
        let m = err.to_string();
        assert!(m.contains("non-loopback bind"), "err = {m}");
        assert!(m.contains("127.0.0.1"), "err = {m}");
        assert!(m.contains("inter_service_token_env"), "err = {m}");
        assert!(m.contains("trust_network"), "err = {m}");
    }

    #[test]
    fn runtime_posture_accepts_non_loopback_with_bearer_gate() {
        let key = "RESQL_TEST_FLEET_3_1_TOKEN";
        std::env::set_var(key, "secret");
        let yaml = format!(
            "sql_dir: ./sql\nserver:\n  bind: \"0.0.0.0:8080\"\nsecurity:\n  inter_service_token_env: \"{key}\"\n"
        );
        let cfg = Config::from_yaml_str(&yaml).unwrap();
        let ok = cfg.validate_runtime_posture();
        std::env::remove_var(key);
        assert!(ok.is_ok(), "err = {:?}", ok.err());
    }

    #[test]
    fn runtime_posture_accepts_non_loopback_with_trust_network() {
        let yaml =
            "sql_dir: ./sql\nserver:\n  bind: \"0.0.0.0:8080\"\nsecurity:\n  trust_network: true\n";
        let cfg = Config::from_yaml_str(yaml).unwrap();
        assert!(cfg.validate_runtime_posture().is_ok());
    }

    #[test]
    fn runtime_posture_accepts_loopback_without_auth() {
        for yaml in [
            "sql_dir: ./sql\nserver:\n  bind: \"127.0.0.1:8080\"\n",
            "sql_dir: ./sql\nserver:\n  bind: \"[::1]:8080\"\n",
            "sql_dir: ./sql\nserver:\n  bind: \"localhost:8080\"\n",
        ] {
            let cfg = Config::from_yaml_str(yaml).unwrap();
            assert!(cfg.validate_runtime_posture().is_ok(), "yaml = {yaml}");
        }
    }

    #[test]
    fn zero_request_timeout_rejected() {
        let yaml = r#"
sql_dir: ./sql
server:
  request_timeout_seconds: 0
"#;
        let err = Config::from_yaml_str(yaml).unwrap_err();
        assert!(
            err.to_string().contains("request_timeout_seconds"),
            "err = {err:?}"
        );
    }
}
