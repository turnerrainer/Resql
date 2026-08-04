# Rust Resql — Target Surface Enumeration

Generated: 2026-08-04
Branch: `refacto/spec-compliance-v1` (based on `dev` at 763c83f)
Framework: Axum 0.7, sqlx 0.8, serde_yaml_ng 0.10
Verification level: (code-read-verified)

## 1. Config schema (all structs use `#[serde(deny_unknown_fields)]`)

### Top-level `Config` (src/config.rs:7-23)

| Field | Type | Default | Notes |
|---|---|---|---|
| `server` | ServerConfig | default | Nested |
| `sql_dir` | PathBuf | **REQUIRED** — no default | |
| `project_datasource_map` | HashMap<String,String> | `{}` | Project→datasource routing |
| `allow_datasource_header` | bool | `true` | Governs `X-Datasource` header |
| `datasources` | Vec<DatasourceConfig> | `[]` | |
| `cors` | CorsConfig | default | |
| `logging` | LoggingConfig | default | |

### `ServerConfig` (src/config.rs:25-44)

| Field | Type | Default |
|---|---|---|
| `bind` | String | `0.0.0.0:8080` |
| `max_body_bytes` | usize | `1_048_576` (1 MiB) |
| `request_timeout_seconds` | u64 | `30` |

### `DatasourceConfig` (src/config.rs:46-59)

| Field | Type | Default |
|---|---|---|
| `name` | String | required |
| `url` | String | required |
| `username` | String | `""` |
| `password_env` | String | `""` |
| `max_connections` | u32 | `10` |
| `acquire_timeout_seconds` | u64 | `5` |

### `CorsConfig` / `LoggingConfig`

- `cors.allowed_origins`: default `*`
- `logging.level`: default `info,resql=debug`
- `logging.format`: default `text` (also `json`)

### CLI + env

- `-c/--config <PATH>` (default `/app/resql.yaml`)
- `RESQL_CONFIG` env alt
- `RESQL_LOG` env overrides `logging.level`
- Per-datasource: `password_env` field names the env var to read

## 2. HTTP routes (src/server.rs:30-43)

| Method | Path | Handler |
|---|---|---|
| GET | `/health` | health |
| GET | `/healthz` | health |
| GET | `/datasources` | list_datasources |
| GET | `/:project/*tail` | query_get |
| POST | `/:project/*tail` | query_post |
| POST | `/:project/*tail/batch` | (via query_post path recognition) |

**Path extractor:** `Path((project, tail))` — project is first segment; tail is all remaining.
**Header read:** `X-Datasource` (case-insensitive header name), only honored when `allow_datasource_header: true` (silent otherwise).
**Body:** POST expects JSON; empty body treated as `{}`.
**Batch:** detected by `/batch` suffix in tail; body `{"queries":[...]}`; missing `queries` key → 400 MalformedRequest.

## 3. SQL layout (src/loader.rs)

- Root: config `sql_dir` (required, no default).
- Layout: `<sql_dir>/<project>/<GET|POST>/<subpath>.sql` — METHOD dirs case-**sensitive** ("GET"/"POST" only).
- URL: `<METHOD> /<project>/<subpath>` (no `.sql`).
- Endpoint matching (loader.rs:41-44, 247-253): `normalize()` lowercases full `project/path` string → case-**insensitive** endpoint lookup.
- File extension: `.sql` (case-insensitive extension check).
- Non-.sql files silently ignored.

## 4. Named-parameter syntax (src/query.rs)

- Format: `:identifier`, identifier = `[a-zA-Z_][a-zA-Z0-9_]*`.
- **Case-sensitive** matching against JSON body keys (query.rs:80-84).
- Handles: line comments `--`, block comments `/* */`, string literals `'..'` and `".."` with doubled-quote escape, PG cast `::`.
- Missing params → 400 MissingParameter.
- Extra params ignored.

## 5. Response format (src/query.rs)

- Column names: snake_case → camelCase (query.rs:410-426).
- Rows: array of JSON objects.
- Nulls: included as JSON `null`.
- INSERT/UPDATE/DELETE with no result set: `[]`.
- Batch: `[[...], [...], ...]` — parallel to input.
- Postgres types handled: TIMESTAMP (naive string), TIMESTAMPTZ (RFC 3339 with offset), DATE (`YYYY-MM-DD`), TEXT[]/INT[], JSON/JSONB, NUMERIC, and dynamic fallback.
- SQLite: dynamic types, string fallback.

## 6. Error format (src/error.rs)

Body shape: `{"error":"<Kind>","message":"..."}` — matches Java.

| Rust ErrorKind | Class-name string | HTTP status |
|---|---|---|
| QueryNotFound | `ResqlRuntimeException` | 400 |
| UnknownDataSource | `UnknownDataSourceNameException` | 400 |
| MissingParameter | `InvalidDataAccessApiUsageException` | 400 |
| InvalidQuery | `InvalidQueryException` | 400 |
| InvalidDirectory | `InvalidDirectoryException` | 400 |
| SqlExecution | `BadSqlGrammarException` | 400 |
| BodyTooLarge | `PayloadTooLargeException` | **413** |
| MalformedRequest | `MalformedRequestException` | 400 |
| Internal | `InternalError` | **500** |

Message templates broadly match Java (e.g. `Saved query '%s' does not exist`, `Specified dataSourceName name: '%s' is unknown to the service`).

## 7. Boot output (src/main.rs)

- INFO with version, bind, sql_dir, datasource count.
- INFO endpoints/datasources loaded.
- INFO listening addr.
- INFO shutdown.
- `tracing_subscriber` w/ EnvFilter.
- No boot-time WARN for unfamiliar config surface (there's nothing to WARN about because `deny_unknown_fields` forbids anything unfamiliar).

## 8. `/health` and `/healthz` response (src/health.rs)

Shape (camelCase serde rename):
```json
{
  "appName": "resql",
  "version": "0.1.0-alpha.1",
  "appStartTime": 1700000000000,
  "serverTime": 1700000001234,
  "status": "UP"
}
```

**vs Java:** Java has `packagingTime` (Rust missing), Java version format `v{MAJOR}.{MINOR}.{PATCH}` (Rust semver), Java has no `status` field (Rust adds it).

## 9. `/datasources` response (src/server.rs:67-91)

Shape:
```json
[{"name":"users","url":"postgres://user:*****@host/db","driver":"postgres"}]
```

**vs Java:** Java has `jdbcUrl`, `username`, `driverClassName`. Rust: `url` (masked), no `username`, `driver` (simplified).

## 10. Compat gap summary — Java → Rust

| Java surface | Rust state | Compat? |
|---|---|---|
| Config file `application.yml` (Spring convention) | `resql.yaml` (via `-c` or `/app/resql.yaml`) | ❌ File not found by default |
| `sqlms.datasources[]` (Spring prefix) | `datasources[]` (flat, no prefix) | ❌ Field name change |
| `sqlms.saved-queries-dir` | `sql_dir` | ❌ Field name change |
| `sqlms.saved-queries-dir` default `./templates/` | Required (no default) | ❌ No default |
| `datasources[].jdbcUrl` | `datasources[].url` | ❌ Field name change |
| `datasources[].password` | `datasources[].password_env` (env-var indirection) | ❌ Field name + semantic change |
| `datasources[].driverClassName` (required) | Inferred from URL scheme | ❌ Field dropped |
| `headers.contentSecurityPolicy` | Not present | ❌ Dropped |
| `userIPHeaderName` / `userIPLoggingPrefix` / `userIPLoggingMDCkey` | Not present | ❌ Dropped |
| `cors.allowedOrigins` (default `*`) | `cors.allowed_origins` (default `*`) | ❌ Field name change (kebab vs snake) |
| `server.port` (Spring) | `server.bind` (host:port) | ❌ Field name/shape change |
| SQL file layout `{project}/{METHOD}/{name}.sql` | Same layout | ✅ |
| METHOD directory names `GET`/`POST` uppercase | Same | ✅ |
| Named param `:name` | Same syntax | ✅ |
| Named param **case-insensitive** | **Case-sensitive** in Rust | ❌ Behavior change |
| POST body: JSON object of params | Same | ✅ |
| GET query params: single-valued | Same | ✅ |
| Response: `List<Map>` with snake→camel | Same | ✅ |
| Timestamps ISO 8601 with offset | TIMESTAMPTZ ✅, TIMESTAMP (no tz) as naive string ⚠️ minor diff | ⚠️ Partial |
| Batch: `POST /{name}/batch` (name = query, `byk` datasource hardcoded) | `POST /{project}/{name}/batch` | ❌ URL shape change |
| `/healthz` — 5 fields incl. `packagingTime`, version `v{M}.{m}.{p}` | 5 fields (packagingTime missing, status added), semver version | ❌ Shape drift |
| `/datasources` — 4 fields incl. `jdbcUrl`, `username`, `driverClassName` | 3 fields, renamed/simplified | ❌ Shape drift |
| Error body `{"error":"<Class>","message":"..."}` HTTP 400 for all | Same shape, HTTP 400 for most, 413/500 for two Rust-added kinds | ✅ (matches for Java-known kinds) |
| No auth (permitAll) | No auth | ✅ |
| No request-body size cap | 1 MiB cap → 413 | ⚠️ New reject-condition |
| No request timeout | 30s cap → likely 500/408 | ⚠️ New reject-condition |
| Project name in URL routes to "byk" always | Project name routes to actual datasource | ❌ Semantic behavior change |

## 11. Rust-only additions (§8.3 negative-space)

- `/health` alias (new).
- `status: "UP"` field on health (new).
- Body-size cap → 413.
- Request timeout → likely 500.
- `X-Datasource` header (Java may have implicitly had this — unclear).
- `project_datasource_map` config field (new).
- `allow_datasource_header` config toggle (new).
- `logging.format: json` mode (new).
- CLI flag `-c/--config` (Java uses Spring conventions).
- `password_env` indirection instead of plaintext password (new).
- Password/user masking in `/datasources` URL output.
- MySQL/MariaDB/JDBC schemes explicitly rejected.
