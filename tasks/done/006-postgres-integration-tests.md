# 006 — Postgres integration tests in CI (Liquibase-managed schema)

## Filed
2026-07-29 — Follow-up from task 002. The Rust rewrite has full
Postgres support in code but CI only exercises the SQLite path. That
leaves the Postgres branch of `query::execute` and `db::connect` at
risk of drifting.

## Landed
2026-07-30 — commit `<pending>`. Postgres integration tests promoted
to a first-class CI requirement (not a backlog nice-to-have). Schema
and test data both managed via Liquibase — no schema code lives in
the Rust source or in test fixtures.

Deliverables shipped:
- `db/changelog/master.yaml` + `001-schema.yaml` (5 tables covering
  BIGSERIAL, TEXT, TIMESTAMPTZ, JSONB, BOOLEAN, NUMERIC, DATE) +
  `002-test-fixtures.yaml` (context=test seed data).
- `.github/workflows/tests.yml`: `postgres:16` service + Liquibase
  update step on both arch matrix rows.
- `Makefile`: `pg-up` / `pg-schema` / `test-pg` / `test-all` targets.
- `tests/integration_postgres.rs`: 16 tests covering type mapping
  (JSONB, TIMESTAMPTZ, NUMERIC, BOOLEAN), snake→camel columns,
  INSERT+RETURNING, batch endpoint, SQL errors, missing param,
  password masking in /datasources. Skips silently without
  `TEST_POSTGRES_URL`.
- `book/src/postgres-setup.md`: setup recipe + deployment patterns
  (init container / one-shot job).

## Severity
Medium. Real prod runs on Postgres; CI must exercise it before we ship
a stable 0.1.0.

## Motivation
DEV-REQUIREMENTS §3 rejects mocking databases. To keep that rule and
still get Postgres coverage, CI needs to run a real Postgres. Two
options: testcontainers-rs or a Docker sidecar service in the workflow.

## Fix / Design
Prefer **workflow sidecar** — testcontainers-rs adds a heavy dep tree
just for CI and doesn't play nicely with matrix runners.

In `tests.yml`:

```yaml
services:
  postgres:
    image: postgres:16
    env:
      POSTGRES_PASSWORD: ci
    options: >-
      --health-cmd "pg_isready"
      --health-interval 10s
      --health-timeout 5s
      --health-retries 5
    ports:
      - 5432:5432
```

Test file `tests/integration_postgres.rs` gated `#[cfg(feature = "pg-integration")]`
or `if let Ok(url) = env::var("TEST_POSTGRES_URL") {}` skipped locally by default.

Cover:

- Same suite as `integration_query.rs` but against Postgres.
- Postgres-specific types: JSONB round-trip, TIMESTAMPTZ, text[] arrays.
- Multi-datasource routing test with two separate Postgres schemas.

## Acceptance
- [x] Postgres sidecar in `tests.yml` — matrix job runs against it on both amd64 and arm64.
- [x] New `tests/integration_postgres.rs` covers the SQLite suite + PG-specific types (JSONB, TIMESTAMPTZ, NUMERIC, BOOLEAN).
- [x] Local `cargo test` still passes without a running Postgres (skips when env var missing).
- [x] Test count in HANDOFF reflects new tests.
- [x] Schema and test fixtures come from Liquibase — nothing baked into Rust or fixture SQL.

## Estimated effort
1 day.

## Dependencies
Task 002.

## Non-scope
- MySQL (task 005).
- Postgres-specific optimizations (query prepared cache, LISTEN/NOTIFY).

## Risks
- CI matrix run time increase. Mitigation: parallelize; if it becomes
  a bottleneck, split PG job to a separate workflow.
