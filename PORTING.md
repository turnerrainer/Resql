# Operator porting guide — Java Resql → Rust Resql

**Audience.** Operators running the Java Spring Boot Resql who want to move
to the Rust reimplementation.

**Promise.** After this guide, your existing `application.yml`, SQL tree,
and clients keep working. No code changes required on the client side unless
you hit one of the divergences called out below.

**REFACTO-REQUIREMENTS §9.3 pledge:** every field in the Java
`application.yml` appears in the table below with its target equivalent and
migration action — you can port your config in one review pass without reading
target source.

**Cross-references:** For the *why* behind each behavioural difference, see
[DIVERGENCES.md](DIVERGENCES.md).

---

## 30-second summary

1. `cp application.yml resql.yaml` — the Rust binary auto-discovers
   `resql.yaml`, `application.yml`, and profile-suffixed variants.
2. `docker run -v $(pwd)/templates:/app/templates ghcr.io/turnerrainer/resql:latest`
   — the SQL tree layout is unchanged.
3. Read the boot log — every Java-specific field you had is announced with
   its migration hint.

Common gotcha: your queries used to route through the hardcoded `"byk"`
datasource. In Rust they route by URL project → datasource-name (or via
`project_datasource_map`). See DIV-004 in DIVERGENCES.md.

---

## Config field-by-field port

Every entry marked **KEEP** already works via the compat shim. Everything
marked **RENAME** works with the old name but you should adopt the new
name before the compat shim is deprecated (Rust's release-N+2 grace window
per REFACTO-REQUIREMENTS §6.3).

| Java field | Rust equivalent | Action | Notes |
|---|---|---|---|
| `spring.profiles.active` | (none) | **DELETE** | No profile mechanism; select the file at boot via `-c/--config` instead. Compat shim emits INFO. |
| `server.port` | `server.bind` | **RENAME** | Compat: `server.port: 8082` auto-translates to `server.bind: "0.0.0.0:8082"`. Also accepts `server.address` prefix. |
| `headers.contentSecurityPolicy` | (deferred) | **NOTE** | No target equivalent yet. Compat shim emits WARN naming this field. Track via issue `security-headers`. |
| `userIPHeaderName` | (deferred) | **NOTE** | See above; issue `request-log-mdc`. |
| `userIPLoggingPrefix` | (deferred) | **NOTE** | Same. |
| `userIPLoggingMDCkey` | (deferred) | **NOTE** | Same. |
| `h2.console.enabled` | (none) | **DELETE** | Rust doesn't ship H2; use SQLite for dev instead. |
| `sqlms.saved-queries-dir` | `sql_dir` | **KEEP / RENAME** | Compat: both `sqlms.saved-queries-dir: ./templates/` and top-level `saved-queries-dir:` and `sql_dir:` all work. Default is `./templates/` (matches Java). |
| `sqlms.datasources` | `datasources` | **KEEP / RENAME** | Compat: `sqlms.datasources[]` auto-flattens to top-level `datasources[]`. |
| `sqlms.datasources[].name` | `datasources[].name` | **KEEP** | Unchanged. |
| `sqlms.datasources[].jdbcUrl` | `datasources[].url` | **KEEP / RENAME** | Compat: `jdbcUrl`, `jdbc_url`, `jdbc-url` all accepted. |
| `sqlms.datasources[].username` | `datasources[].username` | **KEEP** | Unchanged. |
| `sqlms.datasources[].password` | `datasources[].password_env` | **CHANGE** | Plaintext accepted with WARN. Prefer setting `password_env: DB_PW` and providing the env var. See DIV-005. |
| `sqlms.datasources[].driverClassName` | (derived from URL) | **DELETE** | Rust infers driver from `postgres://` / `sqlite:` prefix. Compat: field accepted with INFO. |
| `logging.level.root: <lvl>` | `logging.level: "<lvl>,..."` | **KEEP / RENAME** | Compat: Spring's per-logger map is flattened into an EnvFilter directive. `logger` names are NOT translated — `rig.sqlms=debug` remains but won't match Rust modules; use `resql=debug` after migration. |
| `logging.level.<pkg>` | `logging.level: "<pkg>=<lvl>,..."` | **KEEP / RENAME** | See above. |
| `cors.allowedOrigins` | `cors.allowed_origins` | **KEEP / RENAME** | Both work. |

### Rust-only additions you can opt into

| Rust field | Default | Purpose |
|---|---|---|
| `server.max_body_bytes` | `1048576` (1 MiB) | Body-size cap. Set higher for large batches (DIV-011). |
| `server.request_timeout_seconds` | `30` | Per-request wall-clock timeout (DIV-012). |
| `datasources[].password_env` | `""` | Env-var name to read DB password from (preferred over plaintext). |
| `datasources[].max_connections` | `10` | Per-datasource pool size. |
| `datasources[].acquire_timeout_seconds` | `5` | Pool acquire timeout. |
| `allow_datasource_header` | `true` | If `true`, requests can override datasource via `X-Datasource` header. |
| `project_datasource_map` | `{}` | Explicit project → datasource mapping. |
| `default_datasource` | `null` | Reserved for future legacy batch-URL support (DIV-003). |
| `logging.format` | `"text"` | Set to `"json"` for structured logs. |

---

## HTTP endpoint reference — Java → Rust delta

| Java route | Rust route | Change? | Notes |
|---|---|---|---|
| `POST /{project}/**` (query) | Same | ✅ | Identical shape. |
| `GET /{project}/**` (query) | Same | ✅ | Identical shape. |
| `POST /{name}/batch` | `POST /{project}/{name}/batch` | ⚠️ URL change | See DIV-003. Java's route was broken (AIOOBE); Rust's is functional but requires the project prefix. |
| `GET /datasources` | Same URL, same JSON shape | ✅ | Fields: `name`, `jdbcUrl`, `username`, `driverClassName`. Password stripped. |
| `GET /healthz` | Same URL, same 5 fields | ✅ | Rust additionally emits `status: "UP"` — Java clients that ignore unknown fields see no change. |

**Rust additions** (never present in Java):
- `GET /health` — alias of `/healthz`.

---

## SQL file layout — Java → Rust delta

**Layout convention unchanged.** Both Java and Rust:

```
<sql_dir>/
  <project>/
    GET/                        (uppercase, case-sensitive on Linux)
      <path...>.sql
    POST/
      <path...>.sql
```

Rust additions:
- Endpoint lookup is case-**insensitive** (matches Java's `.toLowerCase()`).
- Named parameters `:name` are case-**sensitive** (matches Java's Spring
  `NamedParameterJdbcTemplate` default).
- Parse errors in a single SQL file fail the boot (Java would log ERROR and
  continue — see DIV-018). If you rely on partial loads, remove the broken
  files before deploying.

---

## Datasource routing — Java → Rust delta

**Java behaviour (broken as authored).** Every query, regardless of URL
project, resolves to a datasource named `"byk"`. If you have no such entry
in `sqlms.datasources`, every request errors out with
`UnknownDataSourceNameException`. If you have exactly one datasource, it's
almost certainly named `"byk"` (otherwise your service never functioned).

**Rust behaviour (fixed).** The URL project is looked up as a datasource
name; if `project_datasource_map[project]` is set, that value is used
instead. `X-Datasource: <name>` header overrides both when
`allow_datasource_header: true`.

**Migration paths:**

Option A — one-datasource legacy shape (most common):
```yaml
# Old Java config:
sqlms:
  datasources:
    - name: byk
      jdbcUrl: jdbc:postgresql://db/mydb
      username: byk
      password: 01234

# Direct port (compat-shim rewrites everything above), plus one line:
project_datasource_map:
  services: byk       # if your templates live under templates/services/
  # (add one entry per project dir)
```

Option B — rename template project-dirs to match the datasource name:
```
templates/byk/GET/…  # was templates/services/GET/…
templates/byk/POST/… # was templates/services/POST/…
```

Option C — send the header from your clients:
```
curl -H "X-Datasource: byk" http://resql:8080/services/get-users
```

Option D — set `default_datasource` (once implemented per DIV-003 —
currently deferred).

---

## Client-side impact checklist

Run through this list once before switching your traffic to Rust:

- [ ] **Config file loaded?** Set `-c /path/to/application.yml` or drop it
      at one of the auto-discovered paths. Boot log announces which file
      won.
- [ ] **Every WARN in the boot log addressed?** Each names a Java-only field
      with no effect on the Rust runtime.
- [ ] **Batch clients updated?** Change `POST /{name}/batch` →
      `POST /{project}/{name}/batch`.
- [ ] **Query URLs still resolve?** Try one of your existing endpoints; if
      you get `UnknownDataSourceNameException`, follow one of the migration
      paths above (Option A is easiest).
- [ ] **Dashboards on `/datasources` still parse?** Fields are the same
      Java shape: `name`, `jdbcUrl`, `username`, `driverClassName`.
- [ ] **Health probes still pass?** `/healthz` returns the same 5 Java
      fields plus a `status: "UP"` addition.
- [ ] **Bodies stay under 1 MiB?** If you post larger, raise
      `server.max_body_bytes`.
- [ ] **Queries finish under 30 s?** If not, raise
      `server.request_timeout_seconds`.

---

## Boot log — what you'll see

Example output (informational; message text may drift within minor Rust
releases without breaking this contract):

```
INFO field="spring" `spring` block is Spring-specific and has no effect on the Rust target
INFO field="h2" `h2` block is Spring-specific and has no effect on the Rust target
WARN field="userIPHeaderName" `userIPHeaderName` (Java) has no target equivalent yet; user-IP will not be logged. See DIVERGENCES.md
WARN field="headers.contentSecurityPolicy" `headers.contentSecurityPolicy` (Java) has no target equivalent yet; header will not be emitted. See DIVERGENCES.md
INFO field="datasources[test_db_1].driverClassName" `driverClassName: org.h2.Driver` accepted for compat; driver is derived from URL scheme in the Rust target
WARN field="datasources[test_db_1].password" datasource 'test_db_1' uses plaintext `password:` (Java shape). The Rust target prefers `password_env: <ENV_VAR_NAME>`. Plaintext accepted for compatibility. See DIVERGENCES.md.
INFO version="0.1.0-alpha.2" config="/app/application.yml" bind="0.0.0.0:8082" sql_dir="./templates/" datasources=1 starting Resql
INFO endpoints=12 datasources=1 app state ready
INFO addr=0.0.0.0:8082 listening
```

Everything above the `starting Resql` line names a Java-specific field the
Rust target either handled or intentionally deferred. Nothing was dropped
silently.
