# DIVERGENCES — Rust Resql vs. Java Resql (source of truth)

**Purpose.** REFACTO-REQUIREMENTS.md §5.1 requires one entry per intentional
behavioural difference from the source of truth. This file is that log.

**Source of truth.** The Java Spring Boot Resql at
`../Resql/` (upstream: <https://github.com/buerokratt/Resql>).

**Scope.** Only *changes* to Java-observable behaviour are recorded here.
Additive Rust features that do not alter existing Java surface (e.g.
`/health` alias, `X-Datasource` header override, `project_datasource_map`,
`logging.format: json`) are not divergences — they are extensions and live in
the coverage matrix under §8.3 "additions".

**Reversibility legend.** *low* = revertible with a one-file change;
*medium* = touches multiple modules; *high* = design implication, would
require re-architecture.

---

## DIV-001 — `/datasources` field names

- **Field / behaviour.** Response body field names.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/properties/DataSourceConfigProperties.java:9-18` and
  `Resql/src/test/java/rig/sqlms/controller/DataSourceControllerIntegrationTest.java:19-35` (STRICT JSONAssert).
  Java response: `{"name":..., "jdbcUrl":..., "username":..., "driverClassName":...}`.
- **Target behaviour.** As of this branch: identical shape (see
  `src/server.rs:67-91`). Prior alpha shipped `{"name", "url", "driver"}` — that
  was a regression, fixed here.
- **Motivation.** Java operators run dashboards / monitoring keyed on
  `jdbcUrl` and `driverClassName`. Match preserved.
- **Migration.** None required.
- **Reversibility.** low.

## DIV-002 — `/healthz` `packagingTime` presence, `version` format, additive `status`

- **Field / behaviour.** Response body of `/healthz` (and its Rust `/health` alias).
- **Source of truth.** `Resql/src/main/java/rig/sqlms/dto/HeartBeatInfo.java` and
  `Resql/src/main/java/rig/sqlms/service/HeartBeatService.java:27-44`. Java
  returns 5 fields: `appName`, `version` (format `v{MAJOR}.{MINOR}.{PATCH}`),
  `packagingTime`, `appStartTime`, `serverTime`.
- **Target behaviour.** Same 5 fields with same JSON keys and same shape, PLUS
  a Rust-only `status: "UP"` field. `packagingTime` is sourced from the
  `RESQL_BUILD_TIME` env var at build time; falls back to `0` when unset.
  `version` is formatted as `v{MAJOR}.{MINOR}.{PATCH}` — pre-release / build
  metadata (`-alpha.1`, `+build.5`) is stripped so the exposed format matches
  Java exactly.
- **Motivation.** `packagingTime` and version format restored to preserve
  Java operator's dashboards. `status` added because it is a common health-
  check field expected by k8s liveness probes; Java clients that assert only
  the 5 known keys ignore it.
- **Migration.** Operators wanting the exact Java packaging-time value must
  set `RESQL_BUILD_TIME=<unix-ms>` at container build time (see Dockerfile).
  Leaving it unset yields `packagingTime: 0`, which JSONAssert LENIENT-mode
  clients still accept.
- **Reversibility.** low (drop `status` from `HealthResponse`).

## DIV-003 — Batch URL shape

- **Field / behaviour.** Endpoint URL for batch execution.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/controller/QueryController.java:62-73`. Java
  declares `POST /{name}/batch`.
- **Target behaviour.** Rust exposes `POST /{project}/{name}/batch`.
- **Motivation.** Java's `POST /{name}/batch` handler is *broken as authored*:
  `QueryService.execute(String method, String queryName, ...)`
  (`Resql/src/main/java/rig/sqlms/service/QueryService.java:24-29`)
  calls `queryName.split("/", 1)` with `limit=1`, which by JDK contract returns
  a single-element array containing the entire input. The subsequent
  `projectQuery[1]` access throws `ArrayIndexOutOfBoundsException` on every
  invocation. Java's route has therefore never actually functioned. The Rust
  route uses the project prefix consistent with the single-query URL
  (`POST /{project}/{name}`), which resolves both routing and datasource
  disambiguation cleanly.
- **Migration.** Java clients calling `POST /add-debt/batch` must switch to
  `POST /{project}/add-debt/batch` (add the project segment). Operators who
  need a bridge can set the Rust `default_datasource:` config field — a
  future release will accept the Java shape and route through it (currently
  scoped out; see NOT-COVERED in the audit report).
- **Reversibility.** medium (would need to add a second route in `server.rs`
  and pick a default datasource for the un-prefixed shape).

## DIV-004 — Datasource routing: project → datasource

- **Field / behaviour.** How a URL project segment maps to a concrete DB.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/model/SavedQuery.java:24-26`.
  Java hardcodes the datasource name `"byk"` for *every* query, regardless of
  URL project. Multi-datasource configuration (`sqlms.datasources[]`) is
  silently ignored for routing purposes — only the `"byk"`-named entry ever
  serves traffic.
- **Target behaviour.** Rust routes by URL project. Given a request to
  `POST /crm/foo`, it looks up datasource `crm` (or the value of
  `project_datasource_map["crm"]` if set), or falls back to the value of the
  `X-Datasource` header when `allow_datasource_header: true`. Unknown
  datasource → 400 `UnknownDataSourceNameException` (Java message preserved).
- **Motivation.** Java's hardcoding is an obvious bug (per the TODO comment
  on line 25 of `SavedQuery.java`). Multi-datasource routing is the feature
  the config claims to expose. Fixing it is a strict improvement.
- **Migration.** A Java operator whose `sqlms.datasources` had exactly one
  entry named `byk` can keep working by (a) renaming their template
  project-dirs to `byk/` (matching the datasource name), OR (b) mapping every
  project explicitly via `project_datasource_map`. See MIGRATION.md.
- **Reversibility.** low (add a `legacy_hardcoded_datasource: byk` config
  field that overrides the router).

## DIV-005 — `datasources[].password` plaintext posture

- **Field / behaviour.** How database passwords are supplied.
- **Source of truth.** `Resql/src/main/resources/application.yml:22-27`
  (`sqlms.datasources[].password: <plaintext>`).
- **Target behaviour.** Rust accepts both the canonical `password_env: <VAR>`
  (env-var indirection, preferred) and the Java-legacy plaintext `password:`
  (for compatibility). Plaintext use emits a boot-time WARN naming the
  datasource. Specifying both fields is a hard error at parse time.
- **Motivation.** The Rust target's default security posture prefers env-var
  indirection so DB creds don't live in on-disk YAML that gets shipped with
  images or checked into git. Java's plaintext form is accepted for the
  operator's migration window but flagged loudly.
- **Migration.** Move `password: X` → `password_env: MY_DB_PW` and set the
  env var. No functional change beyond that.
- **Reversibility.** low (drop the plaintext field on `DatasourceConfig`).

## DIV-006 — `datasources[].driverClassName` accepted but ignored

- **Field / behaviour.** Explicit JDBC driver-class field.
- **Source of truth.** `Resql/src/main/resources/application.yml:27`
  (`driverClassName: org.h2.Driver`).
- **Target behaviour.** Rust accepts the field for compat (also the snake_case
  and kebab-case forms) and emits a boot-time INFO diagnostic naming it. The
  actual driver is derived from the URL scheme (`postgres://`, `sqlite:`).
- **Motivation.** Rust does not need JDBC (uses sqlx). Requiring the operator
  to name a driver class that will never be loaded is friction with no
  benefit.
- **Migration.** Optional — the field can be deleted from Java configs. If
  left in, Rust ignores it after logging.
- **Reversibility.** trivial.

## DIV-007 — Java-only config keys accepted with diagnostics

- **Field / behaviour.** Config keys `spring.*`, `h2.*`, `headers.*`,
  `userIPHeaderName`, `userIPLoggingPrefix`, `userIPLoggingMDCkey`.
- **Source of truth.** `Resql/src/main/resources/application.yml:1-14, 16-18`.
- **Target behaviour.** All are accepted at parse time (compat shim strips them
  before deserialisation) and each surfaces a boot-time diagnostic naming the
  field. Values have no effect on the Rust runtime.
- **Motivation.** §2.1 requires either honouring, rejecting loudly, or
  WARN-logging Java inputs. These fields have no target equivalent yet, so
  option 3 (WARN) is the compliant choice.
- **Migration.**
  - `spring.*` — no equivalent; safe to remove.
  - `h2.console.enabled` — Rust doesn't ship H2. Remove.
  - `headers.contentSecurityPolicy` — no wired equivalent yet.
    Track under future work (issue: security-headers).
  - `userIPHeaderName` / `userIPLoggingPrefix` / `userIPLoggingMDCkey` — no
    wired equivalent yet. Track under future work (issue: request-log-mdc).
- **Reversibility.** medium (each of the deferred features could be
  implemented later; the compat shim just documents the gap).

## DIV-008 — `server.port` → `server.bind`

- **Field / behaviour.** How the listen socket is specified.
- **Source of truth.** `Resql/src/main/resources/application.yml:5-6`
  (`server.port: 8082`), Spring Boot convention.
- **Target behaviour.** Rust's canonical field is `server.bind: 0.0.0.0:8080`.
  The compat shim translates Spring's `server.port` (and optional
  `server.address`) into `server.bind` at load time. If both `port` and
  `bind` are set, `bind` wins and a WARN is logged.
- **Motivation.** Rust needs full host:port to support IPv6 and non-INADDR_ANY
  binds without introducing a second config field.
- **Migration.** `server: { port: 8082 }` → `server: { bind: "0.0.0.0:8082" }`.
  Both work.
- **Reversibility.** low.

## DIV-009 — `cors.allowedOrigins` → `cors.allowed_origins`

- **Field / behaviour.** CORS origins config key.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/config/RestConfiguration.java:26-27`
  (`cors.allowedOrigins`, camelCase).
- **Target behaviour.** Rust canonical: `cors.allowed_origins` (snake_case).
  Compat shim translates camel → snake at load time.
- **Motivation.** snake_case matches the rest of the Rust config surface.
  Java camelCase preserved as alias.
- **Migration.** Either form works.
- **Reversibility.** trivial.

## DIV-010 — `logging.level` (Spring map) → EnvFilter directive

- **Field / behaviour.** Log-level configuration shape.
- **Source of truth.** `Resql/src/test/resources/application.yml:27-31`
  (`logging.level: { root: info, rig.sqlms: debug, org.springframework.jdbc.core: trace }`).
- **Target behaviour.** Rust canonical: `logging.level: "<envfilter-directive>"`
  (e.g. `"info,resql=debug"`). Compat shim translates a Spring-shape map into
  the EnvFilter directive at load time: `{root: info, rig.sqlms: debug}` →
  `"info,rig.sqlms=debug"`.
- **Motivation.** Rust's `tracing_subscriber` uses `EnvFilter`, which is a
  single directive string, not a per-logger map.
- **Migration.** Either form works. The translated form is what Rust logs
  will use, so `rig.sqlms=debug` becomes a rust-tracing filter that will
  never match (Rust modules are `resql`, not `rig.sqlms`). Operator should
  update to `resql=debug` for parity.
- **Reversibility.** low.

## DIV-011 — Request body size cap (1 MiB default) → 413

- **Field / behaviour.** New reject-condition on the request path.
- **Source of truth.** Java has no such cap; Spring Boot's `MULTIPART_MAX` and
  `spring.servlet.multipart.max-request-size` are separately configured and
  default to 10 MiB, but neither is wired into Resql. In practice, Java Resql
  accepts arbitrarily large JSON bodies.
- **Target behaviour.** Rust rejects request bodies larger than
  `server.max_body_bytes` (default 1 MiB) with HTTP 413 and error class
  `PayloadTooLargeException` (Spring's exact simpleName).
- **Motivation.** Denial-of-service protection: uncapped bodies exhaust
  server memory. 1 MiB comfortably fits any reasonable single-query or batch
  payload.
- **Migration.** Operators with legitimately larger batches should set
  `server.max_body_bytes:` to their needed cap.
- **Reversibility.** low (set to `usize::MAX` to disable).

## DIV-012 — Request timeout (30s default)

- **Field / behaviour.** New reject-condition on the request path.
- **Source of truth.** Java has no per-request timeout above Tomcat's default
  connector settings.
- **Target behaviour.** Rust applies `server.request_timeout_seconds`
  (default 30). Long-running queries beyond this are aborted; the exact HTTP
  status returned depends on where in the pipeline the timeout fires
  (typically 500 from the tower layer).
- **Motivation.** Prevent slow-query pileup exhausting the tokio runtime.
- **Migration.** Operators with legitimately long queries should raise this
  value, or better, move the workload out of the request path.
- **Reversibility.** low.

## DIV-013 — Unknown URL schemes rejected at boot

- **Field / behaviour.** DB URL scheme validation.
- **Source of truth.** Java accepts any `driverClassName` — MySQL, Oracle,
  MSSQL, etc. — with a corresponding JDBC driver on the classpath.
- **Target behaviour.** Rust only accepts URLs starting with `postgres://`,
  `postgresql://`, or `sqlite:`. Anything else fails at boot with a clear
  message.
- **Motivation.** Rust depends on `sqlx` which currently ships PG + SQLite
  drivers here; MySQL/MariaDB support is on the roadmap (Task 005). Failing
  loudly at boot beats a runtime `NoSuchDriverException`.
- **Migration.** Operators using MySQL must wait for Task 005 or use
  Postgres/SQLite.
- **Reversibility.** medium (would need `sqlx` MySQL driver + type mapping).

## DIV-014 — Boot log line strings

- **Field / behaviour.** Log-grep-visible strings emitted at startup.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/service/SavedQueryService.java:31, 36, 41`
  (`Initializing SavedQueryService`, `Loading queries from ...`, `Loaded queries: ...`),
  `Resql/src/main/java/rig/sqlms/datasource/DataSourceConfiguration.java:22`
  (`Initializing datasource: ...`).
- **Target behaviour.** Rust logs a different set of INFO lines: `starting
  Resql`, `app state ready`, `listening`, `bye`. Java-shape strings are not
  emitted.
- **Motivation.** Rust logs are structured (key/value pairs) and use the
  `tracing` crate's conventions. Reproducing Java's exact strings would
  require plaintext log format + hand-tuning per line.
- **Migration.** Operators grepping boot logs for `Initializing datasource`
  should switch to grepping for `app state ready` (which contains the
  datasource count) or use the `logging.format: json` mode.
- **Reversibility.** low (add extra `info!()` calls at the specific sites).
- **NOTE on §2.5 compliance:** §2.5 says user-facing strings SHOULD have
  target equivalents. This divergence is a deliberate SHOULD-violation with
  the rationale above. If any downstream operator reports that a boot-log
  monitor broke, the fix is trivial and would land as a §2.5-restoration
  change.

## DIV-015 — Per-request log format

- **Field / behaviour.** Per-request log line at INFO.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/service/QueryService.java:31`:
  `log.info("Incoming " + method + " for " + project + "::" + queryName)`.
- **Target behaviour.** Rust emits tower-http `TraceLayer` records instead,
  which use the request-span pattern (method, path, status, latency).
- **Motivation.** Same as DIV-014.
- **Migration.** Operators keyed on `Incoming POST for crm::/get-users`
  should switch to structured log parsing on `method="POST" uri="/crm/get-users"`.
- **Reversibility.** low.

## DIV-016 — Error handling for a malformed request body

- **Field / behaviour.** Error `error` field for malformed JSON POST body.
- **Source of truth.** Java: Spring's `HttpMessageNotReadableException`.
- **Target behaviour.** Rust: `MalformedRequestException` (custom Rust
  class name).
- **Motivation.** Rust doesn't use Spring's HTTP message-conversion pipeline,
  so mirroring the exact class name would be misleading. `MalformedRequest`
  is more accurate.
- **Migration.** Operators grepping for `HttpMessageNotReadableException`
  should switch to `MalformedRequestException`.
- **Reversibility.** trivial (rename in `src/error.rs:48`).

## DIV-017 — Additional Rust-side HTTP status codes

- **Field / behaviour.** HTTP status returned for two Rust-only conditions.
- **Source of truth.** Java: `GlobalExceptionHandler` maps EVERY exception to
  HTTP 400.
- **Target behaviour.** Rust maps `PayloadTooLargeException` → 413,
  `InternalError` → 500. All Java-known conditions still return 400.
- **Motivation.** 400 for a body-size violation is misleading; 413 is the
  standard status. 500 for genuinely internal errors avoids clients treating
  a server bug as a client bug they can fix.
- **Migration.** Operators handling only 400 should add 413 + 500 handlers.
- **Reversibility.** low.

## DIV-018 — SQL loader strictness on parse errors

- **Field / behaviour.** Startup behaviour when a single SQL file is malformed.
- **Source of truth.** `Resql/src/main/java/rig/sqlms/service/SavedQueryService.java:58-63`:
  logs ERROR for the offending file, continues loading the rest.
- **Target behaviour.** Rust fails to boot with an `InvalidQueryException`
  naming the file.
- **Motivation.** A partially-loaded query catalog is a footgun: operators
  believe their deploy succeeded while a subset of endpoints silently 404.
  Failing loudly forces a fix before traffic hits.
- **Migration.** Operators intentionally shipping "known broken" SQL files
  (a lint stage in their template repo) must remove them from the tree.
- **Reversibility.** low.
