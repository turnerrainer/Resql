# Changelog

All notable changes to this project will be documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Fixed
- **Binding a JSON number after a JSON `null` on the same cached statement corrupted the connection.** `bind_pg` bound `Value::Null` as an untyped/text-ish `Option<String>::None`, but bound `Value::Number` natively (`i64`/`f64`). sqlx caches prepared statements per SQL text on a connection: the first `Parse` of a statement fixes each `$N` placeholder's type, and later executions against that same cached statement reuse that fixed type regardless of what Rust type is bound. An optional numeric parameter that is `null` on one call and a real number on a later call to the same endpoint -- on the same connection -- would send raw binary `i64`/`f64` bytes into a slot Postgres still expected as text, surfacing as `invalid byte sequence for encoding "UTF8": 0x00` and failing the request with a generic `BadSqlGrammarException`. Fixed by binding all JSON numbers as their string representation, matching how `Value::Null` is already bound, so the Rust type stays constant across calls regardless of the JSON value's shape; SQL that needs an actual numeric type continues to work via Postgres's implicit/assignment casts or an explicit `::INTEGER`/`::BIGINT`/`::NUMERIC` cast, as already used throughout this codebase. Added a regression test (`pg_number_after_null_on_same_cached_statement_does_not_corrupt`) and a companion test documenting the supported explicit-cast INSERT shape (`pg_insert_number_into_integer_column_with_explicit_cast`). The Postgres test pool is now pinned to a single connection so this reproduces deterministically rather than depending on which pooled connection a request happens to get.

## [0.1.0-alpha.4] - 2026-08-26

### Added — operator-grade logging (parity with Ruuter-on-Rust)
- **Per-request access log** — one INFO line per completed request with OpenTelemetry HTTP semantic-convention fields: `http.request.method`, `http.route`, `http.response.status_code`, `duration_ms`, `resql.project`, `trace_id`. Toggle via `logging.access_log` (on by default).
- **W3C `traceparent` propagation** — adopted verbatim from the caller, generated server-side when absent. Every request-scoped log line inherits the same 32-hex `trace_id`; the id is echoed back to callers via `X-Trace-Id` on every response (including 4xx/5xx). Correlate client and server logs without extra headers.
- **Structured error logs** — every `ResqlError` returned as HTTP 400+ now emits a WARN (or ERROR for 5xx) line with `error.kind` (Java-canonical exception name) alongside the message. Same `trace_id` as the request span.
- **CRLF-safe log values** — `logging::sanitize_log_value` runs on user-controlled fields before they hit a log line. Blocks log-line splicing via header or body payload.
- **Body redaction toolkit** — `logging::redact::redact_json` / `redact_headers` replace configured field names with `"[REDACTED]"` (case-insensitive, at any depth). Defaults cover `password`, `token`, `authorization`, `api_key`, etc. New config keys: `logging.redact_body_fields`, `logging.max_body_bytes`, `logging.print_stack_trace`.
- **Env-var overrides** — `RESQL_LOG_FORMAT=text|json` overrides `logging.format` at runtime so an operator can flip a running container without editing config. `RESQL_LOG` continues to override the level directive.
- **JSON output** — the JSON layer now emits current-span context so downstream log stores see the request's `trace_id` / `resql.project` on every event.
- Health probes (`/health`, `/healthz`) are excluded from the access log.
- New chapter: `book/src/logging.md` — field vocabulary, config reference, redaction semantics, correlation recipes.

## [0.1.0-alpha.3] - 2026-08-26

### Added
- **Closed-set input validation via `enum:` on declared params.** Any `DeclaredParam` can carry a JSON-Schema-style `enum: [...]` list; values outside the set are rejected at the request boundary with 400 `InvalidParameterValueException` before any SQL binding. Enum entries are validated at boot against the declared `type`; `default:` (when set) must be in the enum or `null`; empty lists are rejected at boot. The set is echoed in the OpenAPI spec as JSON Schema's `enum` keyword so code-gen clients see the true type. Closes the input-side security half of [Resql#3](https://github.com/turnerrainer/Resql/issues/3) — no SQL is rewritten; the DB never sees an out-of-set value.

### Fixed
- **Timestamp serialisation is now ISO 8601 / RFC 3339 by default.** Postgres `TIMESTAMP` columns now render as `2026-01-01T10:20:30` (previously `2026-01-01 10:20:30` — space separator, neither ISO 8601 nor RFC 3339); `TIMESTAMPTZ` columns at UTC render as `2026-01-01T10:20:30Z` (previously `2026-01-01T10:20:30+00:00`). Matches Jackson / JVM Resql defaults. Fixes [Resql#3](https://github.com/turnerrainer/Resql/issues/3).

### Added — mandatory declaration section + OpenAPI 3.1 (task 008)

- **Every `.sql` file now opens with a `/* … */` YAML declaration block** naming its parameters, their types, whether each is required, and (optionally) the returned row shape. The block body is plain YAML — no per-line prefix, so authors can paste YAML from any editor. Boot refuses any file without a declaration, any declaration that fails to cover every `:name` in the SQL, and any orphan declared params. See `book/src/declarations.md`.
- **Optional parameters may be omitted from requests** — the SQL sees SQL NULL for the missing placeholder (or a declared `default:`). Fixes [Resql#4](https://github.com/turnerrainer/Resql/issues/4): callers no longer have to send explicit `null` for every optional filter on every request.
- **Type-safe request validation.** Requests now hit three declaration-driven boundaries before touching the DB: unknown key → 400 `UnknownParameterException`; wrong type → 400 `InvalidParameterTypeException`; required missing → existing 400 `InvalidDataAccessApiUsageException`. GET query-string values are coerced to the declared type at the request boundary.
- **OpenAPI 3.1 spec exposed at `/openapi.json`.** Generated at boot from every declaration; paths and operations are alpha-sorted so a diff on the file is meaningful across restarts. POST endpoints get an auto-emitted `/…/batch` variant. Customise `info` and `servers` via a new optional `openapi:` block in `resql.yaml`.
- **Type set:** `string`, `integer`, `number`, `boolean`, `array`, `object`, `date`, `datetime`, `uuid`. Semantic types (`date`, `datetime`, `uuid`) emit as `string` with the standard OpenAPI `format`.
- New `book/src/declarations.md` chapter; `DIVERGENCES.md` entries DIV-019 through DIV-021 documenting the intentional break from JVM Resql's permissive request handling.

### Changed

- **Breaking.** All existing `.sql` files require a declaration to load. Minimum viable declaration for a file using `:a` and `:b`: `/*\nparams:\n  a: { type: string }\n  b: { type: string }\n*/`. Extras (name matches, default values, returns schema) are recommended.
- `query::execute`, `query::execute_transactional`, and `query::execute_batch` now take an extra `&Declaration` argument; the runtime validates every request map against it before binding.

## [0.1.0-alpha.2] - 2026-08-05

### Added — batch atomicity and array binding (task 007)
- **`/batch` is now atomic.** The full batch runs inside one database transaction; any failure rolls the whole batch back with the standard 400 response. Missing-parameter checks run against every set BEFORE the tx opens for fail-fast. Supersedes the batch-atomicity part of task 003.
- **Native Postgres array binding.** Homogeneous scalar JSON arrays bind as native `text[]` / `int8[]` / `float8[]` / `bool[]` so a single SQL statement using `unnest()` handles the whole set in one round-trip. Mixed / nested / all-null / empty arrays fall back to JSONB with a `debug`-level log. Nulls inside a homogeneous array stay as SQL NULL elements (via `Vec<Option<T>>`). SQLite continues to bind arrays as JSON strings — use `json_each()` to unpack.
- New book sections in `sql-files.md`: atomicity guarantee for `/batch`, native array parameters, per-file `@transactional` marker. `failure-modes.md` entry documenting batch rollback semantics.

### Added — per-file `@transactional` marker (task 003)
- SQL files with `-- @transactional` in the leading comment block execute inside a single database transaction (commit on success, rollback on any error). Recognised only before the first non-comment line; applies to both GET and POST endpoints (primary use case: multi-statement POST files).

### Added — Java compatibility layer
- `src/config_compat.rs`: reads Java `application.yml` (and `application-{prod,dev,test}.yml`) at boot, translates Spring-shape keys to Resql config, and records diagnostics for anything unsupported so operators know exactly what won't carry over.
- Auto-discovery of Java-style config paths at boot: `/app/resql.yaml`, `./resql.yaml`, `./application.yml`, `./application-{prod,dev,test}.yml` — first hit wins.
- `/datasources` response uses Java-canonical camelCase (`jdbcUrl`, `driverClassName`) so existing Spring-era dashboards keep working. Driver class inferred from the pool type.
- `/health` shape extended with Java-canonical fields; `/healthz` alias preserved.
- Boot logs every captured compat diagnostic per Java §6.2 so operators can determine unsupported-feature dependencies from a single log read.

### Added — Postgres integration test suite (task 006 — actually shipped in alpha.1, retroactively documented here)
- `tests/integration_postgres.rs` — now 24 tests covering type mapping (JSONB, TIMESTAMPTZ, NUMERIC, BOOLEAN, DATE), snake→camel columns, INSERT+RETURNING, batch endpoint, SQL errors, password masking, and the new atomicity + array binding + transactional-marker behaviours. Skips silently without `TEST_POSTGRES_URL`.
- Liquibase-managed schema and test fixtures (`db/changelog/master.yaml` + `001-schema.yaml` + `002-test-fixtures.yaml`). Test-only data is gated on `context: test` so production applications never see it.
- CI (`tests.yml`): Postgres 16 service container + Liquibase update step (`--contexts=test`) on both amd64 and arm64 matrix rows.
- `Makefile` with `pg-up` / `pg-schema` / `test-pg` / `test-all` targets for local Postgres development.
- New book chapter `book/src/postgres-setup.md` covering the Liquibase pattern, test workflow, and deployment recipes (init container + one-shot job).

### Added — reference-shape regression tests
- `tests/integration_reference_shapes.rs` + `tests/fixtures/*.json` — golden-file assertions that `/health`, `/datasources`, and error responses keep the Java-canonical field set that Spring-era clients depend on.
- `tests/integration_java_compat.rs` — end-to-end tests for the compat translator (config parsing, diagnostic capture, health/datasources shape).
- Docs: `docs/DIVERGENCES.md`, `docs/PORTING.md`, `docs/MIGRATION.md`, `docs/REFACTO-DEVIATIONS.md`, `docs/audits/2026-08-04-spec-compliance.md` — explicit inventory of what differs from JVM Resql and why.

### Changed
- Postgres testing promoted from backlog task 006 to a first-class CI requirement. The runtime image still ships without Liquibase or a JVM — schema is applied out-of-band.
- `query::execute_batch` and `query::execute_transactional` are the new atomicity entry points; the classic `query::execute` still works for non-transactional single-shot queries.

### Fixed
- Postgres INT4 columns now decode correctly (previously fell through to `null` because the extractor only tried `i64`; sqlx-postgres decodes INT4 as `i32`). Surfaced by the new Postgres suite.
- Postgres NUMERIC columns preserve full precision as a JSON string (previously `null` because the required `rust_decimal` sqlx feature was off).

## [0.1.0-alpha.1] - 2026-07-29

Initial Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service. Interface-compatible with the original for the SQL-file-to-endpoint,
`:named`-parameter, and snake→camel result semantics.

### Added

- SQL file loader (`sql/<project>/<GET|POST>/<name>.sql` → `<METHOD> /<project>/<name>`).
- Named-parameter binder with awareness of string literals, comments, and Postgres `::` casts.
- Repeated-parameter support: `SELECT :x, :x, :y` binds `x` once.
- Multi-datasource routing with three-tier resolution:
  1. `X-Datasource` request header (when `allow_datasource_header: true`).
  2. `project_datasource_map` entry.
  3. Project name as datasource name.
- Postgres and SQLite pools (dispatched by URL scheme).
- Batch endpoint at `<POST-path>/batch`.
- Health endpoint (`/health` and `/healthz` alias).
- Datasource listing endpoint (`/datasources`) with password masking.
- CORS layer + configurable body-size cap (413 on overflow).
- Structured JSON error responses matching JVM Resql shape.
- Docker image (multi-stage, non-root, tini, self-contained demo).
- CI: tests (matrix amd64 + arm64), security (audit + deny + daily cron), publish (multi-arch, provenance, SBOM, cosign, Trivy), docs (mdBook to Pages).
- mdBook: introduction, getting-started, configuration, sql-files, failure-modes.

### Fixed (vs JVM Resql)

- Datasource-by-project routing (JVM version hardcoded a single datasource name — multi-database deployments were impossible without patching the source).
- Startup refuses to boot on any misconfigured datasource (JVM version silently ignored several).
- Passwords never appear in config file (JVM defaulted keystore password to `"123456"`).
- Request body cap prevents unbounded memory growth.

### Security

- `deny.toml` bans `openssl`, `openssl-sys`, `serde_yaml` (unmaintained).
- `cargo audit --deny warnings` runs on every push, PR, and daily cron.
- Container image signed with cosign keyless via GHA OIDC.
- Trivy HIGH/CRITICAL scan gates image signing.

[Unreleased]: https://github.com/turnerrainer/Resql/compare/v0.1.0-alpha.4...HEAD
[0.1.0-alpha.4]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.1
