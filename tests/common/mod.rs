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
use resql_on_rust::config::{Config, DatasourceConfig};
use resql_on_rust::db::{DatasourceRegistry, Pool};
use resql_on_rust::health::StartTime;
use resql_on_rust::loader;
use resql_on_rust::server::{self, AppState};
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
            fs::write(path, body).unwrap();
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
                max_connections: 1,
                acquire_timeout_seconds: 1,
            });
        }
        for (name, url) in &self.pg_datasources {
            let pool_inner = PgPoolOptions::new()
                .max_connections(4)
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
                max_connections: 4,
                acquire_timeout_seconds: 5,
            });
        }

        let mut project_map = std::collections::HashMap::new();
        for (k, v) in self.project_map {
            project_map.insert(k, v);
        }

        let config = Config {
            server: resql_on_rust::config::ServerConfig {
                bind: "127.0.0.1:0".into(),
                max_body_bytes: self.max_body,
                request_timeout_seconds: 30,
            },
            sql_dir: sql_root.clone(),
            project_datasource_map: project_map,
            allow_datasource_header: self.allow_header,
            datasources: cfg_datasources,
            cors: resql_on_rust::config::CorsConfig::default(),
            logging: resql_on_rust::config::LoggingConfig::default(),
        };

        let state = AppState {
            config: Arc::new(config),
            index: Arc::new(index),
            registry: Arc::new(registry),
            start: StartTime::now(),
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
        let bytes = resp.into_body().collect().await.unwrap().to_bytes();
        let json: Value = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes)
                .unwrap_or_else(|_| Value::String(String::from_utf8_lossy(&bytes).into_owned()))
        };
        (status.as_u16(), json)
    }

    pub fn sql_root(&self) -> &Path {
        &self.sql_root
    }
}
