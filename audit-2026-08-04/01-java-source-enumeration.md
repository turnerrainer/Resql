# Java Resql — Source-of-Truth Contract Enumeration

Generated: 2026-08-04
Scope: Feeds §1.1 coverage matrix for Rust reimplementation audit.
Verification level: (code-read-verified) unless noted.

## 1. Configuration fields

### 1.1 application.yml (dev profile)

| Path | Type | Default | Parsed at | Consumed at | Required |
|---|---|---|---|---|---|
| `spring.profiles.active` | String | `dev` | application.yml:3 | Spring | Yes |
| `server.port` | Integer | `8082` | application.yml:6 | Spring | No |
| `headers.contentSecurityPolicy` | String | `script-src 'self'` | application.yml:9 | SecurityConfiguration.java:14 | No |
| `userIPHeaderName` | String | `x-forwarded-for` | application.yml:11 | RestConfiguration.java:23 | No |
| `userIPLoggingPrefix` | String | `from IP` | application.yml:12 | RestConfiguration.java:21 | No |
| `userIPLoggingMDCkey` | String | `userIP` | application.yml:14 | RestConfiguration.java:25 | No |
| `h2.console.enabled` | Boolean | `true` | application.yml:18 | Spring H2 | No |
| `sqlms.saved-queries-dir` | String | `./templates/` | application.yml:21 | SavedQueryService.java:30 | Yes |
| `sqlms.datasources[]` | List | (see 1.2) | application.yml:22 | DataSourceConfiguration.java:45 | Yes |
| `logging.level.root` | String | `info` | application.yml:30 | Spring Logging | No |
| `cors.allowedOrigins` | String | `*` | @Value default | RestConfiguration.java:42 | No |

### 1.2 Datasource array — each element

Class: `rig.sqlms.properties.DataSourceConfigProperties`

| Field | Required | Notes |
|---|---|---|
| `name` | Yes | Datasource identifier |
| `jdbcUrl` | Yes | Full JDBC URL |
| `username` | Yes | |
| `password` | Yes | `@JsonIgnore` — hidden from `/datasources` response |
| `driverClassName` | Yes | e.g. `org.h2.Driver`, `org.postgresql.Driver` |

### 1.3 Auxiliary properties files

- `classpath:heartbeat.properties` — `app.name`, `app.version`, `app.packaging.time`
- `file:/app/.env` — `BUILDTIME`, `MAJOR`, `MINOR`, `PATCH` (build-time metadata)

### 1.4 Docker override sample

`docker-compose.yml:8-15` sets `sqlms.saved-queries-dir=/DSL` and one datasource.

## 2. HTTP routes

| Method | URL pattern | Path vars | Body | Response | Statuses |
|---|---|---|---|---|---|
| POST | `/{project}/**` | project | JSON object (parameters) | `List<Map<String,Object>>` | 200, 400 |
| GET | `/{project}/**` | project | (query params) | `List<Map<String,Object>>` | 200, 400 |
| POST | `/{name}/batch` | name (query name — NOT project-prefixed) | `{"queries":[...]}` | `List<List<Map>>` | 200, 400 |
| GET | `/datasources` | — | — | `List<DataSourceConfigProperties>` (password hidden) | 200 |
| GET | `/healthz` | — | — | `HeartBeatInfo` | 200 |

**HeartBeatInfo fields** (DTOHeartBeatInfo.java): `appName`, `version` (format `v{MAJOR}.{MINOR}.{PATCH}`), `packagingTime`, `appStartTime`, `serverTime`.

**Path regex** (QueryController.java:24): `(/.+?)(/.+)` — splits project from rest. Non-matching paths → `NotFoundException` → 400.

**Batch NOTE:** Java's `/{name}/batch` takes only a query name; datasource is hardcoded to `"byk"` (SavedQuery.java:24-26).

## 3. User-authored SQL file format

- **Base directory:** value of `sqlms.saved-queries-dir` (default `./templates/`).
- **Layout:** `{project}/{METHOD}/{query-name}.sql` where METHOD is `GET` or `POST` (uppercase directory names).
- **URL mapping:** file `templates/crm/GET/get-users.sql` → route `GET /crm/get-users`.
- **Extension:** `.sql`.
- **Encoding:** UTF-8.
- **Named params:** `:paramName` syntax; case-insensitive lookup (SavedQueryService.java:82 lowercases).
- **Path regex for query name** (SavedQueryService.java:27): `(/.+?){3}(/.+)\..*` — extracts sub-path as query name.
- **Datasource routing:** hardcoded `"byk"` in Java (SavedQuery.java:25) — project name in URL does not actually route.
- **No header-comment DSL** (no `-- @directive:` conventions in the code).

## 4. Log lines and error messages

| Level | Template | Source |
|---|---|---|
| INFO | `"Incoming " + method + " for " + project + "::" + queryName` | QueryService.java:31 |
| INFO | `"Initializing SavedQueryService"` | SavedQueryService.java:31 |
| INFO | `"Loading queries from " + queriesPath` | SavedQueryService.java:36 |
| INFO | `"Loaded queries: " + ...` | SavedQueryService.java:41 |
| INFO | `"Initializing datasource: {names}"` | DataSourceConfiguration.java:22 |
| DEBUG | `"INPUT POST: {}"` / `"INPUT GET: {}"` | QueryController.java:38, 54 |
| DEBUG | `"Loaded queries: " + mapDeepToString(...)` | SavedQueryService.java:70 |
| ERROR | `"Failed parsing saved query file {}"` | SavedQueryService.java:61 |
| ERROR | `"Failed loading configuration service"` | SavedQueryService.java:67 |
| ERROR | `"Writing error: %s (%s)"` | GlobalExceptionHandler.java:34 |

**Exception messages:**

| Exception | Message template |
|---|---|
| `ResqlRuntimeException` | `"Saved query '%s' does not exist"` |
| `InvalidDirectoryException` | `"Saved configuration directory seems to empty"` or `"Saved configuration directory missing or not a directory: " + path` |
| `UnknownDataSourceNameException` | `"Specified dataSourceName name: '%s' is unknown to the service"` |
| `NotFoundException` | `"<URI> not found"` |

**Error response body shape** (GlobalExceptionHandler.java):

```json
{ "error": "<ExceptionClassName>", "message": "<message or 'Internal error'>" }
```

All errors: HTTP 400.

## 5. Metrics + tracing

- OpenTelemetry dependencies present in pom.xml but **no custom instrumentation** in source.
- No Micrometer @Timed / @Counted.
- No actuator endpoints exposed.

## 6. Test fixtures

**Integration test classes:**
- `BaseIntegrationTest.java` — MockMvc + two datasource setup
- `QueryControllerIntegrationTest.java` — POST/GET/batch, params, edge cases
- `DataSourceControllerIntegrationTest.java` — `/datasources`, password hiding
- `HeartBeatControllerIntegrationTest.java` — `/healthz`

**SQL fixtures** (`src/test/resources/templates/`):
- `crm/get-users.sql`, `crm/get-user-email-by-login.sql`, `crm/update-user-email-by-login.sql`, `crm/get-uppercase-user-email-by-login.sql`
- `debt/get-debt-by-user-id.sql`, `debt/add-debt.sql`, `debt/get-debt-due-dates-by-user-id.sql`, `debt/get-unknown-table.sql`
- `no-datasource-configured/no-datasource-configured.sql`

**DB init:** `init-crm-db.sql`, `init-debt-db.sql`.

**⚠ FIXTURE-STRUCTURE NOTE:** BaseIntegrationTest and Java's test SQL directory places files directly under `templates/{project}/{name}.sql` — NOT under `templates/{project}/{METHOD}/{name}.sql`. The production DSL says `{project}/{METHOD}/{name}.sql`. The test tree appears to break this rule — investigate to confirm actual production behavior.

## 7. Security / auth

- `SecurityConfiguration.java`: `anyRequest().permitAll()`, `csrf.disable()`.
- CSP header applied per config.
- No authentication scheme configured.

## 8. Response format details

- **Column-name transform:** snake_case → camelCase via `CaseUtils.toCamelCase(col.toLowerCase(), false, '_')` (ResqlJdbcTemplate.java:44).
- **Nulls:** included in result rows.
- **Timestamps:** default Jackson ISO 8601 with offset (e.g. `"2021-11-26T14:52:32.748+00:00"`).
- **SQL arrays:** unwrapped to JSON arrays via `arrayValue.getArray()`.
- **INSERT/UPDATE/DELETE:** returns `[]`.
- **Compact JSON** — no pretty-print.

## 9. Startup / shutdown

- Boot logs listed in §4.
- Spring Boot auto-configs disabled: `DataSourceAutoConfiguration`, `DataSourceTransactionManagerAutoConfiguration`, `UserDetailsServiceAutoConfiguration`.
- No custom retry, no custom shutdown hook, no explicit DB connectivity check in `/healthz`.

## 10. Misc

- **Batch:** `POST /{name}/batch`, name = query name only (not project-prefixed); Java hardcodes `"byk"` datasource for these.
- **No files written to disk** by the service.
- **Exit codes:** Spring defaults (0/1).
- **Env-var override:** via Spring property placeholders, no direct `System.getenv`.

## Summary — what the Java surface guarantees

**Every operator relying on Java Resql expects:**
- Config file: `application.yml` (Spring Boot convention).
- Config field prefix: `sqlms.*` (specifically `sqlms.datasources[]`, `sqlms.saved-queries-dir`).
- SQL dir default: `./templates/`.
- SQL layout: `{project}/{METHOD}/{name}.sql`.
- Named-param syntax: `:name`, case-insensitive.
- Health endpoint: `/healthz`.
- Endpoints: `POST/GET /{project}/{name...}`, `POST /{name}/batch`, `GET /datasources`.
- Response: `List<Map>` with snake→camel column renaming.
- Errors: HTTP 400 with `{"error":"...","message":"..."}` body.
- Password field on `/datasources` is stripped.
- All requests allowed (no auth).
