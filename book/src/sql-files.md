# Writing SQL endpoints

Every `.sql` file under `sql_dir` becomes one HTTP endpoint. This chapter
covers the directory layout, parameter binding, result shaping, and the
batch API.

> **Every `.sql` file must open with a `/* … */` YAML declaration block.** See
> [Declarations & OpenAPI](./declarations.md) for the syntax and the
> validation semantics that the fence unlocks (optional params, typed
> requests, generated OpenAPI spec).

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
/*
params:
  login:
    type: string
    required: false
  status:
    type: string
    required: false
*/
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

Behaviour depends on the file's declaration
(see [Declarations & OpenAPI](./declarations.md)):

- **Required missing**: 400 with `error:
  InvalidDataAccessApiUsageException`.
- **Optional missing**: SQL NULL is bound. If the declaration sets
  `default:`, that value is bound instead.
- **Unknown key** (not in `params`): 400 with `error:
  UnknownParameterException`.
- **Wrong type**: 400 with `error: InvalidParameterTypeException`.
- **Value outside declared `enum:`**: 400 with `error:
  InvalidParameterValueException`.

## Type coercion

JSON values bind at their natural type:

- `null` → SQL NULL
- `true` / `false` → BOOLEAN
- integer → 64-bit integer
- floating-point → 64-bit double
- string → text
- arrays / objects (Postgres only) → JSONB (bind with `::jsonb` cast if
  you want the DB to enforce it)

For dates and timestamps, cast in SQL and declare the param as `datetime`:

```sql
/*
params:
  occurredAt:
    type: datetime
    required: true
    format: date-time
*/
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

**Atomicity guarantee (since v0.1.0-alpha.2):** the whole batch runs
inside a single database transaction. Every iteration binds and
executes through the same transaction; a failure on any iteration
rolls the entire transaction back — no partial writes ever land. The
response body on failure is the standard 400 shape as if the failing
query had been sent on its own.

```bash
# If the 2nd iteration violates a UNIQUE constraint, iterations 1 AND 3
# are rolled back too. A subsequent SELECT sees zero of these rows.
curl -X POST http://localhost:8080/crm/users/create/batch \
     -H "content-type: application/json" \
     -d '{"queries":[{"login":"a"},{"login":"a"},{"login":"c"}]}'
# → 400 {"error":"BadSqlGrammarException", "message":"..."}
```

Missing-parameter checks run against every parameter set BEFORE the
transaction opens, so a batch that couldn't possibly succeed fails
fast without any DB round-trips.

Soft ceiling: batching 10⁴ rows in one request works fine, but the
transaction holds row locks until commit. Consumers doing 10⁵+ row
loads should split into multiple batches at the caller.

## Native array parameters (Postgres)

Declaring `items.type` on an array parameter binds the array as its
matching native Postgres type — `text[]`, `int8[]`, `float8[]`,
`bool[]`, `uuid[]`, `date[]`, or `timestamptz[]` — even for empty
and all-null payloads. A single SQL statement using `unnest()` can
then process the whole batch in one round-trip:

```sql
-- sql/audit/POST/append-many.sql
/*
params:
  actors:
    type: array
    required: true
    items:
      type: string
  actions:
    type: array
    required: true
    items:
      type: string
returns:
  - name: id
    type: integer
    nullable: false
*/
INSERT INTO audit_log (actor, action)
SELECT unnest(:actors), unnest(:actions)
RETURNING id;
```

```bash
curl -X POST http://localhost:8080/audit/append-many \
     -H "content-type: application/json" \
     -d '{"actors":["alice","bob"],"actions":["created","updated"]}'
# → 200 [{"id":42}, {"id":43}]   (one INSERT, two rows)
```

Rules:

- **`items.type` wins.** When declared, the array binds as the
  matching native Postgres array — including for empty and all-null
  payloads. Wrong-typed elements are rejected at the request boundary
  with `xs[i]: expected <type>, got <actual>` before any SQL runs.
  Semantic types (`uuid`, `date`, `datetime`) also get strict
  per-element format validation. Nulls are permitted anywhere and
  become SQL NULL elements.
- **Legacy: no `items:` block.** The runtime scalar-type heuristic
  runs — non-null elements must all be the same scalar kind, int +
  float mixed promotes to `float8[]`, and mixed kinds
  (`[1, "two", true]`), nested (`[[1,2],[3,4]]`), all-null, and empty
  arrays fall back to JSONB binding (use `jsonb_array_elements*` in
  your SQL to unpack). New endpoints should declare `items:` — the
  fallback exists for backward compatibility.
- **Nested arrays / arrays of objects always bind as JSONB.**
  Postgres arrays are physically flat (no native "array of array"
  type), so `items: {type: array, items: {type: integer}}` still
  binds JSONB at the wire — the declaration adds recursive
  per-element *validation*, not a new native binding.
- **SQLite has no native array type.** Arrays bind as a JSON string;
  use `json_each()` to unpack:

  ```sql
  SELECT value AS actor FROM json_each(:actors);
  ```

## Per-file `@transactional` marker

Add `-- @transactional` as a leading comment after the declaration
block, before the first SQL statement, to have the endpoint's
execution wrapped in a single database transaction (commit on
success, rollback on any error). Batch endpoints are always
transactional regardless — this marker is for single-shot POST
endpoints whose SQL contains multiple statements or where the caller
wants explicit rollback semantics on failure.

```sql
/*
params:
  actor:
    type: string
    required: true
  action:
    type: string
    required: true
  userId:
    type: integer
    required: true
*/
-- @transactional
INSERT INTO audit_log (actor, action) VALUES (:actor, :action);
UPDATE users SET last_seen = now() WHERE id = :userId;
```

Rules:

- Only recognised in the leading comment block. Once real SQL starts,
  no further marker is honoured.
- Applies to both GET and POST (transaction is essentially free for a
  read-only statement; primary use case is POST).
- Absence of the marker preserves the pre-v0.1.0-alpha.2 behaviour of
  auto-commit per statement.
