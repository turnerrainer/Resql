# Writing SQL endpoints

Every `.sql` file under `sql_dir` becomes one HTTP endpoint. This chapter
covers the directory layout, parameter binding, result shaping, and the
batch API.

## Directory layout → URL

The scanner expects three levels:

```
<sql_dir>/<project>/<GET|POST>/<name...>.sql
```

- `<project>` — the first URL segment. Also the default datasource name.
- `<GET|POST>` — an all-caps method directory. Other names (including
  `PUT`, `DELETE`) are silently ignored.
- `<name...>.sql` — filename with an optional subdirectory prefix.

Resolved endpoint:

```
<METHOD> /<project>/<name...>
```

Examples:

| File | Endpoint |
|---|---|
| `sql/crm/GET/users/find-by-login.sql` | `GET /crm/users/find-by-login` |
| `sql/crm/POST/users/create.sql` | `POST /crm/users/create` |
| `sql/reports/GET/daily/sales-by-region.sql` | `GET /reports/daily/sales-by-region` |
| `sql/analytics/POST/rollup.sql` | `POST /analytics/rollup` |

Rules:

- Project and path lookups are **case-insensitive**. `/CRM/find` and
  `/crm/find` route to the same file.
- Non-`.sql` files are ignored (README notes, .keep files, etc.).
- Duplicate endpoints (same case-insensitive key) fail startup.
- Empty files fail startup.
- Startup fails loudly if `sql_dir` doesn't exist.

## Named parameters

Use `:name` in your SQL. Any JSON body key with a matching name binds
to it. Repeated `:name` binds the same value at every position.

```sql
-- sql/crm/POST/users/find.sql
SELECT id, email
FROM users
WHERE (:login IS NULL OR login = :login)
  AND (:status IS NULL OR status = :status);
```

```bash
curl -X POST http://localhost:8080/crm/users/find \
     -H "content-type: application/json" \
     -d '{"login":"alice","status":null}'
```

The parser understands:

- String literals (`'…'`, `"…"`) with doubled-quote escapes — `:xyz`
  inside a string is not a parameter.
- Line comments (`-- …`) and block comments (`/* … */`).
- Postgres cast syntax (`::type`) is left alone.

For `GET` endpoints, use the query string. Every key becomes a bind
target with the value as a string:

```bash
curl "http://localhost:8080/crm/users/find?login=alice"
```

## Missing / extra parameters

- **Missing**: any `:name` in the SQL that has no matching JSON key →
  HTTP 400 with `error: InvalidDataAccessApiUsageException` and the
  parameter name in the message.
- **Extra**: extra JSON keys are silently ignored.

## Type coercion

JSON values bind at their natural type:

- `null` → SQL NULL
- `true` / `false` → BOOLEAN
- integer → 64-bit integer
- floating-point → 64-bit double
- string → text
- arrays / objects (Postgres only) → JSONB (bind with `::jsonb` cast if
  you want the DB to enforce it)

For dates and timestamps, cast in SQL:

```sql
INSERT INTO events (occurred_at)
VALUES (cast(:occurredAt AS TIMESTAMPTZ));
```

## Result columns

Every result column is renamed from `snake_case` to `camelCase`:

- `user_id` → `userId`
- `PASSWORD_HASH` → `passwordHash`
- `id` → `id` (single tokens are unchanged)

Results come back as a JSON array of objects. DDL and other statements
with no result set return `[]`.

## Datasource selection

For a URL `/<project>/<name>`:

1. If `X-Datasource: <name>` is present **and** `allow_datasource_header`
   is `true`, that name wins.
2. Otherwise, if `project_datasource_map` has an entry for `<project>`,
   the mapped name is used.
3. Otherwise, `<project>` itself is the datasource name.

If the resolved name is not in `datasources`, the response is HTTP 400
with `error: UnknownDataSourceNameException`.

## Batch endpoint

Any `POST` endpoint accepts a batched variant at the same path with
`/batch` appended. The body wraps a list of parameter objects; the
response is a list-of-lists, one entry per input.

```bash
curl -X POST http://localhost:8080/crm/users/find/batch \
     -H "content-type: application/json" \
     -d '{"queries":[{"login":"alice"},{"login":"bob"}]}'
# [[{"id":1,"email":"alice@x"}], [{"id":2,"email":"bob@x"}]]
```

If any single query in the batch fails (missing param, SQL error), the
whole request fails with the same 400 shape as the equivalent non-batch
call. There is no partial-success mode by design — batching is a
performance-only shortcut, not a transactional grouping.
