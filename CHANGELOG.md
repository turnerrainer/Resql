# Changelog

All notable changes to this project will be documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

### Added (R8)

- **Optional built-in rate limiter.** New config block
  `rate_limit: { requests_per_second: u32, burst: u32 }` (default
  `requests_per_second: 0` — disabled; the middleware isn't attached
  and adds zero cost per request). When enabled, a global
  process-wide token bucket caps sustained throughput at `rps` with
  bucket size `burst` (auto-sizes to `2 * rps` when `burst: 0`).
  Exhaustion returns `429 Too Many Requests` in the canonical
  header-envelope shape (`X-Resql-Error-Code: TooManyRequestsException`)
  plus an RFC 6585 `Retry-After` header. Health probes bypass the
  bucket so LB liveness isn't affected by rate saturation. Scope is
  deliberately global rather than per-IP: Resql almost always sees a
  reverse proxy's IP, so per-IP buckets collapse to a single-key
  cache. Operators who need per-caller rate limits should apply them
  at the proxy where real client IPs are visible. R8.

### Security (v1 log-attack pass — h2ck.me)

- **User-controlled fields that land in WARN log lines and error
  response headers are now length-capped and CRLF-stripped.** FN-LOG-1
  and FN-LOG-2 in the v1 log-attack pass: a caller sending a 100 KB
  JSON key or an absurdly long URL path would produce a 100 KB WARN
  line and a 100 KB `X-Resql-Error-Message` header (cheap DoS for the
  log store and rejected on size by some proxies). Now every path
  where user input flows into a log-line or response-header goes
  through the new `truncate_for_log` helper (cap at 1 KB with a
  `[truncated N bytes]` marker), and CR/LF stripping runs before the
  message hits the log template. Sanitiser for
  `X-Resql-Error-Message` also caps at 1 KB with `...` marker so a
  compliant peer / proxy will never reject the response on header
  size. Server request logs already used `sanitize_log_value` on the
  route — a per-line truncate to 512 chars has been added for the
  same reason.

### Security (v1 runtime break-test — h2ck.me)

- **Shipped `resql.yaml` no longer contradicts the CHANGELOG's "closed
  by default" copy.** Prior versions carried `allow_datasource_header:
  true` (with no allowlist — every override 403s anyway) and
  `cors.allowed_origins: "*"` (browser drive-by lane). The shipped file
  now matches the code-level safe defaults: header routing left off,
  CORS empty. Operators who need either lane opt in explicitly.
  Additionally, the boot log emits one WARN per remaining permissive
  knob (`security_warnings()`), starting with a "wildcard CORS opens
  every origin" line whenever `allowed_origins == "*"`. FN1.
- **Shipped `docker-compose.yml` now runs with `read_only: true`.** The
  container rootfs is immutable — combined with the existing
  `cap_drop: ALL` and `no-new-privileges` the compose file already set,
  an RCE inside the resql process can no longer drop a payload
  anywhere on disk. Previously `read_only` was commented out with a
  note about SQLite needing to write to its DB file; the in-memory demo
  doesn't need that, and file-backed SQLite users are now pointed at
  the volume-mount pattern (write only to `/var/lib/resql/data`) so the
  rest of the filesystem stays sealed. FN4.
- **`/openapi.json` is now 404 by default; enable with
  `admin.openapi_public: true`.** The endpoint previously returned the
  full spec unauth — every registered SQL endpoint, every declared
  parameter, every response shape — giving a network-reachable
  attacker a free catalogue of the surface to probe. Now it follows
  the same env-gate posture as `/datasources`: default off,
  indistinguishable from a non-mounted route, opt-in per deployment.
  Operators who need the spec (dev, staging, or behind a same-origin
  reverse proxy that authenticates before it hits Resql) set
  `admin.openapi_public: true`. FN3.
- **413 (body too large) and 504 (request timeout) responses now use
  the same header-envelope shape as every other error.** The two
  layers `tower-http` provides — `RequestBodyLimitLayer` and
  `TimeoutLayer` — emit their responses outside the router, so they
  previously bypassed the ResqlError envelope: 413 was bare
  `text/plain` "length limit exceeded", 504 was an empty body without
  the two `X-Resql-Error-*` headers. Downstream DSLs had to
  special-case those two paths. A new outer middleware reshapes any
  non-2xx response missing `X-Resql-Error-Code` into the canonical
  envelope: body swapped to `[]`, headers set to
  `PayloadTooLargeException` / `RequestTimeoutException` (or the
  status's canonical name for other 4xx/5xx cases). Idempotent —
  ResqlError responses already carry the envelope and are untouched.
  FN5.
- **Method mismatch on an existing saved query now returns 405 with an
  `Allow:` header** (RFC 7231 §7.4.1). Previously `GET /users/echo`
  when only `POST /users/echo.sql` was registered returned 400
  `ResqlRuntimeException: Saved query '/users/echo' does not exist` —
  misleading, since the path IS registered under a different method.
  Callers now see 405, `X-Resql-Error-Code: MethodNotAllowedException`,
  and `Allow: POST` (or `GET, POST` when both variants exist). A GET
  or POST to a truly unregistered path is unchanged (still 400
  `ResqlRuntimeException / QueryNotFound`). FN6.

### Added (fleet stronghold §8.2)

- **`resql doctor` — pre-boot health check subcommand.** Parses the
  config, runs the same safety checks `serve` runs (semantic
  validation + `validate_runtime_posture` from fleet §3.1), and prints
  compat-shim diagnostics in the same order they'd land in the boot
  log. Never binds a port, never opens a datasource pool — deliberately
  side-effect-free so ops teams can point it at a candidate config in
  staging without disturbing anything. Exit-code contract: 0 = clean,
  1 = hard error (parse failure or runtime-posture refuse), 2 =
  warnings only with `--strict`. Wire into CI as a blocking gate
  (`--strict`) or a non-blocking check (default). The `Command`
  wrapper defaults to `serve` when no subcommand is given, so existing
  invocations of `resql -c resql.yaml` still work unchanged.

### Added (fleet stronghold §3.1)

- **Boot refuses to start on a non-loopback bind without an
  authentication story.** New `security.trust_network: bool` opt-out.
  Boot fails fast (with an actionable message listing all three
  satisfying postures) when `server.bind` is non-loopback AND
  `security.inter_service_token_env` is unset AND
  `security.trust_network` is not `true`. Semantic config parsing is
  unchanged — the new check runs from `main.rs::validate_runtime_posture`
  after parse, so compat-shim / fixture parses still succeed; only
  the actual boot refuses. Closes the "public bind, no auth, oops"
  failure mode the F-RES-3 finding flagged as the worst-case shape.

### Added (v1 runtime break-test — h2ck.me)

- **Optional inter-service bearer gate.** New config field
  `security.inter_service_token_env: <ENV_VAR_NAME>`. When set, the
  named env var's value becomes the required `Authorization: Bearer …`
  token for every request except `/health` and `/healthz`; missing or
  mismatched tokens return 401 `UnauthorizedException` with the
  canonical header-envelope shape. Token comparison is constant-time on
  equal-length inputs (length mismatch rejects up-front — token length
  is not a secret). Boot refuses to start if the env var is unset or
  empty, so a wired-but-unset gate can't silently disable itself. The
  gate is OFF by default — Resql's design is "internal-only behind
  Ruuter" and operators must opt in per deployment. Closes the F-RES-3
  finding: a Resql that becomes network-reachable outside its intended
  trust boundary (dev, staging with a misconfigured proxy, or an
  unguarded Ruuter DSL) is no longer one `curl` away from unauth SQL
  execution against every registered datasource.

### Added (fleet stronghold §8.1)

- **Extended boot-time WARN catalogue.** The `Config::security_warnings`
  entry seeded by FN1 now covers four additional permissive knobs:
  non-loopback `server.bind` without an auth gate; `admin.datasources_public: true`
  (topology leak on an unauth endpoint); `logging.print_stack_trace: true`
  (schema-name leak in error chains); any datasource carrying a
  plaintext `password:` (Java-compat form; steer to `password_env`).
  Each check emits one WARN at boot with the field name and the
  actionable "fix by …" hint — matching the TIM / CronManager reference
  pattern from the fleet stronghold guidance.

## [0.3.0-alpha] - 2026-09-10

**Minor bump because the error-response wire shape changed.** Configs
are unaffected; callers that read `body.error` / `body.message` must
switch to the `X-Resql-Error-Code` / `X-Resql-Error-Message` response
headers. HTTP status and exception-class identifiers are unchanged.
See [`CLAUDE.md`](https://github.com/turnerrainer/Resql/blob/dev/CLAUDE.md#post-020-alpha-breaking-change-breaking-for-callers-not-configs)
for the operator-facing short list and DIV-022 in `DIVERGENCES.md`
for the rationale.

### Changed (BREAKING)

- **Error responses on query endpoints now use an empty-array body +
  headers envelope.** Body is always `[]` on any non-2xx from
  `/:project/:tail` (or the batch shape); the same information — the
  Java-canonical exception class name and the human-readable message —
  moves to the response headers `X-Resql-Error-Code` and
  `X-Resql-Error-Message`. HTTP status and the exception-class
  identifiers are unchanged; the message header is sanitised to
  printable ASCII (CR/LF stripped, non-printable → `?`) and the full
  un-sanitised text stays in the server log for the request's trace
  id. Fixes a whole class of silent fail-open bugs in downstream DSLs
  that branched on `body.length > 0` — with the old object body,
  `.length` was `undefined` on error, which many DSL evaluators coerce
  to `false`, silently routing DB failures into a "not-found" branch.
  Callers that used to read `body["error"]` / `body["message"]` must
  now read the two response headers. Issue #25. See DIV-022 in
  `DIVERGENCES.md`.

### Fixed

- **`password_env` no longer silently defeated when the datasource URL
  already carries a `user:pw@` component.** Config-supplied credentials
  (from `password_env` or the Java-compat `password` field) now override
  any userinfo embedded in the URL, and a WARN log line names the
  collision so an operator who rotated the URL's password without
  clearing `password_env` notices that the env variable is now the
  source of truth. Previous behaviour returned the URL unchanged,
  which turned `password_env` into a decoration and let stale
  URL-embedded passwords silently take effect. Issue #27.
- **Postgres user-defined `ENUM` columns (and enum arrays) are now
  auto-coerced to their text label on read.** Previously a bare
  `SELECT status FROM t` where `status` is a project-defined enum
  returned JSON `null` — sqlx's built-in `String` decoder isn't
  compatible with the enum's dynamic OID, so every branch of the row
  materialiser fell through. Callers had to remember `::text` on every
  enum SELECT or lose data. The row materialiser now inspects
  `PgTypeInfo::kind()` and decodes `PgTypeKind::Enum` and
  `PgTypeKind::Array(Enum)` columns via their raw wire form, since
  Postgres transmits enum values as UTF-8 labels. Issue #26.

## [0.2.0-alpha] - 2026-09-06

Security release. Closes the [h2ck.me](https://github.com/h2ckme) v1 pre-publication audit (findings R1–R7 + R9, all ✅). **MINOR bump because config defaults were flipped in ways that will make some existing `0.1.x-alpha` configs behave differently or refuse to boot.** See the [README "Upgrading to 0.2.0-alpha"](https://github.com/turnerrainer/Resql/blob/dev/README.md#upgrading-to-020-alpha) table for the operator-facing short list and [`CLAUDE.md`](https://github.com/turnerrainer/Resql/blob/dev/CLAUDE.md#v1-security-audit-changes-breaking-for-existing-configs) for grep recipes + a paste-in Python auditor.

### Security (v1 pre-publication audit — h2ck.me)

- **`allow_datasource_header` now defaults to `false`, and every override
  is gated by a per-project allowlist.** The `X-Datasource` header
  previously let any caller route a request against a different
  registered datasource, opening a lateral-move lane inside the service
  trust boundary. New config field `datasource_header_allowlist:
  { <project>: [<allowed datasource names>] }` — an override that names
  a (project, datasource) pair not in this map now returns `403
  ForbiddenDatasourceOverrideException`. The rejection message names
  only the offender, not the set of registered datasources, so
  attackers cannot enumerate the registry by sending guesses.
  Operators who need header routing must set
  `allow_datasource_header: true` **and** populate the allowlist. (R1)
- **CORS defaults to closed.** `cors.allowed_origins` now defaults to
  `""` — the CORS layer is not attached at all, so no
  `Access-Control-Allow-Origin` header is emitted and browsers refuse
  cross-origin reads. Operators who need cross-origin must set it
  explicitly. (R2)
- **CORS methods and headers narrowed when configured.** When
  `cors.allowed_origins` is set, the layer now advertises only `GET`
  and `POST` (the methods the router actually serves) and only the
  request headers Resql actually reads (`content-type`,
  `authorization`, `x-datasource`, `traceparent`). Previously all
  methods and all headers were echoed back on preflight, expanding
  the drive-by surface a malicious origin could exploit alongside R2. (R3)
- **Loader now refuses symlinks in the SQL tree.** Every filesystem
  entry inside `sql_dir` is inspected with `symlink_metadata`; any
  symlink — file, directory, or broken — is skipped with a WARN log
  line and never opened. As defence-in-depth, each opened file's
  canonical path is asserted to still live under the canonical
  `sql_dir`, so an entry whose *parent chain* contains a symlink is
  also rejected. Previously an operator (or attacker) with write
  access to the SQL directory could plant `sql/prod/GET/leak
  -> /etc/passwd` and have Resql read the target as SQL. (R4)
- **`/datasources` now returns 404 by default; when enabled, output is
  redacted.** New config gate `admin.datasources_public: bool`
  (defaults to `false`) hides the endpoint entirely — unauth callers
  can't distinguish Resql from a service that never mounted it.
  Operators who need the endpoint set the flag to `true`, but even
  then the response is hardened:
  - `jdbcUrl` is redacted to `<scheme>://<host>[:port]` — path,
    query, and userinfo are all stripped. SQLite URLs redact down to
    just `sqlite:` so file paths don't leak deployment topology.
  - `username` is always the empty string. The real value stays in
    the operator's startup logs only.
  Startup emits one `datasource connected` INFO line per pool with the
  masked-password URL so operators can still verify connection
  topology without exposing it on the wire. (R5)
- **Batch endpoint errors no longer leak underlying SQL detail.**
  Previously a failing `POST /:project/:path/batch` returned the raw
  driver error text — which for Postgres includes constraint,
  column, and table names, and for SQLite includes messages like
  `UNIQUE constraint failed: t.login`. Attackers could iterate a
  batch to probe schema. Now the caller sees
  `Batch failed at statement N of M, rolled back` (with the failing
  position and total, but no driver detail). The full underlying
  error is logged at WARN so operators can still diagnose. HTTP
  status and `error` field are unchanged (`400`,
  `BadSqlGrammarException`) so existing callers that only branch on
  status still work. (R9)

### Reliability (v1 pre-publication audit — h2ck.me)

- **`server.request_timeout_seconds` is now enforced.** Previously the
  config field existed but no middleware honoured it, so a caller
  could send `SELECT pg_sleep(3600)` and pin a pool connection for
  the full hour — with `max_connections: 10` (default) it took 10
  such queries to lock every legitimate caller out for the pool's
  `acquire_timeout_seconds`. Now:
  - A `TimeoutLayer` (from `tower-http`) wraps every route and
    returns **504 Gateway Timeout** when the deadline elapses.
    Cancellation drops the future, releasing the pool connection.
  - Every Postgres pool connection now runs
    `SET statement_timeout = <request_timeout_seconds>` in its
    `after_connect` hook, so a query whose future was already
    cancelled still gets killed at the server side rather than
    running to completion in the background.
  - `request_timeout_seconds: 0` is now rejected at config validation
    time. A zero value would silently disable the timeout and re-open
    the pool-exhaustion attack lane. (R6, also folds in R7.)

## [0.1.2-alpha] - 2026-09-03

Correctness release. Fixes issue #11 (declared `items.type` now drives array element validation and binding — empty typed arrays into native `text[]` columns work end-to-end) plus a batch of audit-cycle hardening the fix uncovered: silent-null decode bugs for `float8[]` / `bool[]` / `uuid[]` / `date[]` / `timestamptz[]` / `numeric[]` / `jsonb[]` columns, strict scalar `uuid` / `date` / `datetime` format validation, boot-time coerce checks for declared defaults + enum entries, and nested `items:` support for arrays-of-arrays. Every fix has an end-to-end integration test against a real Postgres column; the audit exposed and fixed several latent silent-data-loss bugs that hadn't been reported yet.

Also verifies issue #10 with a regression test hammering the reporter's exact SQL shape across mixed input flavours on a pinned single connection — passes on the current codebase, so the underlying fix already shipped in [#9](https://github.com/turnerrainer/Resql/pull/9) (0.1.1-alpha). Callers still seeing that flake should upgrade to ≥ 0.1.1-alpha.

### Fixed
- **Empty typed arrays (`{"xs": []}` with `items: {type: string}` declared) could not be inserted into a native `text[]` column.** `bind_pg_array` used only the runtime JSON heuristic (`detect_pg_array_kind`) which gave up on empty and all-null arrays, falling back to a JSONB bind that Postgres refused for a `text[]` target. Fixed by teaching the bind path to consult the declared `items.type` first: empty / all-null arrays now bind as the correct native array (`text[]`, `int8[]`, `float8[]`, `bool[]`, `uuid[]`, `date[]`, `timestamptz[]`) instead of JSONB. The runtime heuristic remains as the fallback for legacy declarations with no `items:` block. ([#11](https://github.com/turnerrainer/Resql/issues/11))
- **`float8[]` and `bool[]` columns silently decoded as JSON `null` instead of their value.** `pg_column_value` only had branches for text-family and integer-width array types; every other array flavour fell through the `ty_upper.ends_with("[]")` catch-all, tried a `Vec<i64>` decode that failed, then landed in the last-resort text probes (which also fail on non-text arrays) and came back as `null`. Fixed by adding decode branches for `_FLOAT4` / `_FLOAT8` / `_BOOL` ahead of the catch-all. Latent silent-data-loss bug — the fix for issue #11 exposed it via new end-to-end tests. Related: also added decode branches for `_UUID`, `_DATE`, `_TIMESTAMP`, `_TIMESTAMPTZ`, and `_TIME` arrays (same failure mode, different types).
- **`numeric[]`, `json[]`, and `jsonb[]` columns silently decoded as JSON `null`.** Same silent-loss path as the `float8[]` / `bool[]` bug — no decode branch, fell through the int8[] catch-all, came back as null. Fixed: NUMERIC[] elements stringify each (precision preservation, matches scalar NUMERIC treatment); JSON[]/JSONB[] elements unwrap each `sqlx::types::Json` wrapper so the response holds the actual JSON value at each slot rather than a stringified copy. `_JSONB` decode ordering matters — `starts_with("_JSON")` also matches `_JSONB`, so the more-specific branch runs first.

### Added
- **Per-element type enforcement for array parameters with declared `items.type`.** Wrong-typed elements now reject at the request boundary with `xs[idx]: expected <type>, got <actual>` instead of being silently coerced to `SQL NULL` or bound into a heuristic-picked wire type that mismatches the target column. Element-level coercion runs the same coerce rules as the scalar path, so string-encoded numbers still pass for `items: {type: integer}` (mirrors GET query-string arrival). ([#11](https://github.com/turnerrainer/Resql/issues/11))
- **Native binding for `uuid[]`, `date[]`, and `timestamptz[]` typed arrays.** Declaring `items: {type: uuid|date|datetime}` now binds the matching native Postgres array type — callers no longer need `::uuid[]` / `::date[]` / `::timestamptz[]` casts in the SQL. Every element is format-validated up front (`uuid::Uuid::parse_str`, `NaiveDate::parse_from_str`, RFC 3339 + naive ISO 8601 fallbacks for datetime), so bad literals surface as 400 responses naming the failing element instead of a cryptic bind-time or Postgres-side error. `datetime` elements are UTC-anchored (matches how `pg_column_value` renders `TIMESTAMPTZ` back).
- **Recursive per-element validation for nested typed arrays.** `ItemType` now carries its own optional `items:` block so a declaration like `items: {type: array, items: {type: integer}}` validates grand-child elements — a mistyped element in `[[1, "two"]]` surfaces as `xs[0][1]: expected integer, got string`. The wire binding for nested arrays stays JSONB (Postgres arrays are physically flat, no native "array of array" type), but the validation path is now recursive to arbitrary depth. OpenAPI emission likewise nests `items` schemas to match.
- **Boot-time rejection of `items:` on non-array parameter types.** `type: string, items: {type: integer}` is nonsensical and was previously silently ignored at runtime; now fails at load with `param 'x': 'items' is only valid on 'type: array'`.
- **Boot-time coerce validation for declared `default:` values.** Every `default:` now runs through the same coerce pipeline the runtime uses (`coerce_to` + per-element `coerce_element`), so a misdeclared default (`default: "not-a-number"` on `type: integer`, or `default: [1, 2]` on `items: {type: string}`) fails at load instead of silently corrupting the request path when a caller happens to omit the param and trigger the default fallback.
- **Strict format validation for scalar `uuid` / `date` / `datetime` params.** Bad literals now surface at the request boundary as `400 InvalidParameterTypeException` naming the failing param, instead of continuing to Postgres and coming back as a `BadSqlGrammarException` from the server-side cast. Uses the same `validate_semantic_format` helper the array-element path uses, so scalar and array behaviour are unified. **Wire binding is unchanged** — scalar semantic types still bind as text (Postgres implicit-casts on assignment), so no OIDs shift and existing SQL that relies on the text-binding shape (e.g. `WHERE id::text = :id`) keeps working. Well-typed callers see no change; only clients sending malformed literals are affected, and they get a clearer error sooner.
- **Boot-time semantic-format validation for `enum:` entries.** `declaration::validate_enum_shapes` asserts the type *family* of each enum entry, which for `type: uuid|date|datetime` reduces to "is a string" — it can't distinguish a valid UUID from a garbage string. `type: uuid, enum: [..., "not-a-uuid"]` therefore loaded fine but shipped a dead entry no valid request could ever match (the runtime format check would reject the caller's payload before the enum lookup). New boot pass now format-validates each enum entry so declaration authors see the mistake at load rather than never.
- Regression tests exercising the reporter's exact `LIMIT COALESCE(:limit::int, ...) OFFSET COALESCE(:offset::int, ...)` shape with mixed input flavours (default fallback / string-coerced / native numeric / null) across sequential calls on a pinned connection, plus a 40-iteration stress variant. All pass on the current codebase — the underlying cache-stable-OID fix landed in [#9](https://github.com/turnerrainer/Resql/pull/9), so anyone still seeing the `invalid byte sequence for encoding "UTF8": 0x80` flake from [#10](https://github.com/turnerrainer/Resql/issues/10) should upgrade to ≥ 0.1.1-alpha. ([#10](https://github.com/turnerrainer/Resql/issues/10))

### Changed (behaviour tightening)
- Callers who were previously sending array payloads that violated a declared `items.type` (e.g. numeric elements against `items: {type: string}`) will now receive a `400 InvalidParameterTypeException` instead of the previous heuristic-picked native-array bind (which typically failed later with a Postgres type-mismatch error, or silently succeeded with the wrong element type). The declaration is now the strict contract; per-element mismatches are rejected at the request boundary. **No callers who were sending well-typed payloads are affected.**
- Callers sending malformed `uuid` / `date` / `datetime` scalar literals against a param typed accordingly will now receive `400 InvalidParameterTypeException` at the request boundary instead of a Postgres-side `BadSqlGrammarException`. Accepted formats: `uuid` — anything `uuid::Uuid::parse_str` accepts (hyphenated / unhyphenated / braced hex); `date` — strict `YYYY-MM-DD`; `datetime` — RFC 3339 with any offset (normalised to UTC), or naive ISO 8601 (`YYYY-MM-DD[T| ]HH:MM:SS[.f]`) treated as UTC. Callers previously relying on Postgres's more permissive date-string parser (e.g. `'July 1, 2026'`, `'20260701'`) will need to send strict ISO 8601 instead. **Well-formed literals see no change.**

## [0.1.1-alpha] - 2026-08-27

Bug-fix release. Four Postgres correctness fixes that were silently returning `null` or corrupting the connection in the previous alpha. Also: **versioning scheme shift** — from now on the alpha series increments PATCH per release (`0.1.1-alpha`, `0.1.2-alpha`, …) instead of an alpha counter under a single target version (`0.1.0-alpha.N`). RC series will begin at `1.0.0-rc.1`; there will be no stable `0.1.0` release.

### Fixed
- **`bind_pg` now types every Postgres parameter by its declared `ParamType`, not the incoming JSON shape.** sqlx caches prepared statements per SQL text on a connection: the first `Parse` fixes each `$N` slot's OID, and every later execution against that cached statement must bind the same OID or Postgres re-interprets the wire bytes under the cached type (e.g. an `i64`'s binary bytes read as text → `invalid byte sequence for encoding "UTF8": 0x00`). Previously `bind_pg` bound `Value::Null` as `Option::<String>::None` (pinning the slot to text) but `Value::Number` natively as `i64`/`f64`, so any endpoint with an optional numeric param — `null` on one call, a real number on a later call, same connection — corrupted the request with a `BadSqlGrammarException`. The fix drives the bind Rust type off `DeclaredParam.ty`, so both null and non-null bindings for the same param use the same OID regardless of the JSON value's shape; existing SQL like `WHERE id = :id` (native i64 bind, no explicit cast) keeps working. Diagnosis and reproduction (`pg_number_after_null_on_same_cached_statement_does_not_corrupt`) originally by @Aljoxa88 in [#7](https://github.com/turnerrainer/Resql/pull/7).
- **Multi-byte UTF-8 characters could be corrupted by the named-parameter rewriter.** `rewrite_named_params` walked the SQL text one byte at a time and cast each byte to `char` individually (`bytes[i] as char`). For any non-ASCII character encoded as 2–4 UTF-8 bytes (e.g. accented Latin like `Ä`/`õ`/`ü`, or Cyrillic), this reassembled the wrong Unicode scalar value byte-by-byte instead of treating the sequence as one character — silently corrupting the SQL text inside line comments (`-- ...`), block comments (`/* ... */`), string literals, and even bare SQL (e.g. a column alias), rather than raising an error. Fixed by detecting non-ASCII leading bytes and copying the whole UTF-8 sequence through verbatim (`copy_utf8_char`). Only affects SQL files containing non-ASCII text; purely-ASCII SQL is unaffected. Added 4 regression tests covering line comments, block comments, string literals, and bare SQL. ([#5](https://github.com/turnerrainer/Resql/pull/5))
- **`TIME` (without time zone) columns silently came back as `null` instead of their value.** `pg_column_value`'s timestamp/date branch tried `NaiveDateTime`, `DateTime<Utc>`, and `NaiveDate` decodes, none of which match a bare `TIME` column, and the subsequent fallbacks (`try_get::<Option<String>>`) also fail to decode a Postgres `TIME` value as a Rust `String` -- so a non-NULL `TIME` value was misreported as JSON `null` rather than erroring or returning the actual time. Fixed by adding a `chrono::NaiveTime` decode attempt alongside the existing date/timestamp ones; `TIME` columns now come back as `"HH:MM:SS[.ffffff]"` strings, consistent with how `DATE` is already rendered. Added a regression test (`pg_naked_time_column_decodes_as_string`). ([#6](https://github.com/turnerrainer/Resql/pull/6))
- **`smallint[]`/`integer[]` and `varchar[]`/`bpchar[]`/`char[]`/`name[]`/`citext[]` array columns silently came back as `null`.** `pg_column_value` only ever attempted `Vec<String>` for `TEXT[]` and a single `Vec<i64>` for every integer array type; sqlx requires the exact Rust integer width to match the Postgres array's element width (`int2[]` only decodes as `Vec<i16>`, `int4[]` only as `Vec<i32>`, `int8[]` only as `Vec<i64>`), so only `bigint[]`/`int8[]` columns ever actually decoded -- `smallint[]` and `integer[]` silently fell through every branch to `null`. Likewise, only `TEXT[]` was covered by the text-array branch; `VARCHAR[]`, `BPCHAR[]`/`CHAR[]`, `NAME[]`, and `CITEXT[]` arrays fell through the same way. Fixed by widening both branches to cover the full character-type family (text-as-`Vec<String>`) and each integer width (`int2[]`/`int4[]`/`int8[]`, all widened to `i64` for JSON). Added 5 regression tests covering `smallint[]`, `integer[]`, `varchar[]`, and `char[]` (plus a guard test confirming `bigint[]`, which already worked, keeps working). ([#8](https://github.com/turnerrainer/Resql/pull/8))

## [0.1.0-alpha.4] - 2026-08-26

### Added — operator-grade logging (parity with Ruuter-on-Rust)
- **Per-request access log** — one INFO line per completed request with OpenTelemetry HTTP semantic-convention fields: `http.request.method`, `http.route`, `http.response.status_code`, `duration_ms`, `resql.project`, `trace_id`. Toggle via `logging.access_log` (on by default).
- **W3C `traceparent` propagation** — adopted verbatim from the caller, generated server-side when absent. Every request-scoped log line inherits the same 32-hex `trace_id`; the id is echoed back to callers via `X-Trace-Id` on every response (including 4xx/5xx). Correlate client and server logs without extra headers.
- **Structured error logs** — every `ResqlError` returned as HTTP 400+ now emits a WARN (or ERROR for 5xx) line with `error.kind` (Java-canonical exception name) alongside the message. Same `trace_id` as the request span.
- **CRLF-safe log values** — `logging::sanitize_log_value` runs on user-controlled fields before they hit a log line. Blocks log-line splicing via header or body payload.
- **Body redaction toolkit** — `logging::redact::redact_json` / `redact_headers` replace configured field names with `"[REDACTED]"` (case-insensitive, at any depth). Defaults cover `password`, `token`, `authorization`, `api_key`, etc. New config keys: `logging.redact_body_fields`, `logging.max_body_bytes`, `logging.print_stack_trace`.
- **Env-var overrides** — `RESQL_LOG_FORMAT=text|json` overrides `logging.format` at runtime so an operator can flip a running container without editing config. `RESQL_LOG` continues to override the level directive.
- **JSON output** — the JSON layer now emits current-span context so downstream log stores see the request's `trace_id` / `resql.project` on every event.
- Health probes (`/health`, `/healthz`) are excluded from the access log.
- New chapter: `book/src/logging.md` — field vocabulary, config reference, redaction semantics, correlation recipes.

## [0.1.0-alpha.3] - 2026-08-26

### Added
- **Closed-set input validation via `enum:` on declared params.** Any `DeclaredParam` can carry a JSON-Schema-style `enum: [...]` list; values outside the set are rejected at the request boundary with 400 `InvalidParameterValueException` before any SQL binding. Enum entries are validated at boot against the declared `type`; `default:` (when set) must be in the enum or `null`; empty lists are rejected at boot. The set is echoed in the OpenAPI spec as JSON Schema's `enum` keyword so code-gen clients see the true type. Closes the input-side security half of [Resql#3](https://github.com/turnerrainer/Resql/issues/3) — no SQL is rewritten; the DB never sees an out-of-set value.

### Fixed
- **Timestamp serialisation is now ISO 8601 / RFC 3339 by default.** Postgres `TIMESTAMP` columns now render as `2026-01-01T10:20:30` (previously `2026-01-01 10:20:30` — space separator, neither ISO 8601 nor RFC 3339); `TIMESTAMPTZ` columns at UTC render as `2026-01-01T10:20:30Z` (previously `2026-01-01T10:20:30+00:00`). Matches Jackson / JVM Resql defaults. Fixes [Resql#3](https://github.com/turnerrainer/Resql/issues/3).

### Added — mandatory declaration section + OpenAPI 3.1 (task 008)

- **Every `.sql` file now opens with a `/* … */` YAML declaration block** naming its parameters, their types, whether each is required, and (optionally) the returned row shape. The block body is plain YAML — no per-line prefix, so authors can paste YAML from any editor. Boot refuses any file without a declaration, any declaration that fails to cover every `:name` in the SQL, and any orphan declared params. See `book/src/declarations.md`.
- **Optional parameters may be omitted from requests** — the SQL sees SQL NULL for the missing placeholder (or a declared `default:`). Fixes [Resql#4](https://github.com/turnerrainer/Resql/issues/4): callers no longer have to send explicit `null` for every optional filter on every request.
- **Type-safe request validation.** Requests now hit three declaration-driven boundaries before touching the DB: unknown key → 400 `UnknownParameterException`; wrong type → 400 `InvalidParameterTypeException`; required missing → existing 400 `InvalidDataAccessApiUsageException`. GET query-string values are coerced to the declared type at the request boundary.
- **OpenAPI 3.1 spec exposed at `/openapi.json`.** Generated at boot from every declaration; paths and operations are alpha-sorted so a diff on the file is meaningful across restarts. POST endpoints get an auto-emitted `/…/batch` variant. Customise `info` and `servers` via a new optional `openapi:` block in `resql.yaml`.
- **Type set:** `string`, `integer`, `number`, `boolean`, `array`, `object`, `date`, `datetime`, `uuid`. Semantic types (`date`, `datetime`, `uuid`) emit as `string` with the standard OpenAPI `format`.
- New `book/src/declarations.md` chapter; `DIVERGENCES.md` entries DIV-019 through DIV-021 documenting the intentional break from JVM Resql's permissive request handling.

### Changed

- **Breaking.** All existing `.sql` files require a declaration to load. Minimum viable declaration for a file using `:a` and `:b`: `/*\nparams:\n  a: { type: string }\n  b: { type: string }\n*/`. Extras (name matches, default values, returns schema) are recommended.
- `query::execute`, `query::execute_transactional`, and `query::execute_batch` now take an extra `&Declaration` argument; the runtime validates every request map against it before binding.

## [0.1.0-alpha.2] - 2026-08-05

### Added — batch atomicity and array binding (task 007)
- **`/batch` is now atomic.** The full batch runs inside one database transaction; any failure rolls the whole batch back with the standard 400 response. Missing-parameter checks run against every set BEFORE the tx opens for fail-fast. Supersedes the batch-atomicity part of task 003.
- **Native Postgres array binding.** Homogeneous scalar JSON arrays bind as native `text[]` / `int8[]` / `float8[]` / `bool[]` so a single SQL statement using `unnest()` handles the whole set in one round-trip. Mixed / nested / all-null / empty arrays fall back to JSONB with a `debug`-level log. Nulls inside a homogeneous array stay as SQL NULL elements (via `Vec<Option<T>>`). SQLite continues to bind arrays as JSON strings — use `json_each()` to unpack.
- New book sections in `sql-files.md`: atomicity guarantee for `/batch`, native array parameters, per-file `@transactional` marker. `failure-modes.md` entry documenting batch rollback semantics.

### Added — per-file `@transactional` marker (task 003)
- SQL files with `-- @transactional` in the leading comment block execute inside a single database transaction (commit on success, rollback on any error). Recognised only before the first non-comment line; applies to both GET and POST endpoints (primary use case: multi-statement POST files).

### Added — Java compatibility layer
- `src/config_compat.rs`: reads Java `application.yml` (and `application-{prod,dev,test}.yml`) at boot, translates Spring-shape keys to Resql config, and records diagnostics for anything unsupported so operators know exactly what won't carry over.
- Auto-discovery of Java-style config paths at boot: `/app/resql.yaml`, `./resql.yaml`, `./application.yml`, `./application-{prod,dev,test}.yml` — first hit wins.
- `/datasources` response uses Java-canonical camelCase (`jdbcUrl`, `driverClassName`) so existing Spring-era dashboards keep working. Driver class inferred from the pool type.
- `/health` shape extended with Java-canonical fields; `/healthz` alias preserved.
- Boot logs every captured compat diagnostic per Java §6.2 so operators can determine unsupported-feature dependencies from a single log read.

### Added — Postgres integration test suite (task 006 — actually shipped in alpha.1, retroactively documented here)
- `tests/integration_postgres.rs` — now 24 tests covering type mapping (JSONB, TIMESTAMPTZ, NUMERIC, BOOLEAN, DATE), snake→camel columns, INSERT+RETURNING, batch endpoint, SQL errors, password masking, and the new atomicity + array binding + transactional-marker behaviours. Skips silently without `TEST_POSTGRES_URL`.
- Liquibase-managed schema and test fixtures (`db/changelog/master.yaml` + `001-schema.yaml` + `002-test-fixtures.yaml`). Test-only data is gated on `context: test` so production applications never see it.
- CI (`tests.yml`): Postgres 16 service container + Liquibase update step (`--contexts=test`) on both amd64 and arm64 matrix rows.
- `Makefile` with `pg-up` / `pg-schema` / `test-pg` / `test-all` targets for local Postgres development.
- New book chapter `book/src/postgres-setup.md` covering the Liquibase pattern, test workflow, and deployment recipes (init container + one-shot job).

### Added — reference-shape regression tests
- `tests/integration_reference_shapes.rs` + `tests/fixtures/*.json` — golden-file assertions that `/health`, `/datasources`, and error responses keep the Java-canonical field set that Spring-era clients depend on.
- `tests/integration_java_compat.rs` — end-to-end tests for the compat translator (config parsing, diagnostic capture, health/datasources shape).
- Docs: `docs/DIVERGENCES.md`, `docs/PORTING.md`, `docs/MIGRATION.md`, `docs/REFACTO-DEVIATIONS.md`, `docs/audits/2026-08-04-spec-compliance.md` — explicit inventory of what differs from JVM Resql and why.

### Changed
- Postgres testing promoted from backlog task 006 to a first-class CI requirement. The runtime image still ships without Liquibase or a JVM — schema is applied out-of-band.
- `query::execute_batch` and `query::execute_transactional` are the new atomicity entry points; the classic `query::execute` still works for non-transactional single-shot queries.

### Fixed
- Postgres INT4 columns now decode correctly (previously fell through to `null` because the extractor only tried `i64`; sqlx-postgres decodes INT4 as `i32`). Surfaced by the new Postgres suite.
- Postgres NUMERIC columns preserve full precision as a JSON string (previously `null` because the required `rust_decimal` sqlx feature was off).

## [0.1.0-alpha.1] - 2026-07-29

Initial Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service. Interface-compatible with the original for the SQL-file-to-endpoint,
`:named`-parameter, and snake→camel result semantics.

### Added

- SQL file loader (`sql/<project>/<GET|POST>/<name>.sql` → `<METHOD> /<project>/<name>`).
- Named-parameter binder with awareness of string literals, comments, and Postgres `::` casts.
- Repeated-parameter support: `SELECT :x, :x, :y` binds `x` once.
- Multi-datasource routing with three-tier resolution:
  1. `X-Datasource` request header (when `allow_datasource_header: true`).
  2. `project_datasource_map` entry.
  3. Project name as datasource name.
- Postgres and SQLite pools (dispatched by URL scheme).
- Batch endpoint at `<POST-path>/batch`.
- Health endpoint (`/health` and `/healthz` alias).
- Datasource listing endpoint (`/datasources`) with password masking.
- CORS layer + configurable body-size cap (413 on overflow).
- Structured JSON error responses matching JVM Resql shape.
- Docker image (multi-stage, non-root, tini, self-contained demo).
- CI: tests (matrix amd64 + arm64), security (audit + deny + daily cron), publish (multi-arch, provenance, SBOM, cosign, Trivy), docs (mdBook to Pages).
- mdBook: introduction, getting-started, configuration, sql-files, failure-modes.

### Fixed (vs JVM Resql)

- Datasource-by-project routing (JVM version hardcoded a single datasource name — multi-database deployments were impossible without patching the source).
- Startup refuses to boot on any misconfigured datasource (JVM version silently ignored several).
- Passwords never appear in config file (JVM defaulted keystore password to `"123456"`).
- Request body cap prevents unbounded memory growth.

### Security

- `deny.toml` bans `openssl`, `openssl-sys`, `serde_yaml` (unmaintained).
- `cargo audit --deny warnings` runs on every push, PR, and daily cron.
- Container image signed with cosign keyless via GHA OIDC.
- Trivy HIGH/CRITICAL scan gates image signing.

[Unreleased]: https://github.com/turnerrainer/Resql/compare/v0.3.0-alpha...HEAD
[0.3.0-alpha]: https://github.com/turnerrainer/Resql/releases/tag/v0.3.0-alpha
[0.2.0-alpha]: https://github.com/turnerrainer/Resql/releases/tag/v0.2.0-alpha
[0.1.2-alpha]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.2-alpha
[0.1.1-alpha]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.1-alpha
[0.1.0-alpha.4]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.4
[0.1.0-alpha.3]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.3
[0.1.0-alpha.2]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.2
[0.1.0-alpha.1]: https://github.com/turnerrainer/Resql/releases/tag/v0.1.0-alpha.1
