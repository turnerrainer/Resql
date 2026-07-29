# DESIGN — Resql-on-Rust

**Status:** Landed with 0.1.0-rc.1.
**Owner:** Rainer Türner.
**Superseded parts:** None yet.

This is the domain-design doc: what Resql-on-Rust must do, why, and the
interfaces we are committed to. Implementation choices are described
just enough to make the "why" of the design legible; consult the source
under `src/` for the mechanical details.

## 1. Purpose

Turn a directory of `.sql` files into a REST API. Callers post JSON, get
JSON back, and never see the database. Operators configure connections
in one YAML file. There is no controller code to write.

## 2. Non-goals

- **Query builder** — Resql is a SQL executor, not a DSL. Consumers write real SQL.
- **ORM** — no schema management, no migrations, no code generation.
- **Multi-tenant billing / rate limiting** — layer that at the ingress.
- **Admin API** — datasource management is config-driven; the runtime has no admin surface (per DEV-REQUIREMENTS §5.3).
- **Cross-datasource joins / transactions** — each request touches exactly one datasource.

## 3. Public interface

### 3.1 Endpoints

| Method | Path | Purpose |
|---|---|---|
| `GET` | `/health` | Liveness + version + timestamps. Also served at `/healthz`. |
| `GET` | `/datasources` | List configured datasources (password masked). |
| `GET` | `/<project>/<name...>` | Execute the SQL at `sql/<project>/GET/<name...>.sql`. Params from query string. |
| `POST` | `/<project>/<name...>` | Execute the SQL at `sql/<project>/POST/<name...>.sql`. Params from JSON body. |
| `POST` | `/<project>/<name...>/batch` | Execute the same POST SQL N times with a list of parameter objects. |

Response shape (success): `[{...row...}, ...]` — always a JSON array,
even for single-row results and DDL (`[]`).

Response shape (error): `{"error": "<ClassName>", "message": "..."}`.

### 3.2 Path → File resolution

The URL segments after `<project>` map 1:1 to the path under
`sql/<project>/<METHOD>/`:

- `POST /crm/users/create` → `sql/crm/POST/users/create.sql`
- `GET /reports/daily/sales-by-region` → `sql/reports/GET/daily/sales-by-region.sql`

Casing is normalised to lowercase in the lookup key — `/CRM/users/CREATE`
is the same endpoint as `/crm/users/create`.

### 3.3 Parameter binding

- Placeholder syntax in SQL: `:name` (identifier chars).
- The parser skips string literals, `--` line comments, `/* */` block comments, and Postgres `::` casts.
- Repeated placeholders bind the same JSON value at each position.
- JSON body keys with no matching placeholder are ignored.
- Placeholder with no matching JSON key → HTTP 400 with `InvalidDataAccessApiUsageException`.

Type mapping (JSON → SQL):

| JSON | Postgres | SQLite |
|---|---|---|
| `null` | NULL | NULL |
| `true`/`false` | BOOLEAN | INTEGER 0/1 |
| integer | BIGINT (i64) | INTEGER |
| float | DOUBLE PRECISION | REAL |
| string | TEXT | TEXT |
| object / array | JSONB (via `sqlx::types::Json`) | TEXT (JSON-serialised) |

Result columns:

- Every column name is renamed from `snake_case` to `camelCase`.
- Type mapping is best-effort; NULL surfaces as JSON `null`.
- Postgres types with explicit handling: TEXT/VARCHAR/CHAR/UUID/BOOL/INT2/INT4/INT8/FLOAT4/FLOAT8/NUMERIC/JSON/JSONB/TIMESTAMP/TIMESTAMPTZ/DATE/TIME + text[]/int[].
- Other Postgres types are read as text.

### 3.4 Datasource resolution

Precedence (highest wins):

1. `X-Datasource: <name>` header — if `allow_datasource_header: true` (default) and non-empty.
2. `project_datasource_map[<project>]` — if present.
3. `<project>` — the URL segment used as-is.

Resolved name is looked up in `datasources[]`. If not found → HTTP 400 with `UnknownDataSourceNameException`.

## 4. Configuration

Single YAML file, no environment-driven overrides beyond `RESQL_CONFIG` (path) and `RESQL_LOG` (log level). Every field has a safe default; only `sql_dir` is required. Unknown fields fail startup — no silent typos.

## 5. Datasource drivers

**Supported:**

- Postgres (`postgres://` or `postgresql://` URL).
- SQLite (`sqlite:` URL, including `sqlite::memory:` and `sqlite:file.db`).

**Not supported (out-of-scope for v0.1):**

- MySQL — no consumer demand yet; adding requires STANDARDS review.
- MSSQL / Oracle — no plan.

## 6. Error handling

All application-level errors return HTTP 400 with a stable `error` field. Two exceptions:

- `413 Payload Too Large` — inbound body exceeded `server.max_body_bytes`.
- `500 Internal Server Error` — unhandled panic reached the top of the stack.

The 400-for-everything shape is inherited from JVM Resql for compatibility. Consumers already switch on the `error` field, not on the HTTP status.

## 7. Fixed vs the JVM original

| Behaviour | JVM Resql | Resql-on-Rust |
|---|---|---|
| Datasource by URL project | ❌ hardcoded `"byk"` | ✅ project → datasource, header override, config map |
| Config validation | Partial (some datasources default silently) | Full: refuses to boot on any misconfig |
| Password storage | Plaintext in `application.yml` (default `"123456"` for keystore) | Env-var references only; startup refuses if unset |
| Request body cap | Uncapped | Default 1 MiB, structured 413 |
| CORS default | Permissive | Same (`*`), but explicit; opt-in restriction via `cors.allowed_origins` |
| Cold start | ~4 s | <100 ms |
| Memory footprint | ~180 MB | ~15 MB |

## 8. Implementation shape (informational)

- HTTP: **axum 0.7**.
- Async runtime: **tokio 1.40**.
- SQL: **sqlx 0.8** with `postgres` + `sqlite` + `rustls` features.
- YAML: **serde_yaml_ng 0.10** (the maintained fork).
- Errors: **thiserror 2.0** in library, **anyhow 1.0** at binary entrypoint.
- Logging: **tracing** + **tracing_subscriber**.
- Container: `debian:bookworm-slim` runtime, `rust:1.88-slim` builder, non-root UID 1000, tini.

## 9. Testing strategy

Per DEV-REQUIREMENTS §3:

- **Unit tests** live inline (`#[cfg(test)] mod tests`) in every module.
- **Integration tests** live in `tests/` and exercise the full axum router with real SQLite pools (no mocks).
- **No line-coverage chasing**. Tests exist to catch known bugs and to seal seams.
- **Postgres integration tests** are open work (task 006) — CI runs SQLite-backed integration tests only right now.

Test counts (0.1.0-rc.1): 49 unit + 28 integration = 77 passing.

## 10. Deviations from DEV-REQUIREMENTS

None in scope for 0.1.0-rc.1.
