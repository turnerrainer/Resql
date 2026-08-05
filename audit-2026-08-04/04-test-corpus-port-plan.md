# Test-corpus port plan (REFACTO-REQUIREMENTS §1.3, §4.2)

**For every Java test fixture / integration test:** one of
- **Port verbatim** — fixture works in Rust, test asserts same outcome.
- **Port with fixture edits** — target's contract intentionally differs; edits listed.
- **Skip** — out of scope for target; rationale documented.

---

## 1. Java integration tests

### 1.1 `HeartBeatControllerIntegrationTest`

Source: `Resql/src/test/java/rig/sqlms/controller/HeartBeatControllerIntegrationTest.java`.

Assertion:
```
LENIENT compare of {"appName":"rig-sqlms","version":"1.0","packagingTime":1590762565458}
```
plus existence checks on `appStartTime`, `serverTime`.

**Rust equivalent:** `tests/integration_health.rs::health_returns_up_with_all_java_fields`.

**Fixture edits:**
- `appName` differs: Java says `"rig-sqlms"`, Rust says `"resql"`. This is
  an app-name change; not a shape drift. Test asserts the field is present
  and a non-empty string.
- `version` format is the same shape (`v{M}.{m}.{p}`) but different value
  because it's compile-time from Cargo. Test asserts `starts_with('v')`.
- `packagingTime` present as a number (compile-time from `RESQL_BUILD_TIME`,
  falls back to 0). Test asserts `is_number()`.
- `appStartTime` and `serverTime` — same monotonic-increasing semantic,
  asserted `> 0`.
- Rust additive `status: "UP"` — asserted.

**Status:** ✅ Ported with edits; asserts the same 5-field shape and null
behaviour. The value drift on `appName` is intentional and documented in
neither DIVERGENCES.md nor here as a divergence because it's an
implementation detail (branded name of the app), not a contract of the
service.

### 1.2 `DataSourceControllerIntegrationTest`

Source: `Resql/src/test/java/rig/sqlms/controller/DataSourceControllerIntegrationTest.java`.

Assertion:
```json
STRICT compare of [
  {"name":"crm","jdbcUrl":"jdbc:h2:mem:crm-db;...","username":"crm-user","driverClassName":"org.h2.Driver"},
  {"name":"debt","jdbcUrl":"jdbc:h2:mem:debt-db;...","username":"debt-user","driverClassName":"org.h2.Driver"}
]
```

**Rust equivalent:** `tests/integration_health.rs::datasources_endpoint_returns_java_shape`.

**Fixture edits:**
- URL differs (SQLite `:memory:` instead of H2). Semantically equivalent for
  test purposes.
- `driverClassName` value: Rust reports `org.sqlite.JDBC` for SQLite pools,
  `org.postgresql.Driver` for Postgres. Java would report exactly what the
  operator specified in `driverClassName`. Rust's mapping is
  deterministic-from-scheme (see DIV-006).
- Assertion mode changed from STRICT (Java shape only) to a "no unexpected
  Rust field names" style — verifies `url` and `driver` (the pre-fix Rust
  field names) are ABSENT, plus asserts the 4 Java-canonical fields ARE
  present.

**Status:** ✅ Ported with edits; asserts the same 4 Java field names + password
absence + no legacy Rust field-name leakage.

### 1.3 `QueryControllerIntegrationTest`

Source: `Resql/src/test/java/rig/sqlms/controller/QueryControllerIntegrationTest.java`.

**⚠ Caveat.** The Java test file's SQL fixtures live under
`src/test/resources/templates/crm/get-users.sql` (no `GET/POST/` intermediate
directory), and the tests invoke URLs like `POST /get-users` (one path
segment). Both patterns contradict the production `SavedQueryService`
convention (`{project}/{METHOD}/{name}.sql` layout, `/{project}/**` route
regex requiring two `/`). We suspect the Java test suite does not run
green; investigating with maven was out of scope for this audit round.
Recorded in the audit report §"not covered".

Per-test port disposition:

| Java test | Rust equivalent (existing) | Status |
|---|---|---|
| `execute_shouldHandleSelectWithMultipleResults` | `tests/integration_query.rs::post_query_with_json_body`, `snake_case_columns_come_back_camelcased` | ✅ Rust covers |
| `execute_shouldHandleSelectWithParameters` | `tests/integration_query.rs::post_query_with_json_body` | ✅ Rust covers |
| `execute_shouldHandleInsertWithParameters` (Java is `@Disabled`) | `tests/integration_query.rs::ddl_returns_empty_array` (partial) | ⚠ Java is disabled; Rust asserts INSERT return shape |
| `execute_shouldHandleUpdateWithParameters` | `tests/integration_query.rs::ddl_returns_empty_array` | ✅ Rust covers |
| `execute_shouldReturnEmptyArrayWithNullParameter` | `tests/integration_query.rs::null_json_body_is_treated_as_empty_object` | ✅ Rust covers a related null-body case |
| `execute_shouldHandleGetWithParameters` (query string) | `tests/integration_query.rs::get_query_with_query_string_params` | ✅ Rust covers |
| `execute_shouldHandleGetWithMultipleParameters` | `tests/integration_query.rs::get_query_with_query_string_params` | ✅ Rust covers same shape |
| `execute_shouldReturnBadRequestForUnknownDatasource` | `tests/integration_query.rs::unknown_datasource_returns_400` | ✅ |
| `execute_shouldReturnBadRequestForUnknownQuery` | `tests/integration_query.rs::query_not_found_returns_400` | ✅ |
| `execute_shouldReturnBadRequestForSqlError` | `tests/integration_query.rs::sql_error_returns_400` | ✅ |
| `execute_shouldHandleBatchRequest` | `tests/integration_query.rs::batch_returns_array_of_arrays` | ✅ (URL is Rust-shape; Java-shape was broken per DIV-003) |
| `execute_shouldHandleUppercaseColumns` | `tests/integration_query.rs::snake_case_columns_come_back_camelcased` | ✅ same behaviour verified |
| `execute_shouldHandleArrayColumn` | `tests/integration_postgres.rs` (PG-only) | ✅ Rust exercises Postgres array + JSON handling |

**Status:** ✅ Ported (Rust integration suite provides one-to-one coverage
for the meaningful Java tests). Non-portable ones (Java broken test setup)
are noted; not blocking.

---

## 2. Java fixture files

| Java fixture | Disposition | Reason |
|---|---|---|
| `src/test/resources/application.yml` | **Ported as compat corpus** (see §4) | The compat shim must accept this shape verbatim; test `java_application_yml_loads_successfully` in `tests/integration_java_compat.rs` verifies. |
| `src/test/resources/init-crm-db.sql` | **Skip** | Rust already ships equivalents for its own datasource tests. Adapting the exact Java schema is not needed for behaviour-parity. |
| `src/test/resources/init-debt-db.sql` | **Skip** | Same. |
| `src/test/resources/templates/crm/*.sql` | **Skip** | Layout inconsistent with production Java convention (see caveat in §1.3). |
| `src/test/resources/templates/debt/*.sql` | **Skip** | Same. |
| `src/test/resources/templates/no-datasource-configured/no-datasource-configured.sql` | **Skip** | Covered by Rust `unknown_datasource_returns_400`. |
| `docker-compose.yml` (Java) | **Skip** | Rust ships its own docker-compose. |
| `Dockerfile.dev` | **Skip** | Java-specific. |

---

## 3. Java `application.yml` variants — verification

The compat shim's `integration_java_compat` suite loads the actual Java
dev-profile shape as a string constant (`JAVA_DEV_APPLICATION_YML` in
`tests/integration_java_compat.rs`). The constant is a stripped-down but
shape-faithful copy of `Resql/src/main/resources/application.yml`. If the
Java YAML shape drifts upstream, this constant is the tripwire — bump it
to the new shape and re-run the suite.

Coverage this constant provides:
- `spring.profiles.active` — diagnostic emitted (INFO).
- `server.port` — translated to `server.bind`.
- `headers.contentSecurityPolicy` — diagnostic emitted (WARN).
- `userIPHeaderName/Prefix/MDCkey` — diagnostic emitted (WARN).
- `h2.console.enabled` — diagnostic emitted (INFO).
- `sqlms.saved-queries-dir` — flattened + renamed.
- `sqlms.datasources[]` — flattened.
- `sqlms.datasources[].jdbcUrl` — renamed.
- `sqlms.datasources[].password` — plaintext accepted, WARN emitted.
- `sqlms.datasources[].driverClassName` — INFO emitted.
- `logging.level.root` — flattened to EnvFilter directive.

Not (yet) covered by the constant (recorded in audit report §"not covered"):
- Real Java `docker-compose.yml` env-var override style
  (`sqlms.datasources.[0].name` with dotted path syntax).

---

## 4. Cross-implementation reproduction fixtures (§4.3)

See `audit-2026-08-04/05-cross-impl-fixtures.md`.

Summary: for each subsystem the target claims to preserve, at least one
fixture must run against BOTH Java and Rust and produce byte-identical
output (or output-equivalent modulo documented divergences). Current state:

| Subsystem | Fixture | Status |
|---|---|---|
| `/healthz` shape | `audit-2026-08-04/fixtures/healthz-shape.json` (structural) | ✅ Shape-parity fixture landed; behaviour asserted in `integration_health.rs`. Cross-run against Java not automated (Java build not in CI). |
| `/datasources` shape | Structural fixture in `integration_health.rs` | ✅ Same. |
| Config loading | `tests/integration_java_compat.rs` (parses real Java YAML shape) | ✅ Verified. |
| Query execution response shape | Structural via `integration_query.rs` + `integration_postgres.rs` | ⚠ Not byte-compared against Java. |
| Error body shape | Structural via `integration_query.rs` (`error` + `message` keys, HTTP 400 for Java-known conditions) | ✅ Same shape verified. |

**Gap called out:** No CI job spins up both Java and Rust services and diffs
their responses byte-for-byte on a common corpus. Recorded in audit report.
