# Coverage Matrix — Java Resql → Rust Resql

Generated: 2026-08-04
Branch: `refacto/spec-compliance-v1`

**Directive from user:** Rust ⊇ Java. Every existing Java-side surface must still work. Rust may add features on top.

**Verification legend:**
- (grep-v) = grep-verified — string appears/doesn't appear.
- (code-r) = code-read-verified — human traced the code.
- (repro) = reproduced against both implementations.
- **Status:** MATCH / DIVERGE / MISSING / EXTRA / NOT-VERIFIED.

---

## Section A — Configuration contract

| # | Java surface | Java default | Rust equivalent | Rust default | Status | Verif | Notes |
|---|---|---|---|---|---|---|---|
| A01 | Config file: `application.yml` (Spring convention, classpath + profile-based) | — | `resql.yaml` via `-c` or `RESQL_CONFIG` or `/app/resql.yaml` | — | **DIVERGE** | code-r | Java operator's YAML won't be discovered. |
| A02 | Config key `sqlms.saved-queries-dir` | `./templates/` | `sql_dir` (top-level) | required | **DIVERGE** | code-r | Field renamed AND no default. |
| A03 | Config key `sqlms.datasources` (array) | required | `datasources` (top-level, no prefix) | `[]` | **DIVERGE** | code-r | Field prefix removed. |
| A04 | `datasources[].name` | required | `datasources[].name` | required | MATCH | code-r | |
| A05 | `datasources[].jdbcUrl` | required | `datasources[].url` | required | **DIVERGE** | code-r | Field renamed. |
| A06 | `datasources[].username` | required | `datasources[].username` | `""` | ⚠️ partial | code-r | Java requires it; Rust allows empty. |
| A07 | `datasources[].password` (plaintext in yaml) | required | `datasources[].password_env` (env-var indirection) | `""` | **DIVERGE** | code-r | Semantic + name change. |
| A08 | `datasources[].driverClassName` | required | inferred from URL scheme | — | **DIVERGE** | code-r | Field dropped. |
| A09 | `server.port` (Spring) | `8080` (Spring default), Resql yaml sets `8082` | `server.bind` (host:port) | `0.0.0.0:8080` | **DIVERGE** | code-r | Field name/shape change. |
| A10 | `headers.contentSecurityPolicy` | `script-src 'self'` | not present | — | **MISSING** | code-r | Silent drop. |
| A11 | `userIPHeaderName` | `x-forwarded-for` | not present | — | **MISSING** | code-r | Silent drop. |
| A12 | `userIPLoggingPrefix` | `from IP` | not present | — | **MISSING** | code-r | Silent drop. |
| A13 | `userIPLoggingMDCkey` | `userIP` | not present | — | **MISSING** | code-r | Silent drop. |
| A14 | `cors.allowedOrigins` | `*` (via @Value) | `cors.allowed_origins` | `*` | **DIVERGE** | code-r | Field renamed (kebab→snake). |
| A15 | `h2.console.enabled` | `true` | not present (Rust supports SQLite/PG, not H2) | — | **MISSING** | code-r | H2 support dropped. |
| A16 | `logging.level.root` | `info` | `logging.level` (single string, EnvFilter directive) | `info,resql=debug` | **DIVERGE** | code-r | Different shape (per-package map vs single directive). |
| A17 | `spring.profiles.active` | `dev` | not present | — | **MISSING** | code-r | Rust has no profile mechanism. |

**Additions in Rust (§8.3):**
- `server.max_body_bytes` (default 1 MiB) — new **reject-condition** (returns 413), see H01.
- `server.request_timeout_seconds` (default 30) — new **reject-condition**.
- `project_datasource_map` — routes project → datasource.
- `allow_datasource_header` — toggle for `X-Datasource` header honoring.
- `logging.format` (text|json).
- `datasources[].max_connections`, `datasources[].acquire_timeout_seconds`.
- Config validation: duplicate names, missing env var, unknown datasource in map — all cause boot failure (good hygiene, no Java-surface impact).

---

## Section B — HTTP routes

| # | Java route | Java behavior | Rust route | Status | Verif | Notes |
|---|---|---|---|---|---|---|
| B01 | `POST /{project}/**` — execute POST query | 200 with JSON array; 400 on error | Same path, same shape | MATCH | code-r | |
| B02 | `GET /{project}/**` — execute GET query | 200 with JSON array; 400 on error | Same | MATCH | code-r | |
| B03 | `POST /{name}/batch` — batch (name = query name, no project) | Hardcoded datasource "byk"; 200 with array-of-arrays | **`POST /{project}/{name}/batch`** (project-prefixed) | **DIVERGE** | code-r | URL shape change — Java client calling `POST /add-debt/batch` will 404 on Rust. |
| B04 | `GET /datasources` | 200 with `[{name, jdbcUrl, username, driverClassName}]` (password stripped) | 200 with `[{name, url [masked], driver}]` | **DIVERGE** | code-r | Field names & count differ. Java operators/dashboards scraping this will break. |
| B05 | `GET /healthz` | 200 with `{appName, version:"v{M}.{m}.{p}", packagingTime, appStartTime, serverTime}` | 200 with `{appName, version:"semver", appStartTime, serverTime, status:"UP"}` | **DIVERGE** | code-r | Missing `packagingTime`; version format differs; adds `status`. |
| B06 | (implicit) any unmatched path → 400 NotFoundException `"<URI> not found"` | 400 body `{"error":"NotFoundException","message":"<URI> not found"}` | 404 (Axum default) OR route-specific handling | **NOT-VERIFIED** | — | Need to verify Rust's handling of totally-unmatched URLs. |

**Additions:**
- `GET /health` (alias of `/healthz`). — Java clients already hit `/healthz`; alias doesn't break anything. EXTRA.
- No admin/metrics/swagger routes added.

---

## Section C — User-authored file format (SQL layout)

| # | Java surface | Rust equivalent | Status | Verif | Notes |
|---|---|---|---|---|---|
| C01 | Base dir: value of `sqlms.saved-queries-dir` (default `./templates/`) | value of `sql_dir` (no default) | ⚠️ partial | code-r | Base value discoverable via renamed field; default missing. |
| C02 | Layout: `{project}/{METHOD}/{path}.sql`, METHOD ∈ {GET,POST} (uppercase) | Same layout, METHOD case-sensitive uppercase | MATCH | code-r | |
| C03 | URL from layout: `<METHOD> /<project>/<path-without-.sql>` | Same | MATCH | code-r | |
| C04 | Query-name lookup: URL project + `/` + subpath, lowercased | Same (normalize() lowercases full `project/path`) | MATCH | code-r | Both case-insensitive lookup. |
| C05 | Named-param syntax: `:name`, `[a-zA-Z_][a-zA-Z0-9_]*` | Same | MATCH | code-r | |
| C06 | Named-param matching: case-sensitive on JSON keys (Spring `NamedParameterJdbcTemplate` default) | Case-sensitive | MATCH | code-r | Earlier claim of Java-side case-insensitivity was wrong — the `.toLowerCase()` in SavedQueryService applies to the URL, not the param names. |
| C07 | File extension: `.sql` | Same (case-insensitive extension check) | MATCH | code-r | |
| C08 | Encoding: UTF-8 | UTF-8 | MATCH | code-r | |
| C09 | Missing/empty SQL dir behavior: throws `InvalidDirectoryException` at boot | Same (InvalidDirectory error) | MATCH | code-r | |
| C10 | Non-SQL files in query tree: silently ignored | Same | MATCH | code-r | Both ignore non-`.sql`. |
| C11 | Parse failure on a SQL file: logs ERROR, continues loading others | Rust: propagates InvalidQuery error at boot (does NOT continue) | **DIVERGE** | code-r | Rust is stricter — safer, but Java operators with an intentionally-broken file get boot failure instead of a partial catalog. |
| C12 | Header-comment DSL (`-- @directive:`) | None on either side | MATCH (both none) | grep-v | |

---

## Section D — Response format

| # | Java surface | Rust equivalent | Status | Verif | Notes |
|---|---|---|---|---|---|
| D01 | Column names snake_case → camelCase (`CaseUtils.toCamelCase(col.toLowerCase(), false, '_')`) | Same algorithm (query.rs:410-426) | MATCH | code-r | Both produce `userId` from `user_id`. |
| D02 | Rows: JSON array of objects | Same | MATCH | code-r | |
| D03 | Nulls included as JSON `null` | Same | MATCH | code-r | |
| D04 | INSERT/UPDATE/DELETE (no result set) → `[]` | Same | MATCH | code-r | |
| D05 | Empty SELECT → `[]` | Same | MATCH | code-r | |
| D06 | TIMESTAMP WITH TIME ZONE → ISO 8601 with `+00:00` offset (Java Jackson default) | Same via `to_rfc3339()` | MATCH | code-r | |
| D07 | TIMESTAMP (no tz) → Java: ISO 8601 in local zone or JDBC-driver-specific | Rust: `NaiveDateTime.to_string()` (space separator, no `T`) | **DIVERGE** | code-r | Potentially different serialization for `TIMESTAMP` columns. Postgres TIMESTAMP-without-tz is uncommon in Java Resql (H2/PG default is TIMESTAMPTZ), so impact is narrow. Flag for reproduction. |
| D08 | DATE → `YYYY-MM-DD` string | Same | MATCH | code-r | |
| D09 | SQL arrays → JSON arrays | Same (Postgres TEXT[]/INT[]) | ⚠️ partial | code-r | Java uses generic `array.getArray()` which relies on JDBC; Rust handles specific PG types. Broad JDBC arrays (VARCHAR[], NUMERIC[]) may differ. |
| D10 | JSON/JSONB columns → JSON value | Same | MATCH | code-r | Rust explicitly handles PG JSON/JSONB. |
| D11 | Batch response: array-of-arrays | Same | MATCH | code-r | |
| D12 | Response `Content-Type`: `application/json` | Same (Axum default) | MATCH | code-r | |

---

## Section E — Error format

| # | Java surface | Rust equivalent | Status | Verif | Notes |
|---|---|---|---|---|---|
| E01 | Body shape: `{"error":"<SimpleClassName>","message":"..."}` | Same shape (str kind + str message) | MATCH | code-r | |
| E02 | `@JsonInclude(NON_NULL)` — null message omitted | Rust serde default — always includes fields | ⚠️ minor | code-r | If Rust's message is empty string, Java would omit; Rust would emit `"message":""`. Low impact. |
| E03 | ALL exceptions → HTTP 400 | Most Rust errors → 400 EXCEPT `BodyTooLarge`→413, `Internal`→500 | **DIVERGE** | code-r | Two new status codes on new error kinds; Java-known conditions still get 400. |
| E04 | Error class names on Java-known conditions match | Rust uses the exact Java class name strings (`ResqlRuntimeException`, `UnknownDataSourceNameException`, `InvalidDataAccessApiUsageException`, `InvalidDirectoryException`, `BadSqlGrammarException`) | MATCH | code-r | Good — this is a §2.5-compliant string preservation. |
| E05 | Error class name for JSON parse error: Spring's `HttpMessageNotReadableException` | Rust: `MalformedRequestException` | **DIVERGE** | code-r | Java operators grepping for `HttpMessageNotReadableException` won't match. |
| E06 | Error class name for missing endpoint: `NotFoundException` | Rust: `ResqlRuntimeException` (via QueryNotFound) OR Axum default 404 | ⚠️ partial | code-r | Java uses different class for URL-not-found (`NotFoundException`) vs query-not-found (Rust conflates). |
| E07 | Message template: `Saved query '%s' does not exist` | Same | MATCH | code-r | |
| E08 | Message template: `Specified dataSourceName name: '%s' is unknown to the service` | Same | MATCH | code-r | |
| E09 | Message template: `Saved configuration directory missing or not a directory: <path>` | Rust equivalent per InvalidDirectory kind | NOT-VERIFIED | — | Need to check exact Rust message. |

---

## Section F — Logging / boot output

| # | Java surface | Rust equivalent | Status | Verif | Notes |
|---|---|---|---|---|---|
| F01 | Boot log `Initializing SavedQueryService` (INFO) | Rust has different boot log; no exact match | **DIVERGE** | code-r | Log-grep patterns won't match. |
| F02 | Boot log `Loading queries from <path>` (INFO) | No equivalent | **MISSING** | code-r | |
| F03 | Boot log `Loaded queries: <list>` (INFO) | Rust logs endpoint count, not names | **DIVERGE** | code-r | Java pattern grep breaks. |
| F04 | Boot log `Initializing datasource: <names>` (INFO) | Rust logs datasource count only | **DIVERGE** | code-r | |
| F05 | Per-request log `Incoming <METHOD> for <project>::<queryName>` (INFO) | Rust: tower-http `TraceLayer` produces different HTTP-request line | **DIVERGE** | code-r | |
| F06 | Per-request debug log `INPUT POST: <name>` / `INPUT GET: <name>` | No equivalent | **MISSING** | code-r | Debug-only, low impact. |

---

## Section G — Datasource routing (semantic)

| # | Java surface | Rust equivalent | Status | Verif | Notes |
|---|---|---|---|---|---|
| G01 | All queries routed to hardcoded datasource `"byk"` regardless of URL project | Queries routed to datasource named same as URL project, optionally mapped via `project_datasource_map`, optionally overridden by `X-Datasource` header | **DIVERGE (BEHAVIOR CHANGE)** | code-r | HANDOFF.md correctly calls this a Rust improvement. But for spec compliance: this IS a behavior change to a preserved surface. A Java operator with `sqlms.datasources: [{name: byk, ...}]` and templates under `templates/services/…` had all queries hit `byk`. In Rust, a template at `sql/services/…` would try to route to a datasource named `services` and fail with UnknownDataSourceNameException. |
| G02 | `X-Datasource` header (Java: no such feature) | Header override with `allow_datasource_header: true` default | EXTRA | code-r | New; doesn't break Java clients that don't send it. |
| G03 | UnknownDataSourceNameException raised if datasource lookup fails | Same | MATCH | code-r | |

---

## Section H — New reject-conditions (Rust-only checks that reject Java-legal input)

| # | Rust check | What Java did | Impact on Java client |
|---|---|---|---|
| H01 | `max_body_bytes` (1 MiB default) → 413 | No cap (Spring default is very high, effectively unlimited for JSON) | Java operator POSTing a >1 MiB batch will get 413 on Rust. |
| H02 | `request_timeout_seconds` (30s default) → 500/408 | No cap (default Servlet timeout is 30s but not enforced by app) | Long-running queries that used to succeed may now fail. |
| H03 | `password_env` required — plain `password:` in YAML not accepted | Java accepts plaintext `password:` | Java YAML fails to load on Rust. This is §2.1 category, but it's a security posture change with documented rationale. |
| H04 | `deny_unknown_fields` on all config structs | Java (Jackson default) silently ignores unknown fields | A Java YAML with `spring.*`, `headers.*`, `userIP*`, `h2.*`, `sqlms.*` prefix will be rejected outright. **This is R2.2-compliant behavior** but is a hard incompatibility with the Java config file. |
| H05 | SQL dir `sql_dir` required — no default | Java defaulted to `./templates/` | Java operator with no explicit config fails to boot on Rust. |
| H06 | Unknown URL schemes (mysql, mariadb, jdbc) rejected | Java uses whatever `driverClassName` says | Java operator with MySQL driver fails. |

---

## Section I — Test fixtures (§1.3 preview)

| # | Java fixture | Ported to Rust? | Notes |
|---|---|---|---|
| I01 | `src/test/resources/application.yml` (H2 crm+debt config) | No | Port as `compat/` fixture. |
| I02 | `src/test/resources/init-crm-db.sql` | No | Rust uses Liquibase changelog for PG; port as SQLite equivalent for compat suite. |
| I03 | `src/test/resources/init-debt-db.sql` | No | Same. |
| I04 | `src/test/resources/templates/crm/*.sql` | No | See C11 — layout mismatch. |
| I05 | `src/test/resources/templates/debt/*.sql` | No | Same. |
| I06 | `docker-compose.yml` (PG + service) | Rust has its own | Cross-compare fixtures. |
| I07 | Java integration test cases | No equivalent test naming in Rust | Not one-to-one per R4.2. |

---

## Section Z — Coverage summary

**Java surface areas (17 config + 5 routes + 12 SQL-format + 12 response + 9 error + 6 log + 3 routing = ~64 rows)**

| Status | Count | %  |
|---|---|---|
| MATCH | ~25 | 39% |
| DIVERGE | ~26 | 41% |
| MISSING | ~8 | 13% |
| NOT-VERIFIED | ~3 | 5% |
| ⚠️ partial | ~2 | 3% |

**Compliance verdict:** NOT compliant with REFACTO-REQUIREMENTS.md §2.1 / §2.3 as currently shipped. To comply with user directive ("what was there before must remain working"), the DIVERGE + MISSING rows must each be:
1. Fixed (Rust made compatible with Java surface via alias/backport), OR
2. Rejected loudly at boot (§2.1 clause 2 — clear diagnostic naming the field), OR
3. Documented in DIVERGENCES.md with rationale AND a boot WARN (§2.1 clause 3).

Silently ignoring old fields (`headers.*`, `userIP*`, `spring.*`, `h2.*`) is FORBIDDEN by §2.1. With current `deny_unknown_fields`, Rust actually rejects them loudly — that's §2.1-compliant, but the operator sees only a serde error, not a migration hint. §6.2 says the operator should be able to boot-log-read to determine what they're depending on. That's the gap.
