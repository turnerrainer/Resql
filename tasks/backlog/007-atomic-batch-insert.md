# 007 — Atomic single-round-trip batch insert

## Filed
2026-07-30 — Surfaced when reviewing v0.1.0-alpha.1's batch story. The
`/batch` endpoint advertises batching but delivers three semantic
surprises to any caller expecting "batch = atomic bulk load."

## Severity
High. Blocks the "insert 10k rows in one request" use case that any
serious consumer will hit within days of adopting Resql. The current
behaviour is a footgun, not a feature.

## Motivation

Three concrete gaps in the shipped `/batch` implementation
(`src/server.rs::dispatch_batch`):

1. **Not one round-trip.** `dispatch_batch` calls `query::execute` in
   a loop; N inserts = N client → pool → DB traversals. Fine for
   ~10 rows, unacceptable for 10k.
2. **Not transactional.** Each iteration is auto-commit. Fail on
   iteration 50 of 100 → iterations 1–49 are already committed. Silent
   partial-write is the worst possible failure mode.
3. **No native array binding.** JSON arrays in the request body bind
   as JSONB (Postgres) or JSON string (SQLite), so the natural
   "pass an array, `unnest` it into rows" pattern requires
   `unnest(:xs::jsonb) → jsonb_array_elements_text` gymnastics.

The consequence: an ops team who runs `POST /users/create/batch` with
10 000 rows sees the request take 30 s, get half-committed on any
error, and reports Resql as "broken for large batches" without ever
opening the source. All three gaps compound each other.

## Fix / Design

Land as three coordinated changes so semantics stay consistent:

### 3a. Transaction wrapper for `/batch` (subsumes task 003)

`dispatch_batch` opens ONE transaction, executes all N parameter sets
inside it, commits at the end. Any error → rollback → HTTP 400 with
the standard `{"error":"...", "message":"..."}` shape. No partial
writes ever.

Task 003 (per-SQL-file `@transactional` marker) becomes a subset of
this — the marker becomes redundant for batch endpoints; still useful
for single-shot POSTs that want a transaction.

### 3b. Native array parameter binding (Postgres)

Detect `Value::Array` in JSON body; if the SQL binder site references
`:xs` and the value is an array of homogeneous scalars, bind as
Postgres `ARRAY[...]` (`text[]`, `int8[]`, etc.) instead of JSONB.
Enables ergonomic single-statement bulk inserts:

```sql
INSERT INTO users (login, email)
SELECT unnest(:logins), unnest(:emails)
```

One round-trip, N rows. Detection is heuristic — if the array
contains mixed types, fall back to JSONB with a WARN log so callers
notice.

SQLite has no native array type; keep the current JSON-string bind
and let the SQL author use `json_each()`. Document the divergence.

### 3c. Batch-execute contract update

Two shipping modes for `/batch`:

- **Loop mode (default, unchanged shape)**: one transaction wrapping
  N executes of the same SQL. This is 3a on top of the current
  behaviour — request/response shape stays identical.
- **Bulk mode (opt-in via `?mode=bulk` query param or header)**: body
  is a single `{"rows": [...]}` payload. The SQL file must use array
  parameters (3b); framework expands to one multi-row INSERT.

Both modes atomic-by-default. Bulk mode is the 10k-row fast path;
loop mode preserves compat for callers already using `/batch`.

## Acceptance

- [ ] `dispatch_batch` opens/commits a single transaction; rolls back on error.
- [ ] Integration test: batch of 3 inserts, iteration 2 violates a UNIQUE constraint → 400 returned AND 0 rows committed (verify via a SELECT after).
- [ ] Postgres array binding detects `Value::Array` of homogeneous scalars → binds as native array; heterogeneous falls back to JSONB with a log.
- [ ] SQLite continues to bind arrays as JSON strings; documented behavioural divergence.
- [ ] `?mode=bulk` accepted on any POST endpoint; body shape validated (`{"rows":[...]}`); SQL file's parameter list matches array element structure.
- [ ] `book/src/sql-files.md`: rewritten `#Batch endpoint` section
      covering both modes + the atomicity guarantee.
- [ ] `book/src/failure-modes.md`: entry for "batch rolled back due to
      row N failure" — explains client-visible response shape.
- [ ] Postgres integration test: bulk-mode insert of 1000 rows
      succeeds in a single round-trip (proven by
      `EXPLAIN (ANALYZE, BUFFERS)` timing < 100ms).
- [ ] SQLite integration test: bulk-mode insert works with
      `json_each()` unpack.
- [ ] Task 003 either updated to point at this task's transaction
      work, or closed with a `Superseded: 007` note.

## Estimated effort

2 days:
- 0.5d: transaction wrapper + rollback semantics + tests (3a)
- 1.0d: array binding + heuristic + SQLite divergence docs (3b)
- 0.5d: bulk mode routing + docs (3c)

## Dependencies

- Task 002 (MVP).
- Task 006 (Postgres integration tests) — 3b's native array binding
  can only be verified against a real Postgres.

## Non-scope

- Cross-datasource transactions (DESIGN §2 permanent non-goal).
- COPY FROM STDIN — genuinely faster for 100k+ rows but a different
  API shape (streaming body, no result rows). File separately if a
  consumer needs it.
- SAVEPOINTs / nested transactions.
- Client-side batching / connection pooling tuning.

## Risks

- **Long-held transactions block writers.** A 10k-row bulk insert
  in one transaction holds row locks until commit. Postgres handles
  this fine at 10k; at 1M it starts to hurt. Mitigation: document a
  soft ceiling in `sql-files.md` and let ops pick their own batch
  size at the caller.
- **Heuristic array detection surprises callers.** A JSON payload
  that looks like `[[1,2],[3,4]]` (nested array) shouldn't be bound
  as `int[]`. Mitigation: only bind flat homogeneous-scalar arrays
  natively; anything else stays JSONB with a log at INFO.
- **SQLite / Postgres divergence in bulk mode.** SQLite doesn't do
  arrays; the workaround (`json_each`) is enough for tests but real
  consumers on SQLite may still hit friction. Mitigation: document
  clearly; if the friction is real, revisit with a compile-time
  "text-based fallback" that expands to `VALUES (?), (?), ...`.
