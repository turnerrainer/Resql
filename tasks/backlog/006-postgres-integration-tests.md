# 006 — Postgres integration tests in CI

## Filed
2026-07-29 — Follow-up from task 002. The Rust rewrite has full
Postgres support in code but CI only exercises the SQLite path. That
leaves the Postgres branch of `query::execute` and `db::connect` at
risk of drifting.

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
- [ ] Postgres sidecar in `tests.yml` — matrix job runs against it.
- [ ] New `tests/integration_postgres.rs` covers the SQLite suite + PG-specific types.
- [ ] Local `cargo test` still passes without a running Postgres (skip when env var missing).
- [ ] Test count in HANDOFF reflects new tests.

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
