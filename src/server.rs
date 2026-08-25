use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Path, Query, Request, State};
use axum::http::{HeaderMap, HeaderName, HeaderValue};
use axum::middleware::{self, Next};
use axum::response::Response;
use axum::routing::get;
use axum::{Json, Router};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use tower_http::cors::{Any, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tracing::Instrument;

use crate::config::{Config, LoggingConfig};
use crate::db::{DatasourceRegistry, SharedRegistry};
use crate::error::ResqlError;
use crate::health::{self, StartTime};
use crate::loader::{HttpMethod, QueryIndex};
use crate::logging::{generate_traceparent, sanitize_log_value, trace_id_from_traceparent};
use crate::openapi;
use crate::query;

const DATASOURCE_HEADER: &str = "x-datasource";

#[derive(Clone)]
pub struct AppState {
    pub config: Arc<Config>,
    pub index: Arc<QueryIndex>,
    pub registry: SharedRegistry,
    pub start: StartTime,
    /// OpenAPI 3.1 spec derived from the loaded QueryIndex at boot.
    /// Cached because the declaration set is fixed for the process
    /// lifetime and building it per request would be wasteful.
    pub openapi: Arc<Value>,
}

pub fn router(state: AppState) -> Router {
    let cors = build_cors(&state.config.cors.allowed_origins);
    let body_limit = state.config.server.max_body_bytes;
    let logging_for_layer = state.config.logging.clone();

    Router::new()
        .route("/health", get(health_handler))
        .route("/healthz", get(health_handler))
        .route("/datasources", get(list_datasources))
        .route("/openapi.json", get(openapi_handler))
        .route("/:project/*tail", get(query_get).post(query_post))
        .layer(cors)
        .layer(RequestBodyLimitLayer::new(body_limit))
        .layer(middleware::from_fn(move |req, next| {
            let cfg = logging_for_layer.clone();
            request_observability(cfg, req, next)
        }))
        .with_state(state)
}

const TRACEPARENT_HEADER: &str = "traceparent";
const TRACE_ID_HEADER: &str = "x-trace-id";
const HEALTHZ_PATH: &str = "/healthz";
const HEALTH_PATH: &str = "/health";

/// Per-request middleware — the single source of Resql's operational
/// log surface. Responsibilities:
///
/// 1. Adopt any inbound W3C `traceparent`, or synthesise one so every
///    request-scoped log line and the response carry a stable id.
/// 2. Open a request span with OpenTelemetry HTTP semantic-convention
///    fields (`http.request.method`, `http.route`, `client.address`,
///    `resql.project`, `trace_id`) that every downstream `info!` /
///    `warn!` / `error!` inherits.
/// 3. After the handler completes: emit a single INFO access-log line
///    with `http.response.status_code` + `duration_ms` (only when
///    `logging.access_log` is on), and echo the trace id back as
///    `X-Trace-Id` so callers can correlate.
///
/// Health probes are excluded from the access log — they'd bury the
/// real traffic.
async fn request_observability(cfg: LoggingConfig, mut req: Request, next: Next) -> Response {
    let start = Instant::now();
    let method_str = req.method().as_str().to_string();
    let uri_path = req.uri().path().to_string();
    let project_str = uri_path
        .split('/')
        .find(|s| !s.is_empty())
        .unwrap_or("")
        .to_string();

    // Adopt or generate traceparent; inject back so any future
    // header-reading code sees the resolved value.
    let (traceparent_value, needed_injection) = match req
        .headers()
        .get(TRACEPARENT_HEADER)
        .and_then(|v| v.to_str().ok())
    {
        Some(existing) => (existing.to_string(), false),
        None => (generate_traceparent(), true),
    };
    if needed_injection {
        if let Ok(hv) = HeaderValue::try_from(traceparent_value.as_str()) {
            req.headers_mut()
                .insert(HeaderName::from_static(TRACEPARENT_HEADER), hv);
        }
    }
    let trace_id_str = trace_id_from_traceparent(&traceparent_value)
        .unwrap_or("")
        .to_string();

    let request_span = tracing::info_span!(
        "http_request",
        http.request.method = %method_str,
        http.route = %sanitize_log_value(&uri_path),
        resql.project = %project_str,
        trace_id = %trace_id_str,
    );
    let span_for_log = request_span.clone();

    let mut response = next.run(req).instrument(request_span).await;

    // Echo trace id back to caller so they can correlate their logs.
    if !trace_id_str.is_empty() {
        if let Ok(hv) = HeaderValue::try_from(trace_id_str.as_str()) {
            response
                .headers_mut()
                .insert(HeaderName::from_static(TRACE_ID_HEADER), hv);
        }
    }

    // Skip access log for health probes — they'd drown real traffic.
    let is_health = uri_path == HEALTH_PATH || uri_path == HEALTHZ_PATH;
    if cfg.access_log && !is_health {
        let status = response.status().as_u16();
        let duration_ms = start.elapsed().as_secs_f64() * 1000.0;
        let _guard = span_for_log.enter();
        // 5xx surfaces at WARN — it's operational noise if it's routine;
        // an alert-worthy signal if it isn't. 4xx and 2xx are INFO.
        if status >= 500 {
            tracing::warn!(
                http.request.method = %method_str,
                http.route = %sanitize_log_value(&uri_path),
                http.response.status_code = status,
                duration_ms,
                resql.project = %project_str,
                trace_id = %trace_id_str,
                "http request failed"
            );
        } else {
            tracing::info!(
                http.request.method = %method_str,
                http.route = %sanitize_log_value(&uri_path),
                http.response.status_code = status,
                duration_ms,
                resql.project = %project_str,
                trace_id = %trace_id_str,
                "http request completed"
            );
        }
    }

    response
}

async fn openapi_handler(State(state): State<AppState>) -> Json<Value> {
    Json((*state.openapi).clone())
}

fn build_cors(allowed: &str) -> CorsLayer {
    let base = CorsLayer::new().allow_methods(Any).allow_headers(Any);
    if allowed.trim() == "*" {
        return base.allow_origin(Any);
    }
    let origins: Vec<axum::http::HeaderValue> = allowed
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .filter_map(|s| axum::http::HeaderValue::from_str(s).ok())
        .collect();
    if origins.is_empty() {
        base.allow_origin(Any)
    } else {
        base.allow_origin(origins)
    }
}

async fn health_handler(State(state): State<AppState>) -> Json<health::HealthResponse> {
    Json(health::build(state.start))
}

/// `/datasources` response entry. Shape mirrors Java's
/// `DataSourceConfigProperties` (name, jdbcUrl, username, driverClassName)
/// with the password field stripped (matches Java's `@JsonIgnore` on
/// `password`). Field names are the Java-canonical camelCase forms so
/// existing dashboards / grep patterns keep working.
#[derive(Serialize)]
struct DatasourceView {
    name: String,
    #[serde(rename = "jdbcUrl")]
    jdbc_url: String,
    username: String,
    #[serde(rename = "driverClassName")]
    driver_class_name: &'static str,
}

async fn list_datasources(State(state): State<AppState>) -> Json<Vec<DatasourceView>> {
    let mut out = Vec::new();
    for name in state.registry.names() {
        let ds_cfg = state.config.datasources.iter().find(|d| d.name == name);
        let driver_class_name = match state.registry.get(&name) {
            Some(crate::db::Pool::Postgres(_)) => "org.postgresql.Driver",
            Some(crate::db::Pool::Sqlite(_)) => "org.sqlite.JDBC",
            None => "",
        };
        let jdbc_url = ds_cfg.map(|d| mask_password(&d.url)).unwrap_or_default();
        let username = ds_cfg.map(|d| d.username.clone()).unwrap_or_default();
        out.push(DatasourceView {
            name,
            jdbc_url,
            username,
            driver_class_name,
        });
    }
    Json(out)
}

fn mask_password(url: &str) -> String {
    if let Some(scheme_end) = url.find("://") {
        let (scheme, rest) = url.split_at(scheme_end + 3);
        if let Some(at) = rest.find('@') {
            let (userinfo, host) = rest.split_at(at);
            if let Some(colon) = userinfo.find(':') {
                let (user, _) = userinfo.split_at(colon);
                return format!("{scheme}{user}:*****{host}");
            }
        }
    }
    url.to_string()
}

async fn query_get(
    State(state): State<AppState>,
    Path((project, tail)): Path<(String, String)>,
    Query(params): Query<Map<String, Value>>,
    headers: HeaderMap,
) -> Result<Json<Value>, ResqlError> {
    dispatch(
        state,
        HttpMethod::Get,
        project,
        tail,
        headers,
        Value::Object(params),
    )
    .await
}

async fn query_post(
    State(state): State<AppState>,
    Path((project, tail)): Path<(String, String)>,
    headers: HeaderMap,
    body: axum::body::Bytes,
) -> Result<Json<Value>, ResqlError> {
    let body_value: Value = if body.is_empty() {
        Value::Object(Map::new())
    } else {
        serde_json::from_slice(&body)
            .map_err(|e| ResqlError::MalformedRequest(format!("invalid JSON body: {e}")))?
    };
    if let Some(stripped) = tail.strip_suffix("/batch") {
        return dispatch_batch(state, project, stripped.to_string(), headers, body_value).await;
    }
    dispatch(state, HttpMethod::Post, project, tail, headers, body_value).await
}

async fn dispatch(
    state: AppState,
    method: HttpMethod,
    project: String,
    tail: String,
    headers: HeaderMap,
    body: Value,
) -> Result<Json<Value>, ResqlError> {
    // Normalise project casing early so query lookup and datasource lookup
    // agree — Java's SavedQuery.getKey lowercased both segments.
    let project_key = project.to_ascii_lowercase();
    let saved = state
        .index
        .get(method, &project_key, &tail)
        .ok_or_else(|| ResqlError::QueryNotFound(format!("/{project}/{tail}")))?;
    let ds_name = choose_datasource(&state.config, &project_key, &headers);
    let pool = state
        .registry
        .get(&ds_name)
        .ok_or_else(|| ResqlError::UnknownDataSource(ds_name.clone()))?;
    let rows = if saved.transactional {
        query::execute_transactional(pool, &saved.sql, &saved.declaration, &body).await?
    } else {
        query::execute(pool, &saved.sql, &saved.declaration, &body).await?
    };
    Ok(Json(json!(rows)))
}

async fn dispatch_batch(
    state: AppState,
    project: String,
    tail: String,
    headers: HeaderMap,
    body: Value,
) -> Result<Json<Value>, ResqlError> {
    let project_key = project.to_ascii_lowercase();
    let saved = state
        .index
        .get(HttpMethod::Post, &project_key, &tail)
        .ok_or_else(|| ResqlError::QueryNotFound(format!("/{project}/{tail}")))?;
    let ds_name = choose_datasource(&state.config, &project_key, &headers);
    let pool = state
        .registry
        .get(&ds_name)
        .ok_or_else(|| ResqlError::UnknownDataSource(ds_name.clone()))?;

    #[derive(Deserialize)]
    struct BatchBody {
        queries: Vec<Value>,
    }
    let batch: BatchBody = serde_json::from_value(body).map_err(|e| {
        ResqlError::MalformedRequest(format!("batch body must be {{queries: [...]}}: {e}"))
    })?;

    let results = query::execute_batch(pool, &saved.sql, &saved.declaration, batch.queries).await?;
    let all: Vec<Value> = results.into_iter().map(|rows| json!(rows)).collect();
    Ok(Json(Value::Array(all)))
}

fn choose_datasource(cfg: &Config, project: &str, headers: &HeaderMap) -> String {
    if cfg.allow_datasource_header {
        if let Some(v) = headers.get(DATASOURCE_HEADER).and_then(|h| h.to_str().ok()) {
            let trimmed = v.trim();
            if !trimmed.is_empty() {
                return trimmed.to_string();
            }
        }
    }
    cfg.datasource_for_project(project)
}

/// Convenience: build a full AppState from a Config, connecting all
/// datasources and loading the SQL directory.
pub async fn init(config: Config) -> Result<AppState, ResqlError> {
    let index = crate::loader::load_dir(&config.sql_dir)?;
    let registry = DatasourceRegistry::connect_all(&config.datasources).await?;
    let spec = openapi::build_spec(&index, &config.openapi);
    Ok(AppState {
        config: Arc::new(config),
        index: Arc::new(index),
        registry: Arc::new(registry),
        start: StartTime::now(),
        openapi: Arc::new(spec),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;

    #[test]
    fn mask_password_hides_secret() {
        assert_eq!(
            mask_password("postgres://user:secret@host:5432/db"),
            "postgres://user:*****@host:5432/db"
        );
    }

    #[test]
    fn mask_password_leaves_url_without_userinfo_alone() {
        assert_eq!(mask_password("sqlite::memory:"), "sqlite::memory:");
    }

    #[test]
    fn choose_datasource_defaults_to_project_name() {
        let cfg = Config::from_yaml_str("sql_dir: ./sql\n").unwrap();
        let headers = HeaderMap::new();
        assert_eq!(choose_datasource(&cfg, "crm", &headers), "crm");
    }

    #[test]
    fn choose_datasource_respects_header_when_allowed() {
        let cfg = Config::from_yaml_str("sql_dir: ./sql\nallow_datasource_header: true\n").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(DATASOURCE_HEADER, HeaderValue::from_static("other"));
        assert_eq!(choose_datasource(&cfg, "crm", &headers), "other");
    }

    #[test]
    fn choose_datasource_ignores_header_when_disabled() {
        let cfg =
            Config::from_yaml_str("sql_dir: ./sql\nallow_datasource_header: false\n").unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(DATASOURCE_HEADER, HeaderValue::from_static("other"));
        assert_eq!(choose_datasource(&cfg, "crm", &headers), "crm");
    }

    #[test]
    fn choose_datasource_map_takes_precedence_over_default() {
        let yaml = r#"
sql_dir: ./sql
project_datasource_map:
  crm: db1
datasources:
  - name: db1
    url: "sqlite::memory:"
"#;
        let cfg = Config::from_yaml_str(yaml).unwrap();
        let headers = HeaderMap::new();
        assert_eq!(choose_datasource(&cfg, "crm", &headers), "db1");
    }

    #[test]
    fn choose_datasource_header_overrides_even_a_mapped_project() {
        let yaml = r#"
sql_dir: ./sql
project_datasource_map:
  crm: db1
datasources:
  - name: db1
    url: "sqlite::memory:"
"#;
        let cfg = Config::from_yaml_str(yaml).unwrap();
        let mut headers = HeaderMap::new();
        headers.insert(DATASOURCE_HEADER, HeaderValue::from_static("db2"));
        assert_eq!(choose_datasource(&cfg, "crm", &headers), "db2");
    }
}
