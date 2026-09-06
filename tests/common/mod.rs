//! Shared fixtures for integration tests. Every test constructs its own
//! TempDir and SQLite database so tests can run concurrently.
//!
//! Each `tests/*.rs` file compiles as its own binary and only sees the
//! subset of helpers it actually calls — `#[allow(dead_code)]` keeps clippy
//! quiet about helpers other binaries use.
#![allow(dead_code)]

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use axum::Router;
use resql::config::{Config, DatasourceConfig};
use resql::db::{DatasourceRegistry, Pool};
use resql::health::StartTime;
use resql::loader;
use resql::server::{self, AppState};
use serde_json::Value;
use sqlx::postgres::PgPoolOptions;
use sqlx::sqlite::SqlitePoolOptions;
use tempfile::TempDir;

pub struct TestApp {
    pub router: Router,
    pub sql_root: PathBuf,
    _tmp: TempDir,
}

pub struct TestAppBuilder {
    files: Vec<(String, String)>,
    seed_sql: Vec<String>,
    datasource_names: Vec<String>,
    /// Extra datasources hitting a real Postgres URL. Populated by
    /// `with_postgres_datasource(name, url)`. Coexists with SQLite ones.
    pg_datasources: Vec<(String, String)>,
    project_map: Vec<(String, String)>,
    allow_header: bool,
    max_body: usize,
}

impl TestAppBuilder {
    pub fn new() -> Self {
        Self {
            files: Vec::new(),
            seed_sql: Vec::new(),
            datasource_names: vec!["demo".into()],
            pg_datasources: Vec::new(),
            project_map: Vec::new(),
            allow_header: true,
            max_body: 1_048_576,
        }
    }

    /// Add a Postgres-backed datasource by URL. Coexists with any
    /// SQLite datasources set up via `with_datasources`.
    pub fn with_postgres_datasource(mut self, name: &str, url: &str) -> Self {
        self.pg_datasources.push((name.into(), url.into()));
        self
    }

    /// Skip SQLite entirely. Use when the test is Postgres-only.
    pub fn no_sqlite_datasources(mut self) -> Self {
        self.datasource_names.clear();
        self
    }

    pub fn with_sql(mut self, rel: &str, body: &str) -> Self {
        self.files.push((rel.into(), body.into()));
        self
    }

    pub fn with_seed(mut self, sql: &str) -> Self {
        self.seed_sql.push(sql.into());
        self
    }

    pub fn with_datasources(mut self, names: &[&str]) -> Self {
        self.datasource_names = names.iter().map(|s| (*s).to_string()).collect();
        self
    }

    pub fn map_project(mut self, project: &str, datasource: &str) -> Self {
        self.project_map.push((project.into(), datasource.into()));
        self
    }

    pub fn allow_header(mut self, allow: bool) -> Self {
        self.allow_header = allow;
        self
    }

    pub fn max_body(mut self, n: usize) -> Self {
        self.max_body = n;
        self
    }

    pub async fn build(self) -> TestApp {
        let tmp = TempDir::new().unwrap();
        let sql_root = tmp.path().join("sql");
        for (rel, body) in &self.files {
            let path = sql_root.join(rel);
            fs::create_dir_all(path.parent().unwrap()).unwrap();
            fs::write(path, materialise_sql(body)).unwrap();
        }
        if self.files.is_empty() {
            fs::create_dir_all(&sql_root).unwrap();
        }
        let index = loader::load_dir(&sql_root).unwrap();

        let mut registry = DatasourceRegistry::default();
        let mut cfg_datasources = Vec::new();
        for name in &self.datasource_names {
            let pool_inner = SqlitePoolOptions::new()
                .max_connections(1)
                .connect("sqlite::memory:")
                .await
                .unwrap();
            for s in &self.seed_sql {
                sqlx::query(s).execute(&pool_inner).await.unwrap();
            }
            registry.insert(name.clone(), Pool::Sqlite(pool_inner));
            cfg_datasources.push(DatasourceConfig {
                name: name.clone(),
                url: "sqlite::memory:".into(),
                username: "".into(),
                password_env: "".into(),
                password: None,
                max_connections: 1,
                acquire_timeout_seconds: 1,
            });
        }
        for (name, url) in &self.pg_datasources {
            // Pool pinned to one connection so cached-prepared-statement
            // reuse is deterministic across sequential requests to the
            // same endpoint. The suite is not concurrent, and a single-
            // connection pool guarantees both calls in tests like
            // `pg_number_after_null_on_same_cached_statement_does_not_corrupt`
            // land on the same physical connection — reproducing
            // bind-type stability bugs that only surface when a
            // subsequent call hits the same cached `Parse` as an earlier
            // one.
            let pool_inner = PgPoolOptions::new()
                .max_connections(1)
                .connect(url)
                .await
                .unwrap_or_else(|e| {
                    panic!("cannot connect Postgres datasource '{name}' at {url}: {e}")
                });
            registry.insert(name.clone(), Pool::Postgres(pool_inner));
            cfg_datasources.push(DatasourceConfig {
                name: name.clone(),
                url: url.clone(),
                username: "".into(),
                password_env: "".into(),
                password: None,
                max_connections: 1,
                acquire_timeout_seconds: 5,
            });
        }

        let mut project_map = std::collections::HashMap::new();
        for (k, v) in self.project_map {
            project_map.insert(k, v);
        }

        // Build a permissive allowlist for tests: every project the SQL
        // tree references may override to any registered datasource.
        // Production defaults to empty (R1), but the test suite exercises
        // routing behaviour; each `x_datasource_header_*` test that
        // needs a stricter posture sets `allow_header(false)` or writes
        // its own allowlist explicitly.
        let mut header_allowlist: std::collections::HashMap<String, Vec<String>> =
            std::collections::HashMap::new();
        if self.allow_header {
            let mut all_ds: Vec<String> = cfg_datasources.iter().map(|d| d.name.clone()).collect();
            all_ds.sort();
            all_ds.dedup();
            let mut projects: std::collections::BTreeSet<String> =
                std::collections::BTreeSet::new();
            for (rel, _) in &self.files {
                if let Some(first) = rel.split('/').next() {
                    projects.insert(first.to_string());
                }
            }
            for proj in project_map.keys() {
                projects.insert(proj.clone());
            }
            for proj in projects {
                header_allowlist.insert(proj, all_ds.clone());
            }
        }

        let config = Config {
            server: resql::config::ServerConfig {
                bind: "127.0.0.1:0".into(),
                max_body_bytes: self.max_body,
                request_timeout_seconds: 30,
            },
            sql_dir: sql_root.clone(),
            project_datasource_map: project_map,
            allow_datasource_header: self.allow_header,
            datasource_header_allowlist: header_allowlist,
            default_datasource: None,
            datasources: cfg_datasources,
            cors: resql::config::CorsConfig::default(),
            logging: resql::config::LoggingConfig::default(),
            openapi: resql::config::OpenApiConfig::default(),
            compat_diagnostics: Vec::new(),
        };

        let spec = resql::openapi::build_spec(&index, &config.openapi);
        let state = AppState {
            config: Arc::new(config),
            index: Arc::new(index),
            registry: Arc::new(registry),
            start: StartTime::now(),
            openapi: Arc::new(spec),
        };

        let router = server::router(state);
        TestApp {
            router,
            sql_root,
            _tmp: tmp,
        }
    }
}

impl TestApp {
    pub async fn request(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, Value) {
        let (status, _resp_headers, body) = self.request_full(method, path, body, headers).await;
        (status, body)
    }

    /// Same as `request` but also returns the response headers as a
    /// `HeaderMap`. Use when a test needs to assert on headers the
    /// middleware set (trace id, CORS, etc.).
    pub async fn request_full(
        &self,
        method: &str,
        path: &str,
        body: Option<&str>,
        headers: &[(&str, &str)],
    ) -> (u16, axum::http::HeaderMap, Value) {
        use axum::body::Body;
        use axum::http::{Request, StatusCode};
        use http_body_util::BodyExt;
        use tower::ServiceExt;

        let mut req = Request::builder().method(method).uri(path);
        for (k, v) in headers {
            req = req.header(*k, *v);
        }
        let req = if let Some(b) = body {
            req.header("content-type", "application/json")
                .body(Body::from(b.to_string()))
                .unwrap()
        } else {
            req.body(Body::empty()).unwrap()
        };
        let resp = self.router.clone().oneshot(req).await.unwrap();
        let status: StatusCode = resp.status();
        let resp_headers = resp.headers().clone();
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status.as_u16(), resp_headers, json)
    }

    pub fn sql_root(&self) -> &Path {
        &self.sql_root
    }
}

/// If the test author already wrote a `/* … */` declaration block,
/// keep the body verbatim. Otherwise, prepend a permissive declaration
/// inferred from the `:name` occurrences in the SQL — every referenced
/// param becomes `{ type: string, required: false }`. This lets the
/// existing integration suites keep short, expressive fixtures while
/// the production loader still enforces the mandatory declaration
/// rule (task 008).
fn materialise_sql(body: &str) -> String {
    // Detect an already-present block-comment declaration.
    let trimmed = body.trim_start();
    if trimmed.starts_with("/*") {
        return body.to_string();
    }
    let names = collect_named_params(body);
    let mut out = String::from("/*\nparams:\n");
    if names.is_empty() {
        out.push_str("  {}\n");
    } else {
        for n in &names {
            out.push_str(&format!("  {n}: {{ type: string, required: false }}\n"));
        }
    }
    out.push_str("*/\n");
    out.push_str(body);
    out
}

/// Very small `:name` scanner mirroring `query::rewrite_named_params`'s
/// state machine. Duplicates avoided; order preserved.
fn collect_named_params(sql: &str) -> Vec<String> {
    let bytes = sql.as_bytes();
    let n = bytes.len();
    let mut i = 0usize;
    let mut out: Vec<String> = Vec::new();
    while i < n {
        let c = bytes[i] as char;
        if c == '-' && i + 1 < n && bytes[i + 1] == b'-' {
            while i < n && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if c == '/' && i + 1 < n && bytes[i + 1] == b'*' {
            i += 2;
            while i + 1 < n && !(bytes[i] == b'*' && bytes[i + 1] == b'/') {
                i += 1;
            }
            if i + 1 < n {
                i += 2;
            }
            continue;
        }
        if c == '\'' || c == '"' {
            let quote = c;
            i += 1;
            while i < n {
                let ch = bytes[i] as char;
                i += 1;
                if ch == quote {
                    if i < n && bytes[i] == quote as u8 {
                        i += 1;
                        continue;
                    }
                    break;
                }
            }
            continue;
        }
        if c == ':' && i + 1 < n && bytes[i + 1] == b':' {
            i += 2;
            continue;
        }
        if c == ':' && i + 1 < n {
            let nx = bytes[i + 1] as char;
            if nx.is_ascii_alphabetic() || nx == '_' {
                let start = i + 1;
                let mut end = start;
                while end < n {
                    let ch = bytes[end] as char;
                    if ch.is_ascii_alphanumeric() || ch == '_' {
                        end += 1;
                    } else {
                        break;
                    }
                }
                let name = std::str::from_utf8(&bytes[start..end]).unwrap().to_string();
                if !out.contains(&name) {
                    out.push(name);
                }
                i = end;
                continue;
            }
        }
        i += 1;
    }
    out
}
