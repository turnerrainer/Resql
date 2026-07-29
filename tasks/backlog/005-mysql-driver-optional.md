# 005 — Optional MySQL driver

## Filed
2026-07-29 — Two of the sibling services (CronManager, DataMapper) run
against MySQL. If a shared Resql instance is going to be reused, it
would need to speak MySQL.

## Severity
Low. Nothing depends on this today. File so we don't forget the
decision context.

## Motivation
Consumer demand may arrive as those two services migrate off custom
runtime. Better to have the design mapped out than scramble.

## Fix / Design
Add `sqlx` `mysql` feature behind a `mysql` cargo feature flag. Extend
`Pool` enum + `connect()` dispatch. New URL scheme check
(`mysql://` / `mariadb://`).

Per STANDARDS §"Datasource driver support", requires:

1. A design task (this file) with justification.
2. CI matrix expansion — MySQL sidecar in a new integration test file.
3. `deny.toml` re-review — MySQL brings in `mysql_common` and friends.

## Acceptance
- [ ] `mysql` feature compiles; without it, no MySQL deps compile in.
- [ ] Postgres and SQLite tests still pass with the feature on.
- [ ] New integration test `tests/integration_mysql.rs` behind
      `#[cfg(feature = "mysql")]` that spins a real MariaDB and runs the
      compat suite (query, missing param, batch).
- [ ] `book/src/configuration.md` datasource table gains a MySQL row.

## Estimated effort
2 days.

## Dependencies
Task 002. Should follow task 006 (Postgres integration in CI) so the
pattern is established.

## Non-scope
- MSSQL / Oracle (separate tasks if requested).

## Risks
- MySQL type system differs from Postgres for JSON, arrays, timestamps.
  Result mapper needs a MySQL-specific branch. Test coverage will
  surface these.
