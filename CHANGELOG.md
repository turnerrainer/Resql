# Changelog

All notable changes to this project will be documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

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

[Unreleased]: https://github.com/turnerrainer/Resql/compare/v0.1.0-alpha.2...HEAD
[0.1.0-alpha.2]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.1
