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
"query not found." Resql-on-Rust keeps that behaviour for compatibility;
only `413` (body too large) and `500` (unhandled internal) sit outside.

## Error catalog

| `error` field | Cause | HTTP |
|---|---|---|
| `ResqlRuntimeException` | The URL doesn't match any loaded SQL file. | 400 |
| `UnknownDataSourceNameException` | The resolved datasource name is not in config. | 400 |
| `InvalidDataAccessApiUsageException` | A `:name` in the SQL had no matching JSON key. | 400 |
| `InvalidQueryException` | Malformed SQL file caught at load (empty file, unreadable). | Startup fails |
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

## Debugging

Turn logging up with:

```bash
RESQL_LOG="debug,resql_on_rust=trace,sqlx=debug" \
  ./resql-on-rust --config resql.yaml
```

`sqlx=debug` prints every SQL statement and bound parameter — helpful
for tracing missing-param and grammar errors back to the source SQL.
Do **not** run production with `sqlx=debug`: bound parameters may
contain PII.
