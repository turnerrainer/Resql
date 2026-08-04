# MIGRATION — user-authored file formats

**Scope.** REFACTO-REQUIREMENTS §7.2 requires a MIGRATION.md when the target's
user-authored file format (DSL / templates / rulesets) differs from the source
of truth. This file describes the deltas indexed to the entries in
[DIVERGENCES.md](DIVERGENCES.md).

**What Resql's "user-authored format" is.**
1. `application.yml` / `resql.yaml` — the config file.
2. `templates/{project}/{METHOD}/{name}.sql` — the SQL query tree.

**TL;DR.** The SQL tree format is unchanged. The config file format is
compat-shim-supported: Java-shape files load without edits and every
Java-only field is announced at boot.

---

## 1. Config file (`application.yml` → `resql.yaml`)

### 1.1 Discovery order

Rust auto-discovers the config file if you don't set `-c`. Search order:

1. `/app/resql.yaml`   (canonical container path)
2. `./resql.yaml`      (Rust canonical name)
3. `./application.yml` (Java canonical name)
4. `./application-prod.yml`
5. `./application-dev.yml`
6. `./application-test.yml`

First existing file wins. Set `-c /path/to/file` or `RESQL_CONFIG=/path/…`
to override.

### 1.2 Before/after examples

#### 1.2.1 Minimal single-datasource Java config

**Before** (Java `application.yml`):

```yaml
sqlms:
  saved-queries-dir: "./templates/"
  datasources:
    - name: byk
      jdbcUrl: jdbc:postgresql://database:5432/byk
      username: byk
      password: 01234
      driverClassName: org.postgresql.Driver
```

**After (option A) — no edits needed:** the Rust binary loads this file as
`application.yml` (auto-discovery) and translates it. Boot log will emit:
- INFO for `driverClassName` (accepted, ignored).
- WARN for plaintext `password:` (recommend `password_env`).

Add one config field if templates live under a project dir that isn't `byk`:

```yaml
# Append at top level.
project_datasource_map:
  services: byk
```

(See DIV-004.)

**After (option B) — Rust-canonical form:**

```yaml
sql_dir: "./templates/"
datasources:
  - name: byk
    url: postgres://database:5432/byk
    username: byk
    password_env: BYK_DB_PW
project_datasource_map:
  services: byk
```

with the env `BYK_DB_PW=01234` set.

#### 1.2.2 Multi-datasource dev config

**Before** (Java test config):

```yaml
spring:
  profiles:
    active: test

sqlms:
  saved-queries-dir: "./src/test/resources/templates/"
  datasources:
    - name: crm
      jdbcUrl: jdbc:h2:mem:crm-db;DATABASE_TO_UPPER=false
      username: crm-user
      password: crm-password
      driverClassName: org.h2.Driver
    - name: debt
      jdbcUrl: jdbc:h2:mem:debt-db;DATABASE_TO_UPPER=false
      username: debt-user
      password: debt-password
      driverClassName: org.h2.Driver
logging:
  level:
    root: info
    rig.sqlms: debug
```

**Notes on porting:**
- `spring.profiles.active: test` — no target equivalent. Compat shim emits
  INFO; safe to leave.
- `jdbc:h2:mem:…` URL — H2 is not supported (DIV-013). Switch to SQLite
  in-memory: `sqlite::memory:`.
- `driverClassName` — accepted, ignored (DIV-006).
- Plaintext `password:` — accepted, WARN emitted (DIV-005).

**After (Rust-canonical):**

```yaml
sql_dir: "./src/test/resources/templates/"
datasources:
  - name: crm
    url: "sqlite::memory:"
    # (Optional) username + password_env if the SQLite DB requires auth.
  - name: debt
    url: "sqlite::memory:"
logging:
  level: "info,resql=debug"
```

---

## 2. SQL query tree

### 2.1 Layout — unchanged

```
<sql_dir>/
  <project>/
    GET/                        (uppercase, case-sensitive on Linux)
      <path...>.sql
    POST/
      <path...>.sql
```

Rust discovers exactly the same tree the Java service discovers. No edits
required.

### 2.2 Named-parameter syntax — unchanged

`:paramName` in the SQL body, matched against JSON body keys (POST) or query
string parameters (GET). Case-sensitive matching, matching Spring's
`NamedParameterJdbcTemplate` default.

### 2.3 Non-SQL files in the tree — silently ignored

Same behaviour in Java and Rust. Only files with the `.sql` extension
(case-insensitive) are loaded.

### 2.4 Parse-error handling — **stricter in Rust**

- Java: logs ERROR for the offending file, continues.
- Rust: fails to boot with `InvalidQueryException` naming the file.

See DIV-018. If your deploy pipeline expects partial loads, remove the
broken files before deploying to Rust.

---

## 3. URL surface — one change

### 3.1 `POST /{name}/batch` → `POST /{project}/{name}/batch`

Java's un-prefixed batch route (`POST /add-debt/batch`) has never actually
functioned (JDK `split(",", 1)` bug — see DIV-003 for the trace). The Rust
route requires the project segment:

```
# Before (Java, non-functional):
POST /add-debt/batch
{"queries": [{...}, {...}]}

# After (Rust):
POST /debt/add-debt/batch
{"queries": [{...}, {...}]}
```

Body shape is unchanged.

---

## 4. What did NOT change

- SQL syntax and dialect handling (still delegated to the DB driver).
- Named parameter placeholder syntax.
- Case-insensitivity of endpoint URLs.
- Response shape: `List<Map>` with snake_case → camelCase column renaming.
- `[]` for INSERT/UPDATE/DELETE.
- Error body shape: `{"error":"<ClassName>","message":"..."}`.
- The five `/healthz` fields (`appName`, `version`, `packagingTime`,
  `appStartTime`, `serverTime`) and their JSON keys.
- The four `/datasources` fields (`name`, `jdbcUrl`, `username`,
  `driverClassName`) and password stripping.
- The `-c/--config` flag behaviour (Rust adds it; Java uses Spring
  conventions).
- No authentication (Java's `permitAll()` preserved in Rust — no auth added).
- CORS default `*`.
