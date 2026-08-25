# 008 — Mandatory declaration section + OpenAPI generation

## Filed
2026-08-25 — surfaced by [Resql#4](https://github.com/turnerrainer/Resql/issues/4)
(Resql requires callers to send every `:name` referenced by the SQL,
even when the value is only used inside an `IS NULL OR …` filter).
Chosen implementation shape aligns with Ruuter's declaration idiom
so operators recognise the pattern across the two services.

## Severity
High. Every optional-filter endpoint currently forces callers to send
explicit `null` for keys they don't care about. That's the API smell
that #4 reports and it also blocks any partner from generating a
client against Resql — there is no schema to generate from.

## Motivation

Three coupled problems, one fix:

1. **Optional params are impossible today.** `query::check_missing`
   (`src/query.rs:235`) rejects any request whose JSON body does not
   contain every `:name` referenced by the SQL, even when the SQL
   would happily accept a NULL for that key. Callers work around this
   by sending explicit `{"status": null, "role": null, ...}` for
   every optional filter — the "boilerplate on every call" bug in
   issue #4.
2. **No schema, no OpenAPI, no validation.** Resql has no way to say
   "this endpoint takes `login: string (required)` and `status:
   string (optional)`". Consumers cannot generate clients, dashboards
   cannot document endpoints, and there is no defense against a
   caller pushing a wrong-shape payload — the DB just fails
   downstream with a confusing type error.
3. **Consumer-facing surface has no self-description.** JVM Resql
   never solved this; Ruuter's Java version added a `declaration:`
   block that Ruuter-on-Rust now consumes for allowlists + basic
   OpenAPI. Adopting the same idiom means an operator who knows
   Ruuter already knows Resql's contract.

Fix all three by introducing a **mandatory** YAML declaration block
at the top of every `.sql` file that carries per-parameter type +
required + a returns schema, and use it for (a) request validation,
(b) SQL binding of missing-optional-params as SQL NULL, and (c)
OpenAPI 3.1 spec generation exposed at `/openapi.json`.

## Fix / Design

### 1. File format — YAML front-matter inside the `.sql` file

The declaration lives at the top of each `.sql` file inside a
`-- ---` … `-- ---` fenced block. Every line inside the fence is a
SQL line comment; strip the leading `-- ` and feed the resulting text
to `serde_yaml_ng`. The fence approach:

- Keeps one file per endpoint (no sidecar `.yml`).
- Stays valid SQL — an operator opening the file in `psql` sees only
  comments before the statement.
- Matches Ruuter's Java-YAML feel while remaining SQL-native.
- Extends the existing "leading comment block" pattern used by
  `-- @transactional` (`src/loader.rs:27-42`).

Example:

```sql
-- ---
-- description: Find a user by login, optionally filtered by status.
-- params:
--   login:  { type: string,  required: true }
--   status: { type: string,  required: false }
-- returns:
--   - { name: id,    type: integer }
--   - { name: email, type: string  }
-- ---
SELECT id, email FROM users
WHERE login = :login
  AND (:status IS NULL OR status = :status);
```

The fenced block sits BEFORE the existing `-- @transactional` marker
(if any); the marker stays as-is. Parsing rule: scan leading lines;
the first `-- ---` opens the block, the next `-- ---` closes it,
everything in between (with `-- ` stripped) parses as YAML.

### 2. Declaration schema

```yaml
description: string           # optional; used for OpenAPI summary + description
namespace:   string           # optional; becomes OpenAPI tag (defaults to <project>)
version:     string           # optional; per-endpoint semantic version for docs
params:                       # required (may be empty {} if SQL uses no params)
  <name>:
    type:     string | integer | number | boolean | array | object | date | datetime | uuid
    required: bool            # default: false
    format:   string          # OpenAPI format hint (e.g. "email", "int64")
    description: string       # optional per-param docs
    default:  any             # optional; sent to DB when key absent (if not required)
    items:                    # optional; only when type == array
      type: <as above>
returns:                      # optional but recommended; used for 200 response schema
  - name: string
    type: <as above>
    format: string            # optional
    description: string       # optional
    nullable: bool            # default: true
```

Type semantics (JSON → SQL):

| Declared type | Accepted JSON | Bound as |
|---|---|---|
| `string`   | string | text |
| `integer`  | number (integer-valued) | BIGINT |
| `number`   | number | DOUBLE PRECISION |
| `boolean`  | bool   | BOOLEAN |
| `array`    | array  | native scalar array when homogeneous (existing task-007 rules); JSONB fallback |
| `object`   | object | JSONB (Postgres) / JSON string (SQLite) |
| `date`     | string (YYYY-MM-DD) | DATE via `cast(:x AS DATE)` responsibility of author |
| `datetime` | string (RFC 3339) | TIMESTAMPTZ — same caveat |
| `uuid`     | string (UUID form) | UUID |

For GET endpoints, query-string values arrive as strings. Coerce to
the declared type at request boundary: `integer` → `str::parse::<i64>`,
`number` → `str::parse::<f64>`, `boolean` → accept `true|false|1|0`,
`array` → JSON-parse the raw value (`?xs=[1,2,3]`). Coercion failure
→ 400 `InvalidParameterType`.

### 3. Runtime enforcement (replaces `check_missing`)

Given a declaration:

1. **Reject unknown keys.** Any key in the incoming JSON body / query
   map that is not in `params` → 400 `UnknownParameter`. Strict
   posture; matches the "we execute your SQL against the DB, so we
   won't silently drop your input" principle. (Divergence from
   Ruuter's filter-and-continue; called out in DIVERGENCES.md.)
2. **Reject wrong types.** Value present but not the declared type
   (after GET coercion) → 400 `InvalidParameterType` with param name
   and expected type in the message.
3. **Reject missing required.** Same shape as today's
   `MissingParameter`.
4. **Bind missing optional as NULL.** For any param declared with
   `required: false` and absent from the incoming map, bind SQL NULL
   for every `:name` occurrence. This is the direct fix for issue #4.
   If `default:` is set, use that instead of NULL.

Batch endpoints (`/batch`) validate every parameter set against the
declaration before opening the transaction — same fail-fast contract
as today's `check_missing` in `query::execute_batch`.

### 4. Load-time enforcement (mandatory, no grace period)

Boot fails if any `.sql` file:

- Has no declaration fence.
- Has an unclosed / malformed fence.
- Declares a param whose type is not in the type table above.
- Declares no `params:` key (must be present, even as empty `{}`).
- References a `:name` in the SQL that is NOT declared in `params`.
- Declares a param in `params` whose name is NOT referenced by any
  `:name` in the SQL (typo protection — matches audit-cycle rule 2:
  grep for siblings; a declared-but-unused param is either a typo or
  a forgotten cleanup).

Error type: extend `ResqlError` with `InvalidDeclaration { path, reason }`,
raised at load time (surfaces through `loader::load_dir`).

### 5. OpenAPI 3.1 spec + `/openapi.json` endpoint

Build a new module `src/openapi.rs` that walks the `QueryIndex` at
boot and emits an OpenAPI 3.1 document. Serve it at `GET /openapi.json`
(new route in `src/server.rs::router`). Cache the JSON — declaration
is fixed for the process lifetime.

Extraction rules (mirror Ruuter's `openapi.rs` where they map, extend
where Ruuter's is thinner):

- **Path** = `/<project>/<name...>` (existing routing).
- **Method** = `GET` / `POST` from the file's method directory.
- **operationId** = `<method>_<project>_<slug>` (Ruuter parity).
- **summary** = `declaration.description` first line, or filename stem.
- **description** = `declaration.description` full text.
- **tags** = [`declaration.namespace` or `<project>`].
- **parameters** (GET) = each `params` entry, `in: query`, with schema
  from the declared type.
- **requestBody** (POST) = JSON object with `properties` from `params`,
  `required` list from those with `required: true`.
- **responses.200** = array of objects derived from `returns`. When
  `returns` is omitted, fall back to `type: object, additionalProperties:
  true` with a note pointing at the declaration.
- **responses.400** = `$ref: '#/components/schemas/Error'` (the
  existing ResqlError shape).
- **responses.413** = same schema, applies to every route (advertised
  because of `RequestBodyLimitLayer`).
- **info.version** = crate version (`env!("CARGO_PKG_VERSION")`).
- **info.title / description** = new optional `openapi:` block in
  `resql.yaml`:

  ```yaml
  openapi:
    title: "Resql"                    # default: "Resql"
    description: "SQL-files-as-REST." # default: brief boilerplate
    server_url: "/"                   # default: "/"
  ```

Deterministic key ordering (alpha by project, then method, then path)
so a diff on the spec is meaningful across restarts.

### 6. Compat + migration

Since declarations are mandatory, migrate the demo `sql/` files in
this repo (`sql/users/GET/hello.sql`, `sql/users/POST/echo.sql`,
`sql/audit/GET/tail.sql`, `sql/audit/POST/write.sql`) as part of this
task so `docker run turnerrainer/resql` continues to boot green.

No grace period; boot refuses to start on any `.sql` file without a
declaration. Document the migration recipe in
`book/src/sql-files.md` (fenced-block example + the `resql lint`
sub-command, see Non-scope below for what's out).

### 7. Documentation

- Update `docs/DESIGN.md` §3.3 (parameter binding) to point at the
  declaration model.
- New chapter `book/src/declarations.md` describing the fence syntax,
  type table, validation semantics, and OpenAPI mapping.
- Update `book/src/sql-files.md` examples to include the fence.
- Add a row to `docs/DIVERGENCES.md`: Resql rejects unknown keys
  (Ruuter filters).
- Update `CHANGELOG.md` under the next alpha.

## Acceptance

- [ ] `-- ---` … `-- ---` YAML fenced block parses at file load; malformed
      YAML → boot error citing file path + line inside fence.
- [ ] SQL file without a declaration → boot fails with
      `InvalidDeclaration` and actionable message.
- [ ] `:name` in SQL not present in `params` → boot fails.
- [ ] `params:` entry not referenced by any `:name` → boot fails.
- [ ] Missing required key → 400 `MissingParameter` (existing shape).
- [ ] Missing optional key → SQL NULL bind (uses `default:` if set).
      Regression test hits an `IS NULL OR col = :x` filter with and
      without the key. Fixes [#4](https://github.com/turnerrainer/Resql/issues/4).
- [ ] Unknown key in body / query → 400 `UnknownParameter`.
- [ ] Wrong-type value → 400 `InvalidParameterType` with param name +
      expected type in the message.
- [ ] GET-side coercion (`?n=42` → i64 when declared `integer`)
      works; malformed coercion → 400.
- [ ] `/openapi.json` returns a valid OpenAPI 3.1 document; passes
      `openapi-spec-validator` in a golden-file test.
- [ ] Spec ordering deterministic (project → method → path alpha).
- [ ] Demo SQL files under `sql/` migrated; `cargo test`,
      `docker run` smoke, and the existing integration suites remain
      green.
- [ ] `book/src/declarations.md` chapter + updated `sql-files.md` +
      `docs/DIVERGENCES.md` row + `CHANGELOG.md` entry.

## Estimated effort
3–4 days.

- Fence parser + validation + `InvalidDeclaration` error: 0.5 day.
- Declaration-aware bind (missing-optional → NULL, type coercion,
  unknown-key rejection): 1 day.
- `src/openapi.rs` + `/openapi.json` route + golden-file test: 1 day.
- Sample migration + docs + CHANGELOG: 0.5 day.
- Test hardening (audit-cycle rules 1-4): 0.5 day.

## Dependencies
Task 002 (MVP). Interacts with task 007 (batch validation runs
per-set against the declaration; keep the fail-fast property).
Ships in a new alpha — this is a breaking change to the SQL file
format.

## Non-scope
- **`resql lint` sub-command.** Nice to have (validate a `sql/` tree
  without booting); file as a follow-up if operators ask.
- **Server-Sent Events / streaming.** Not covered by OpenAPI 3.1
  well; unrelated to the current bug.
- **Response type detection from SQL alone.** Rely on declared
  `returns`; do not attempt to prepare-and-introspect statements to
  auto-derive the row shape. Would require a live DB at boot which
  contradicts the "config-time validation" property.
- **Multiple response codes per endpoint.** v1 documents only 200 +
  the framework 400/413. Follow-up if consumers need more.

## Risks
- **Every existing operator's `sql/` tree breaks on upgrade.** This
  is inherent to "mandatory" — call it out in the release notes,
  ship the migration recipe, and consider bumping the minor version
  (0.2.0) rather than an alpha increment since the file format is
  changing.
- **YAML parsing surface expands attack surface at boot.** Mitigation:
  `serde_yaml_ng` (already a dep), no anchors/tags surface — parse
  to a typed struct with `deny_unknown_fields`, reject anything the
  schema doesn't name.
- **GET coercion + Java-compat.** JVM Resql passed query strings
  through untyped; consumers may rely on that. Document the tightening
  in `docs/DIVERGENCES.md`. There is no straightforward way to keep
  both.

## Related
- Ruuter task 070 — align Ruuter's own declaration model with the
  same rules (typed params, required flag, returns, unknown-key
  posture, OpenAPI 3.1 richness). Filed as a sibling so the two
  services converge instead of drifting.
