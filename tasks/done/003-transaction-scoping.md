# 003 — Optional transaction wrapper for POST endpoints

## Filed
2026-07-29 — Follow-up from task 002. JVM Resql runs every statement
auto-commit; some consumers want the batch endpoint to run in a
single transaction.

## Severity
Medium. Blocks nothing today but is an obvious enhancement — batch
callers that mean "N inserts as one atomic unit" today have to hand-roll
a wrapping SQL file with `BEGIN; ...; COMMIT;`.

**Note (2026-07-30):** Task 007 (High) supersedes the batch-atomicity
part of this task by making `/batch` transactional by default. The
per-file `@transactional` marker described below is still open scope
for single-shot POST endpoints that need transactional semantics; if
that turns out to have no real consumer once 007 lands, close this
task as `Superseded: 007`.

## Motivation
Batch endpoint currently loops query::execute; failure in the third
of ten statements leaves the first two committed. A caller wanting
atomic semantics has no in-band way to request it.

## Fix / Design
Add optional `transactional: true` marker per SQL file (front-matter
comment: `-- @transactional`). When set:

- POST executor opens a transaction before binding.
- Batch executor opens ONE transaction for the whole batch.
- Any error rolls back.
- Successful path commits at the end.

Alternative: add `?tx=true` query param. Rejected because SQL file
semantics should not depend on how the endpoint is called.

## Acceptance
- [x] `-- @transactional` marker recognised at load time (added to `SavedQuery` struct). `src/loader.rs::parse_transactional_marker` + `SavedQuery.transactional`.
- [x] Postgres path: `pool.begin()` → bind → commit; rollback on error. `src/query.rs::execute_transactional` (Postgres branch) + `finalise_tx`.
- [x] SQLite path: same. Same function, SQLite branch + `finalise_tx_sqlite`.
- [x] Batch path: single transaction spans all N calls. **Superseded by task 007** — `query::execute_batch` opens one tx, iterates, commits or rolls back atomically. Not gated on the `@transactional` marker: batching is always atomic now.
- [x] Integration test: batch of 3 where the 2nd fails leaves 0 rows inserted. `batch_rolls_back_on_error_no_partial_writes` (SQLite) and `pg_batch_rolls_back_on_error` (Postgres).
- [x] Single-shot transactional-marker test: `transactional_marker_rolls_back_on_error` (SQLite) + `pg_transactional_marker_rolls_back_multistatement` (Postgres) — plus a control test `non_transactional_post_leaves_committed_writes` proving that WITHOUT the marker, multi-statement failures leave earlier statements committed (the exact behaviour the marker fixes).
- [x] Documented in `book/src/sql-files.md`. New section `Per-file @transactional marker`.

## Landed

2026-08-05 alongside task 007, in commits on `refacto/spec-compliance-v1`. Released as v0.1.0-alpha.2.

## Estimated effort
1 day.

## Dependencies
Task 002.

## Non-scope
- Cross-datasource transactions (out of scope forever — see DESIGN §2).
- Isolation-level configuration (add later if requested).

## Risks
- SQLite writes serialize globally; a transaction holding open across
  slow reads could throttle other writers. Mitigation: keep the doc
  chapter honest about SQLite's concurrency model.
