# Failure modes

Every error response is a JSON object of shape:

```json
{"error": "<ExceptionClassName>", "message": "<human-readable>"}
```

`error` is a stable identifier suitable for programmatic branching.
`message` is descriptive and may change between minor versions.

## HTTP status codes

| Status | When |
|---|---|
| **200** | Query executed. Body is a JSON array (possibly empty). |
| **400** | Any structured application error — see the table below. |
| **413** | Request body larger than `server.max_body_bytes`. |
| **500** | Panic or unexpected internal error. Reported to logs; body is minimal. |

Note: JVM Resql returned **400 for every error class**, including
"query not found." Resql keeps that behaviour for compatibility;
only `413` (body too large) and `500` (unhandled internal) sit outside.

## Error catalog

| `error` field | Cause | HTTP |
|---|---|---|
| `ResqlRuntimeException` | The URL doesn't match any loaded SQL file. | 400 |
| `UnknownDataSourceNameException` | The resolved datasource name is not in config. | 400 |
| `InvalidDataAccessApiUsageException` | A required declared param is missing (or JSON `null`) from the request. | 400 |
| `UnknownParameterException` | Request carries a key not declared in the endpoint's `params:`. | 400 |
| `InvalidParameterTypeException` | Request value doesn't match the declared type (or GET query-string coercion failed). | 400 |
| `InvalidQueryException` | Malformed SQL file caught at load (empty file, unreadable). | Startup fails |
| `InvalidDeclarationException` | SQL file has no declaration fence, malformed YAML, or declared/referenced params disagree. | Startup fails |
| `InvalidDirectoryException` | `sql_dir` missing, not a directory, or unreadable. | Startup fails |
| `BadSqlGrammarException` | SQL execution failed (syntax error, unknown table, type mismatch). | 400 |
| `MalformedRequestException` | Body is not valid JSON, or batch body has no `queries` field. | 400 |
| `PayloadTooLargeException` | Request body exceeded `server.max_body_bytes`. | 413 |
| `InternalError` | Unhandled panic reached the top of the stack. | 500 |

## Startup failures

The process exits non-zero and writes a single line at ERROR level. It
does **not** attempt to run in a degraded state.

- Missing / invalid `sql_dir`
- Duplicate SQL endpoints
- Empty SQL file
- SQL file with no declaration fence
- Declaration references params the SQL doesn't use, or vice versa
- Malformed YAML inside the declaration fence
- Duplicate datasource `name`
- Datasource with `username` but no `password_env`
- `password_env` naming an unset environment variable
- `project_datasource_map` referring to an unknown datasource
- Unknown fields in the config YAML (typo protection)

## Runtime failures

- Datasource pool exhaustion → `BadSqlGrammarException` with the pool
  message; usually means `max_connections` is set too low.
- Client cancels mid-query → connection returned to pool; nothing
  logged unless the underlying driver reports it.
- Panic in a handler → 500 with `InternalError`; the panic goes to logs
  with backtrace when `RUST_BACKTRACE=1`.
- **Batch rollback on iteration N (since v0.1.0-alpha.2):** the batch
  endpoint runs all iterations inside one transaction. When iteration
  N fails (SQL error, constraint violation, etc.), iterations 1..N-1
  are rolled back atomically. The client sees a single 400 with the
  standard error shape from the failing iteration; there is no
  per-iteration status array. To debug WHICH iteration triggered the
  failure, log the request body on the client side or split the batch.
- **`@transactional` marker rollback:** an endpoint marked
  `-- @transactional` behaves the same on failure — the whole SQL file's
  execution rolls back. Without the marker, mid-string failures in
  multi-statement SQL may leave earlier statements committed.

## Debugging

Turn logging up with:

```bash
RESQL_LOG="debug,resql=trace,sqlx=debug" \
  ./resql --config resql.yaml
```

`sqlx=debug` prints every SQL statement and bound parameter — helpful
for tracing missing-param and grammar errors back to the source SQL.
Do **not** run production with `sqlx=debug`: bound parameters may
contain PII.
