# Failure modes

Every error response has an empty JSON array body:

```json
[]
```

and carries the error envelope in two response headers:

```
X-Resql-Error-Code:    <ExceptionClassName>
X-Resql-Error-Message: <human-readable>
```

`X-Resql-Error-Code` is a stable identifier suitable for programmatic
branching. `X-Resql-Error-Message` is descriptive and may change between
minor versions — and is sanitised to printable ASCII (CR/LF stripped,
non-printable → `?`). Full un-sanitised text stays in the server log for
the request's trace id.

The empty-array body makes a naive downstream check like
`response.body.length > 0` safe: on any error the check now sees 0 rows
instead of `undefined`, so a DB failure cannot silently route into a
"not-found" branch by having a non-array body whose `.length` is
`undefined`. Issue #25 / DIV-022 (`DIVERGENCES.md` in the repo root).

## HTTP status codes

| Status | When |
|---|---|
| **200** | Query executed. Body is a JSON array (possibly empty). |
| **400** | Any structured application error — see the table below. Body: `[]`. |
| **401** | Inter-service bearer gate enabled and the request omitted / mismatched the token. Body: `[]`. |
| **403** | `X-Datasource` override rejected by the per-project allow-list. Body: `[]`. |
| **404** | Admin endpoint disabled (`/datasources`, `/openapi.json` with the gate off). Body: `[]`. |
| **405** | Path exists but only under a different method (FN6). Response carries `Allow:` header per RFC 7231. Body: `[]`. |
| **413** | Request body larger than `server.max_body_bytes`. Body: `[]`. |
| **429** | Optional rate limiter exhausted (R8). Response carries `Retry-After` header. Body: `[]`. |
| **500** | Panic or unexpected internal error. Reported to logs; body: `[]`. |
| **504** | Request exceeded `server.request_timeout_seconds`. Body: `[]`. |

Every response — including the ones `tower-http`'s middleware emits
outside the router (413, 504) — carries the header-envelope shape
(FN5). The empty-array body plus the two `X-Resql-Error-*` headers are
uniform across every failure path.

## Error catalog

| `X-Resql-Error-Code` header | Cause | HTTP |
|---|---|---|
| `ResqlRuntimeException` | The URL doesn't match any loaded SQL file. | 400 |
| `UnknownDataSourceNameException` | The resolved datasource name is not in config. | 400 |
| `InvalidDataAccessApiUsageException` | A required declared param is missing (or JSON `null`) from the request. | 400 |
| `UnknownParameterException` | Request carries a key not declared in the endpoint's `params:`. | 400 |
| `InvalidParameterTypeException` | Request value doesn't match the declared type (or GET query-string coercion failed). | 400 |
| `InvalidQueryException` | Malformed SQL file caught at load (empty file, unreadable). | Startup fails |
| `InvalidDeclarationException` | SQL file has no declaration fence, malformed YAML, or declared/referenced params disagree. | Startup fails |
| `InvalidDirectoryException` | `sql_dir` missing, not a directory, or unreadable. | Startup fails |
| `BadSqlGrammarException` | SQL execution failed (syntax error, unknown table, type mismatch); also the generic-message shape returned by a failing batch iteration (R9). | 400 |
| `MalformedRequestException` | Body is not valid JSON, or batch body has no `queries` field / carries unknown top-level keys. | 400 |
| `ForbiddenDatasourceOverrideException` | `X-Datasource: <name>` names a datasource not in the per-project allow-list (R1). Message names only the offender, not the registry. | 403 |
| `UnauthorizedException` | Inter-service bearer gate enabled (`security.inter_service_token_env`) and the caller's `Authorization: Bearer …` was missing or wrong. Message is generic (F-RES-3). | 401 |
| `MethodNotAllowedException` | Saved query exists under a different method than the request used. Response carries `Allow: <methods>` header (FN6, RFC 7231 §7.4.1). | 405 |
| `NotFoundException` | Admin endpoint disabled (`/datasources` when `admin.datasources_public: false`, `/openapi.json` when `admin.openapi_public: false`). Indistinguishable from a non-mounted route. | 404 |
| `PayloadTooLargeException` | Request body exceeded `server.max_body_bytes` (FN5). | 413 |
| `RequestTimeoutException` | Request wall-clock exceeded `server.request_timeout_seconds` (FN5). | 504 |
| `TooManyRequestsException` | Optional rate limiter exhausted (R8). Response carries `Retry-After`. | 429 |
| `InternalError` | Unhandled panic reached the top of the stack. | 500 |

## Startup failures

The process exits non-zero and writes a single line at ERROR level. It
does **not** attempt to run in a degraded state.

**SQL / config semantic failures** (from `Config::from_yaml_str` and
the SQL loader):

- Missing / invalid `sql_dir`
- Duplicate SQL endpoints
- Empty SQL file
- SQL file with no declaration fence
- Declaration references params the SQL doesn't use, or vice versa
- Malformed YAML inside the declaration fence
- Duplicate datasource `name`
- Datasource with `username` but neither `password` nor `password_env`
- Both `password` and `password_env` set on the same datasource
- `password_env` naming an unset environment variable
- `project_datasource_map` referring to an unknown datasource
- `datasource_header_allowlist` referring to an unknown datasource
- `server.request_timeout_seconds: 0`
- `rate_limit.burst < rate_limit.requests_per_second` (both non-zero)
- `security.inter_service_token_env` set but the env var is unset or empty
- Unknown fields in the config YAML (typo protection)

**Boot-time runtime-posture failures** (from
`Config::validate_runtime_posture`, called from `main.rs` after parse):

- `server.bind` is non-loopback AND `security.inter_service_token_env`
  is empty AND `security.trust_network: false`. Boot fails with an
  actionable message listing the three postures that satisfy the check
  (see [Configuration → `security`](./configuration.md#security---inter-service-bearer--boot-posture)).

Both classes of failures are exercised by `resql doctor` without
binding a port — recommend running it in CI against every config file
you plan to ship.

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
