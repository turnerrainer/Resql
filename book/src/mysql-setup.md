# MySQL / MariaDB backend

MySQL and MariaDB are first-class alongside Postgres and SQLite. Both
speak the same wire protocol and are served by the same code path
(`Pool::MySql`, `Dialect::MySql`). A single Resql instance can serve
Postgres **and** MySQL datasources at the same time — the project →
datasource routing picks the backend per request.

This page is the authoritative reference. If you are an LLM writing a
`.sql` file for a MySQL-backed endpoint, read the [LLM authoring
checklist](#llm-authoring-checklist) at the end first.

## Decision rule — which dialect to write

The dialect you write in each `.sql` file is **determined entirely by
the URL scheme of the datasource that serves it**. There is no
cross-dialect translation layer.

| Datasource URL scheme | Dialect to write |
|---|---|
| `postgres://` or `postgresql://` | Postgres (PL/pgSQL idioms, `$N` casts, `ANY(array)`, `RETURNING`, `ON CONFLICT`, JSONB) |
| `mysql://` or `mariadb://` | **MySQL** (JSON predicates, `LAST_INSERT_ID()`, `ON DUPLICATE KEY UPDATE`, `CAST(x AS SIGNED)`, no `::` operator) |
| `sqlite:` | SQLite (ANSI-ish; `json_each`, `?N` placeholders — not something you usually hand-write) |

The `project_datasource_map` entry (or default project-name routing)
determines which datasource runs each file. Pick the dialect by looking
at the resolved datasource's URL. See [Datasource
selection](./sql-files.md#datasource-selection) for the resolution
rules.

> **Why no translator?** Postgres and MySQL diverge on upserts,
> returning clauses, cast syntax, JSON access, array/list predicates,
> and dozens of smaller shapes. A translation layer would be a
> product-sized undertaking with opaque failure modes. Writing native
> dialect per file keeps the bind layer honest and the error surface
> close to the author. Rationale: `DIVERGENCES.md` DIV-023.

## Configuration

A MySQL datasource is just another entry in `datasources:`. Nothing
new in the surrounding YAML — the URL scheme is the only switch.

```yaml
datasources:
  - name: analytics
    url:          "mysql://analytics-db.internal:3306/analytics"
    username:     "resql_analytics"
    password_env: "RESQL_ANALYTICS_PASSWORD"
    max_connections: 10          # default; raise carefully
    acquire_timeout_seconds: 5   # default; back-pressure on pool exhaustion
```

MariaDB is identical — swap the scheme:

```yaml
  - name: legacy
    url:          "mariadb://legacy-maria.internal:3306/app"
    username:     "resql"
    password_env: "RESQL_LEGACY_PASSWORD"
```

Both schemes land in the same driver; the only user-visible difference
is the `/datasources` listing (reports driver class
`com.mysql.cj.jdbc.Driver` either way, since MySQL Connector/J speaks
MariaDB too — the field is a Java-compat label, not a routing hint).

### Full example — Postgres + MySQL side by side

```yaml
sql_dir: ./sql
datasources:
  - name: users             # Postgres
    url:          "postgres://users-db.internal:5432/users"
    username:     "resql_users"
    password_env: "RESQL_USERS_PASSWORD"

  - name: analytics         # MySQL
    url:          "mysql://analytics-db.internal:3306/analytics"
    username:     "resql_analytics"
    password_env: "RESQL_ANALYTICS_PASSWORD"

project_datasource_map:
  users:     users
  analytics: analytics
```

Then `sql/users/GET/by-login.sql` is Postgres SQL and
`sql/analytics/GET/by-day.sql` is MySQL SQL. Same Resql process, same
request shape, two dialects.

## Postgres → MySQL cheat sheet

Lift this verbatim when porting SQL. **These are the only idioms that
change between the two backends in practice.**

| Need | Postgres | MySQL / MariaDB |
|---|---|---|
| Array membership | `WHERE id = ANY(:ids)` | `WHERE JSON_CONTAINS(:ids, CAST(id AS JSON))` *(portable)* or `WHERE id MEMBER OF(:ids)` *(MySQL 8.0.17+)* |
| Expand array into rows | `SELECT * FROM unnest(:ids) AS t(id)` | `SELECT id FROM JSON_TABLE(:ids, '$[*]' COLUMNS (id INT PATH '$')) jt` |
| Upsert | `INSERT … ON CONFLICT (k) DO UPDATE SET …` | `INSERT … ON DUPLICATE KEY UPDATE …` |
| Return inserted row | `INSERT … RETURNING *` *(one statement)* | Two separate endpoints — see "Multi-statement limitation" below. Alternatively, use the batch endpoint. |
| Cast | `value::int` | `CAST(value AS SIGNED)` (integers) / `CAST(x AS DECIMAL(10,2))` (etc.) |
| Case-insensitive LIKE | `col ILIKE 'foo%'` | `LIKE 'foo%'` *(collation-dependent — use a `_ci` collation on the column)* |
| JSON field access | `data->>'name'` | `data->>'$.name'` *(JSONPath required)* |
| Regex match | `col ~ 'pattern'` | `col REGEXP 'pattern'` |
| String concat | `a \|\| b` | `CONCAT(a, b)` *(`\|\|` is OR by default on MySQL)* |
| Date arithmetic | `now() - interval '7 days'` | `NOW() - INTERVAL 7 DAY` |
| Auto-increment PK | `GENERATED ALWAYS AS IDENTITY` / `SERIAL` | `AUTO_INCREMENT` |
| Boolean type | real `BOOLEAN` | `TINYINT(1)` convention (no true boolean) |
| Current timestamp | `now()` / `CURRENT_TIMESTAMP` | `NOW()` / `CURRENT_TIMESTAMP` *(both work)* |
| TRUE / FALSE | literal `TRUE` / `FALSE` | `1` / `0` *(MySQL accepts `TRUE`/`FALSE` as aliases for 1/0)* |

Anything not in this table is portable or trivially different; when in
doubt, write MySQL SQL and trust the error if it's wrong.

## Named parameters on MySQL

Same `:name` syntax as every other backend. One behaviour difference
to be aware of: **MySQL uses positional-anonymous `?` placeholders at
the wire**, so a repeated `:name` binds the value **at every
occurrence** independently.

```sql
-- Postgres: $1 reused, one bind
-- MySQL:    two anonymous ?, two binds (both get the same value)
SELECT :login AS a, :login AS b;
```

Semantically identical for the caller — the same value lands in both
positions — but worth knowing when reasoning about query cost. The
rewriter handles this automatically; no author action needed.

## Arrays — the array-as-JSON contract

MySQL has no native array type. Resql's declaration still accepts
`type: array` with an `items:` block, and element-level type
validation runs exactly as on Postgres (wrong-typed elements rejected
at the request boundary before the SQL runs). The only difference is
on the SQL side: the bind sends a **JSON string**, and the author
uses JSON predicates to consume it.

Declare the param:

```yaml
params:
  ids:
    type: array
    items:
      type: integer
    required: true
```

Then any of these on the SQL side:

```sql
-- Portable to every MySQL 5.7+ / 8.x — recommended default
SELECT id, login FROM users WHERE JSON_CONTAINS(:ids, CAST(id AS JSON));

-- Terser on MySQL 8.0.17+
SELECT id, login FROM users WHERE id MEMBER OF(:ids);

-- Expand into a row set for joins
SELECT u.id, u.login
FROM users u
JOIN JSON_TABLE(:ids, '$[*]' COLUMNS (id INT PATH '$')) jt ON jt.id = u.id;
```

Caller sends the array as normal JSON:

```json
{ "ids": [1, 2, 7] }
```

Rules:

- **Any `type: array` with any `items:` type binds as JSON.** This
  includes `items: { type: uuid }`, `items: { type: date }`, nested
  arrays, arrays of objects — all land as a JSON string.
- **Element validation still runs.** Wrong-typed elements error at
  the request boundary with `name[i]: expected <type>, got <actual>`
  before any SQL runs. The declaration remains authoritative.
- **Nulls inside the array are preserved** as JSON `null`; use
  `JSON_UNQUOTE(JSON_EXTRACT(:ids, '$[*]'))` or `JSON_TABLE` with
  `DEFAULT NULL ON EMPTY` when the SQL side needs to see them as SQL
  NULL.
- **Casting matters in `JSON_CONTAINS`.** `JSON_CONTAINS('[1,2,3]',
  1)` returns 0 — the needle must be JSON too. Hence the
  `CAST(id AS JSON)` idiom in the cheat sheet.

## Transactions on MySQL

Resql's `-- @transactional` marker works on MySQL — commit on
success, rollback on any error. Batch endpoints are always
transactional. **One important limitation vs. Postgres**: each
`.sql` file on MySQL must contain exactly **one statement**.

### Multi-statement limitation

MySQL's prepared-statement protocol (`COM_STMT_PREPARE`) can only
hold a single statement per prepare. sqlx-mysql uses prepared
statements for every query to get safe parameter binding, so a
`.sql` file containing `INSERT …; SELECT …;` cannot be executed and
will surface as a `BadSqlGrammarException` (`400`).

Postgres's extended-query protocol accepts semicolon-separated
multi-statement SQL in a single `Parse`, which is why the same
`-- @transactional` recipe works there.

### Insert-and-return-row pattern

Split into two endpoints. The caller does one POST + one GET (or
projects just the id client-side from the INSERT).

```sql
-- sql/users/POST/create.sql
/*
params:
  login:  { type: string, required: true }
  email:  { type: string, required: false }
  status: { type: string, required: false, default: active }
*/
-- @transactional
INSERT INTO users (login, email, status) VALUES (:login, :email, :status);
```

```sql
-- sql/users/GET/find-by-login.sql
/*
params:
  login: { type: string, required: true }
*/
SELECT id, login, email, status FROM users WHERE login = :login;
```

Alternatively, the batch endpoint (`POST /users/create/batch`) runs
multiple INSERT statements in one transaction with all-or-nothing
atomicity — see [Batch endpoint](./sql-files.md#batch-endpoint).

### When `-- @transactional` is useful on single-statement MySQL files

- The statement interacts with triggers that write to other tables
  and you want atomic rollback on error anywhere in the trigger
  chain.
- The statement is a multi-row INSERT and you want "all rows or
  no rows" semantics (vs. partial INSERT on some constraint
  failures depending on the storage engine).
- Composability with middleware that assumes transactional
  endpoints (future-proofing).

For plain single-row INSERT/UPDATE/DELETE on InnoDB, the marker is
a no-op — InnoDB wraps each statement in an implicit transaction
anyway.

## Timeout caveat — different variable on MySQL vs. MariaDB

When `server.request_timeout_seconds > 0`, Resql applies a
server-side query timeout in every pool connection's
`after_connect` hook. The variable name AND units differ between
MySQL and MariaDB — Resql detects the server at connect time
(`SELECT VERSION()`) and emits the right one:

| Server | Statement issued | Units | Available since |
|---|---|---|---|
| MySQL | `SET SESSION MAX_EXECUTION_TIME = <ms>` | milliseconds (INT) | 5.7.8 (2015) — SELECT-only, hint-shaped |
| MariaDB | `SET SESSION max_statement_time = <seconds>` | seconds (DOUBLE) | 10.1.1 (2015) — applies to all statements |

Older servers (MySQL <5.7.8 or MariaDB <10.1) have no equivalent —
Resql emits a WARN at connect naming the server version and lets
the pool come up. The HTTP-level `TimeoutLayer` remains the
authoritative wall-clock cap in every case; the server-side knob
is defence in depth.

### Why two different variables?

MariaDB forked MySQL in 2009 and shipped its own statement-timeout
implementation (`max_statement_time`) in 10.1.1 (October 2015).
MySQL added its own version (`MAX_EXECUTION_TIME`) three months
later in 5.7.8 (September 2015) with different semantics (SELECT-only
hint vs. hard kill) and different units (ms vs. seconds). The two
variables are intentionally incompatible — sending the MySQL
variable to a MariaDB server returns error 1193 (`Unknown system
variable 'MAX_EXECUTION_TIME'`) and the entire pool refuses to open.

### Important semantic differences from Postgres `statement_timeout`

1. **MySQL is SELECT-only.** `MAX_EXECUTION_TIME` ignores INSERT /
   UPDATE / DELETE / DDL. MariaDB's `max_statement_time` applies to
   all statements, so it's actually closer in spirit to Postgres's
   knob.
2. **MySQL is hint-shaped.** The optimizer may still finish past
   the budget on specific plans. Treat it as "likely to kill a
   runaway SELECT," not "guaranteed."

Companion knob the DBA may want to set on the server:
`innodb_lock_wait_timeout` (controls how long a transaction waits for
row locks). Resql does not touch it — leave it to your MySQL / MariaDB
config.

## Row → JSON type map

Full type map for columns returned from a MySQL datasource. Matches
the Postgres path's ISO 8601 conventions where applicable.

| MySQL column type | JSON output |
|---|---|
| `TINYINT` *(signed)* | number *(i64)* |
| `TINYINT UNSIGNED` | number *(i64)* |
| `TINYINT(1)` | number *(i64)* — MySQL has no boolean; caller interprets `0` / `1`. Use `CAST(col AS JSON)` in SQL if you need JSON `true`/`false`. |
| `SMALLINT`, `MEDIUMINT`, `INT`, `BIGINT` *(signed)* | number *(i64)* |
| `SMALLINT UNSIGNED`, `MEDIUMINT UNSIGNED`, `INT UNSIGNED` | number *(i64)* |
| `BIGINT UNSIGNED` | number *(u64 — may exceed `i64::MAX`)* |
| `FLOAT`, `DOUBLE` | number |
| `DECIMAL(p, s)` / `NUMERIC` | **string** — precision preserved (same policy as Postgres NUMERIC) |
| `DATE` | string `YYYY-MM-DD` |
| `DATETIME` / `TIMESTAMP` | string `YYYY-MM-DDTHH:MM:SS[.ffffff]` *(ISO 8601 with `T` separator; UTC-anchored → trailing `Z`)* |
| `TIME` | string `HH:MM:SS[.ffffff]` |
| `YEAR` | number |
| `CHAR`, `VARCHAR`, `TEXT`, `TINYTEXT`, `MEDIUMTEXT`, `LONGTEXT` | string |
| `ENUM` | string *(the member label)* |
| `SET` | string *(comma-separated member list — MySQL's wire shape; splitting requires schema knowledge)* |
| `JSON` | unwrapped JSON value *(array / object / scalar — not a stringified copy)* |
| `BIT(1)` | boolean |
| `BIT(N)`, N > 1 | string *(lowercase hex)* |
| `BINARY`, `VARBINARY`, `BLOB`, `TINYBLOB`, `MEDIUMBLOB`, `LONGBLOB` | string if valid UTF-8, else `null` |
| NULL (any column) | `null` |

Column names are camelCased on the way out, same as every other
backend: `created_at` → `createdAt`.

### Why `TINYINT(1)` is a number, not a boolean

MySQL has no native boolean type — `BOOLEAN` is a `TINYINT(1)` alias.
sqlx decodes it as an integer, and we surface it as such. If an
endpoint consumer specifically needs JSON `true`/`false`, project the
column:

```sql
SELECT CAST(enabled AS JSON) AS enabled FROM flags WHERE name = :name;
-- → [{"enabled": true}]
```

Or at the caller side, treat `0` / `1` the way MySQL ecosystems
already do.

## Local development

Run a disposable MySQL 8 instance:

```bash
docker run --rm -d --name resql-mysql \
    -e MYSQL_ROOT_PASSWORD=rootpw \
    -e MYSQL_DATABASE=resql \
    -e MYSQL_USER=resql \
    -e MYSQL_PASSWORD=resqlpw \
    -p 3306:3306 \
    mysql:8.0

export RESQL_ANALYTICS_PASSWORD=resqlpw
./target/release/resql -c resql.yaml
```

To include the MySQL integration suite in `cargo test`:

```bash
export TEST_MYSQL_URL="mysql://root:rootpw@localhost:3306/resql"
cargo test --no-fail-fast --locked
```

The suite drops and re-creates its own fixture tables on every run —
nothing to pre-provision. See `tests/integration_mysql.rs` for the
schema it uses.

## LLM authoring checklist

When you (as an LLM assistant) are asked to write or modify a `.sql`
file in this repo, run this checklist **before** emitting SQL:

1. **Find the resolved datasource.** Open `resql.yaml`. If
   `project_datasource_map` has an entry for the file's URL project,
   that's the datasource name. Otherwise, the project name is the
   datasource name. Find that entry under `datasources:` and read the
   URL scheme.
2. **Pick dialect from scheme:**
   - `postgres://` / `postgresql://` → write **Postgres SQL** (your
     default assumption; this is the majority target in this repo).
   - `mysql://` / `mariadb://` → write **MySQL SQL**. Use the cheat
     sheet above. Avoid every Postgres-ism (`::` casts, `RETURNING`,
     `ANY()`, `ILIKE`, `||` for concat, JSONB operators, interval
     literals with quoted text).
   - `sqlite:` → write SQLite SQL (rare — usually test fixtures).
3. **Array params on MySQL**: declare `type: array` + `items:` as
   normal, then use `JSON_CONTAINS(:xs, CAST(col AS JSON))` /
   `MEMBER OF(:xs)` / `JSON_TABLE(:xs, '$[*]' …)` on the SQL side.
   Never emit `WHERE col = ANY(:xs)` for a MySQL file.
4. **No multi-statement MySQL files.** MySQL prepared statements
   only hold one statement — a `.sql` file with
   `INSERT …; SELECT …;` on MySQL will fail with
   `BadSqlGrammarException`. If the task needs insert-and-return-row,
   split into **two files** (one POST for the INSERT, one GET for
   the SELECT by login/id). The `-- @transactional` marker is still
   valid on single-statement MySQL files for trigger-chain
   atomicity.
5. **Dates/timestamps**: ISO 8601 strings bind directly. No casts
   needed. If you want to be explicit, `CAST(:ts AS DATETIME)`.
6. **Verify after writing**: grep the file for `::`, `RETURNING`,
   `ANY(`, `ILIKE`, `JSONB`. If any of those appear in a MySQL file,
   something is wrong — rewrite using the cheat sheet.
7. **Row-mapper awareness**: `TINYINT(1)` comes back as `0` / `1`
   (not `true` / `false`); `DECIMAL` comes back as a string; `JSON`
   comes back as an unwrapped value. If the caller shape must be
   different, project the column in SQL (see "Why `TINYINT(1)` is a
   number" above).

When editing Rust code that touches `Pool`: every match on `Pool`
must have **three** arms — `Postgres`, `Sqlite`, `MySql` — the
compiler will enforce exhaustiveness.
