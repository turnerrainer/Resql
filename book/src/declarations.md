# Declarations & OpenAPI

Every `.sql` file must open with a YAML **declaration block** naming
its parameters, their types, and (optionally) the row shape it
returns. Declarations:

- Fix the "must send every optional param" bug of the original JVM
  Resql — a declared-optional parameter can be omitted from the request
  and the SQL sees SQL NULL for that placeholder.
- Give Resql enough information to validate every request (reject
  unknown keys, coerce wrong types) before it reaches the database.
- Feed a real OpenAPI 3.1 document served at `GET /openapi.json`, so
  consumers can generate clients and dashboards can render your API
  surface without hand-written documentation.

## The declaration block

The declaration is a plain YAML document written inside a SQL
block comment. Block-style YAML matches Ruuter's convention — one
attribute per line, indented under its key:

```sql
/*
description: Find a user by login, optionally filtered by status.
namespace: crm
params:
  login:
    type: string
    required: true
  status:
    type: string
    required: false
returns:
  - name: id
    type: integer
    nullable: false
  - name: email
    type: string
*/
SELECT id, email FROM users
WHERE login = :login
  AND (:status IS NULL OR status = :status);
```

Rules:

- The `/* … */` block must be the first non-whitespace content in
  the file. Anything after `*/` — including any `-- @transactional`
  marker — is regular SQL and follows the usual rules.
- The body of the block comment is parsed as YAML verbatim.
- `params:` is required (may be `{}` for endpoints with no params).
- Every `:name` referenced by the SQL must be declared in `params`.
- Every entry in `params` must be referenced by at least one `:name`
  (typo protection).
- Unknown YAML keys anywhere in the declaration fail the boot.
- Missing / malformed block → **boot fails** with
  `InvalidDeclarationException` naming the file. No grace period.

YAML's flow style (`{ type: string, required: true }`) also parses,
but block style — used throughout this chapter — is the recommended
convention and matches Ruuter's declaration format.

## Parameter types

| Type       | Accepts (JSON)                         | Bound as                        |
|------------|----------------------------------------|---------------------------------|
| `string`   | string                                 | text                            |
| `integer`  | integer, string that parses as i64     | BIGINT                          |
| `number`   | number, string that parses as f64      | DOUBLE PRECISION                |
| `boolean`  | bool, `"true"`/`"false"`/`"1"`/`"0"`   | BOOLEAN                         |
| `array`    | array, JSON-encoded array string       | native scalar array or JSONB    |
| `object`   | object, JSON-encoded object string     | JSONB (Postgres) / TEXT (SQLite)|
| `date`     | string (YYYY-MM-DD)                    | text (cast in SQL as needed)    |
| `datetime` | string (RFC 3339)                      | text (cast in SQL as needed)    |
| `uuid`     | string (UUID form)                     | UUID / text                     |

Per-param attributes:

- `required: true|false` (default `false`).
- `default: <value>` — used when the caller omits an optional param.
  Absent + no `default` → SQL NULL.
- `format: <string>` — OpenAPI format hint, no runtime effect.
- `description: <string>` — flows into the OpenAPI param description.
- `items:` — for `type: array`, documents the element schema:
  ```yaml
  items:
    type: string
  ```
- `enum: [<values...>]` — closed set of permitted values (JSON Schema
  `enum` keyword). Rejected at the request boundary before any SQL
  binding, so the database never sees a value outside the set. Every
  entry must match the declared `type`; if a `default:` is also set it
  must be in the enum (or `null`). Empty lists are rejected at boot.

  ```yaml
  params:
    status:
      type: string
      required: true
      enum:
        - active
        - disabled
        - pending
  ```

## Samples by type

One minimum-viable declaration per supported type. Every declared
parameter must be referenced by at least one `:name` in the SQL —
these snippets do exactly that.

### `string`

```sql
/*
description: Fetch a user by login.
params:
  login:
    type: string
    required: true
returns:
  - name: id
    type: integer
    nullable: false
*/
SELECT id FROM users WHERE login = :login;
```

Request: `{"login": "alice"}`. GET equivalent: `?login=alice`.

### `integer`

```sql
/*
description: Fetch the N most recent orders.
params:
  n:
    type: integer
    required: false
    default: 10
returns:
  - name: id
    type: integer
    nullable: false
*/
SELECT id FROM orders ORDER BY id DESC LIMIT :n;
```

Request: `{"n": 25}` — or `?n=25` for GET (string coerced to i64 at the
request boundary; `?n=abc` → 400).

### `number`

```sql
/*
description: Products at or above a price threshold.
params:
  minPrice:
    type: number
    required: true
returns:
  - name: id
    type: integer
    nullable: false
  - name: price
    type: number
    nullable: false
*/
SELECT id, price FROM products WHERE price >= :minPrice;
```

Request: `{"minPrice": 9.99}`. Integers widen (`{"minPrice": 10}`
works); strings coerce (`?minPrice=9.99`).

### `boolean`

```sql
/*
description: Lookup by feature-flag state.
params:
  enabled:
    type: boolean
    required: true
returns:
  - name: name
    type: string
    nullable: false
*/
SELECT name FROM flags WHERE enabled = :enabled;
```

Request: `{"enabled": true}`. GET accepts `?enabled=true`,
`?enabled=false`, `?enabled=1`, `?enabled=0`.

### `date`

```sql
/*
description: Revenue for one calendar day.
params:
  day:
    type: date
    required: true
returns:
  - name: region
    type: string
    nullable: false
  - name: revenue
    type: number
    nullable: false
*/
SELECT region, revenue
  FROM daily_stats
 WHERE day = cast(:day AS DATE);
```

Request: `{"day": "2026-08-26"}`. `date` binds as text; use an
explicit `cast(:day AS DATE)` in Postgres so the planner picks a
date-typed index.

### `datetime`

```sql
/*
description: Append an audit entry timestamped by the caller.
params:
  actor:
    type: string
    required: true
  action:
    type: string
    required: true
  occurredAt:
    type: datetime
    required: true
*/
INSERT INTO audit_log (actor, action, at)
VALUES (:actor, :action, cast(:occurredAt AS TIMESTAMPTZ));
```

Request: `{"actor":"alice","action":"login","occurredAt":"2026-08-26T14:30:00Z"}`.
Emit-side: Postgres `TIMESTAMPTZ` columns render as
`2026-08-26T14:30:00Z` and `TIMESTAMP` as `2026-08-26T14:30:00`.

### `uuid`

```sql
/*
description: Session lookup by opaque ID.
params:
  sessionId:
    type: uuid
    required: true
returns:
  - name: userId
    type: integer
    nullable: false
  - name: expiresAt
    type: datetime
    nullable: false
*/
SELECT user_id, expires_at
  FROM sessions
 WHERE id = cast(:sessionId AS UUID);
```

Request: `{"sessionId": "d3f9b1a2-4c8e-4bde-9a2f-01a2b3c4d5e6"}`.
Malformed UUIDs fail at the DB with a 400 (Postgres rejects the cast).

### `array`

```sql
/*
description: Bulk-lookup users by a list of logins.
params:
  logins:
    type: array
    required: true
    items:
      type: string
returns:
  - name: id
    type: integer
    nullable: false
  - name: login
    type: string
    nullable: false
*/
SELECT id, login FROM users WHERE login = ANY(:logins);
```

Request: `{"logins": ["alice", "bob", "charlie"]}`. Homogeneous scalar
arrays bind as native Postgres arrays (`text[]`, `int8[]`, `float8[]`,
`bool[]`); mixed / nested / all-null / empty fall back to JSONB — see
[Writing SQL endpoints](./sql-files.md#native-array-parameters-postgres).
SQLite: `... FROM json_each(:logins)`.

### `object`

```sql
/*
description: Store a JSON document verbatim.
params:
  doc:
    type: object
    required: true
returns:
  - name: id
    type: integer
    nullable: false
*/
INSERT INTO documents (payload) VALUES (:doc::jsonb)
RETURNING id;
```

Request: `{"doc": {"title": "hello", "tags": ["a","b"]}}`. Binds as
JSONB on Postgres (cast in SQL when the column is typed), TEXT on
SQLite.

## A complete example

A single-file endpoint touching every mechanic — required + optional
params, `default:`, `enum:`, arrays, timestamps, `returns:` with
`nullable:`, and `-- @transactional`:

```sql
/*
description: Create an order for a user, tagging any promo codes.
namespace: shop
params:
  userId:
    type: uuid
    required: true
  totalCents:
    type: integer
    required: true
  currency:
    type: string
    required: true
    default: EUR
    enum:
      - EUR
      - USD
      - GBP
  placedAt:
    type: datetime
    required: false
  promoCodes:
    type: array
    required: false
    items:
      type: string
  metadata:
    type: object
    required: false
    description: Free-form vendor metadata
returns:
  - name: id
    type: integer
    nullable: false
  - name: userId
    type: uuid
    nullable: false
  - name: totalCents
    type: integer
    nullable: false
  - name: currency
    type: string
    nullable: false
  - name: placedAt
    type: datetime
    nullable: false
*/
-- @transactional
INSERT INTO orders (user_id, total_cents, currency, placed_at, promo_codes, metadata)
VALUES (
  cast(:userId AS UUID),
  :totalCents,
  :currency,
  coalesce(cast(:placedAt AS TIMESTAMPTZ), now()),
  coalesce(:promoCodes, ARRAY[]::text[]),
  coalesce(:metadata::jsonb, '{}'::jsonb)
)
RETURNING id, user_id AS "userId", total_cents AS "totalCents",
          currency, placed_at AS "placedAt";
```

Sample request:

```bash
curl -X POST http://localhost:8080/shop/orders/create \
     -H "content-type: application/json" \
     -d '{
       "userId": "d3f9b1a2-4c8e-4bde-9a2f-01a2b3c4d5e6",
       "totalCents": 4999,
       "currency": "EUR",
       "promoCodes": ["SUMMER10", "LOYAL5"],
       "metadata": {"referrer": "newsletter", "abTest": "B"}
     }'
```

Omitting `placedAt` binds SQL NULL, so `coalesce(..., now())` records
server time. Omitting `currency` uses the declared default (`EUR`).
Sending `"currency": "JPY"` → 400 `InvalidParameterValueException`
before the DB is touched.

## Runtime enforcement

For every request Resql:

1. **Rejects unknown keys** — an incoming key not in `params` produces
   400 `UnknownParameterException`.
2. **Rejects wrong types** — a value that doesn't fit the declared
   type produces 400 `InvalidParameterTypeException` naming the param
   and expected type.
3. **Rejects values outside declared `enum:`** — a value not in the
   closed set produces 400 `InvalidParameterValueException` naming
   the param, the offending value, and the allowed set. Null bypasses
   this check (nullability is governed by `required:` / `default:`).
4. **Rejects missing required** — same as before: 400
   `InvalidDataAccessApiUsageException`. A required param whose value
   is JSON `null` also counts as missing.
5. **Fills missing optionals with default (or NULL)** — the bind
   layer always sees an entry for every declared param, so
   `IS NULL OR col = :x` filters work naturally.

For GET endpoints, query-string values arrive as strings and are
coerced to the declared type at the request boundary (integer,
number, boolean, array, object). Coercion failure → 400.

## `returns:` — typed response schema

`returns:` is optional but recommended: it lets Resql advertise the
row shape in OpenAPI so consumers can type-check their side of the
wire.

```yaml
returns:
  - name: id
    type: integer
    nullable: false
  - name: email
    type: string
  - name: joined
    type: datetime
```

Fields:

- `name`, `type`: required.
- `nullable`: defaults to `true`. In OpenAPI, nullable fields are
  emitted as a two-element `type` array (`["string", "null"]`).
- `format`, `description`: optional.

Absent `returns:` → the 200 response schema is
`type: object, additionalProperties: true` — advertising honestly
that Resql doesn't know the row shape.

## OpenAPI 3.1 at `/openapi.json`

Resql serves the generated spec at `/openapi.json`. The document
is built once at boot from the declarations on every loaded
`.sql` file. Paths, methods, and operations are sorted alphabetically
so a diff on the file is meaningful across restarts.

Every POST endpoint automatically gets a `/<path>/batch` variant in
the spec, wrapping the same request body in `{ queries: [...] }`
and returning an array of arrays.

Customise `info.title` / `description` / `servers[0].url` via the
optional `openapi:` block in `resql.yaml`:

```yaml
openapi:
  title: "My Resql instance"
  description: "SQL endpoints for the ACME billing service."
  server_url: "https://api.example.com"
```

## Divergence from JVM Resql

- **Mandatory declarations.** JVM Resql accepted any `.sql` file;
  Resql now refuses to boot on any file without a declaration block.
- **Unknown keys rejected.** JVM Resql silently ignored extra body /
  query keys; Resql now returns 400. This matches Resql's "we execute
  your SQL against the DB, so we won't silently drop your input"
  principle.
- **Missing optionals bind NULL.** JVM Resql required every `:name`
  in the SQL to be present in the request; Resql binds SQL NULL when
  a declared-optional param is omitted.

See [`docs/DIVERGENCES.md`](https://github.com/turnerrainer/Resql/blob/main/docs/DIVERGENCES.md)
for the full list of intentional differences.
