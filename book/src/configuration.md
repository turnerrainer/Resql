# Configuration

Resql-on-Rust reads a single YAML file at startup. The path is set with
`--config <path>` or the `RESQL_CONFIG` env var (default `/app/resql.yaml`
inside the container).

All fields have safe defaults except `sql_dir`. Every unknown YAML field
is rejected — typos fail startup instead of silently ignoring config.

## Top-level fields

| Field | Type | Default | Purpose |
|---|---|---|---|
| `sql_dir` | path | *(required)* | Directory scanned for `.sql` files at startup. |
| `server` | object | see below | HTTP listener settings. |
| `datasources` | list | `[]` | Configured database connections. |
| `project_datasource_map` | map | `{}` | Override which datasource a URL project routes to. |
| `allow_datasource_header` | bool | `true` | Honour `X-Datasource: <name>` on requests. |
| `cors` | object | see below | CORS layer settings. |
| `logging` | object | see below | Log level + format. |

## `server`

| Field | Type | Default | Purpose |
|---|---|---|---|
| `bind` | string | `0.0.0.0:8080` | Listener address. |
| `max_body_bytes` | integer | `1048576` (1 MiB) | Inbound JSON body cap. Overflow → HTTP 413. |
| `request_timeout_seconds` | integer | `30` | Reserved. Currently informational. |

## `datasources` (list of)

| Field | Type | Default | Purpose |
|---|---|---|---|
| `name` | string | *(required)* | Unique label. Referenced from URL project or `X-Datasource` header. |
| `url` | string | *(required)* | Connection URL. Supported schemes: `postgres://`, `postgresql://`, `sqlite:`. |
| `username` | string | `""` | Username to inject in the URL userinfo. Ignored if the URL already has one. |
| `password_env` | string | `""` | Env-var **name** holding the password. **Never store passwords in this file.** |
| `max_connections` | integer | `10` | Pool size ceiling. |
| `acquire_timeout_seconds` | integer | `5` | Per-acquire wait. |

Startup refuses in any of these cases:

- Two datasources share a `name`.
- A `username` is set but `password_env` is empty.
- `password_env` names a variable that is not set in the environment.
- `project_datasource_map` refers to a name absent from `datasources`.

## `project_datasource_map`

Maps URL project segment → datasource name. When a request arrives at
`POST /crm/find-user`, the default behaviour is to look up a datasource
named `crm`. Add an entry `crm: primary-db` to route it elsewhere.

Falls back to the project name if the map has no entry — a bare
`sql_dir` layout of `sql/foo/…` will look up datasource `foo` with no
config.

## `allow_datasource_header`

When `true` (default), the `X-Datasource: <name>` request header
overrides both the map and the project name. Set to `false` in
locked-down deployments where operators pick datasources centrally.

## `cors`

| Field | Type | Default | Purpose |
|---|---|---|---|
| `allowed_origins` | string | `*` | `*` = any; otherwise comma-separated exact origins. |

## `logging`

| Field | Type | Default | Purpose |
|---|---|---|---|
| `level` | string | `info,resql_on_rust=debug` | `tracing_subscriber` EnvFilter directive. |
| `format` | string | `text` | `text` or `json`. |

The `RESQL_LOG` env var overrides `level` at runtime without touching
the config file.

## Env-var overrides

Any string in `password_env` is looked up in the process environment.
Nothing else is env-driven — the config file is authoritative. If you
need per-environment overrides, use one config file per environment
or mount a config file from a secret at runtime.

## Complete example

```yaml
server:
  bind: "0.0.0.0:8080"
  max_body_bytes: 2097152   # 2 MiB

sql_dir: "/app/sql"

allow_datasource_header: true

project_datasource_map:
  legacyapp: primary
  reports: analytics

datasources:
  - name: primary
    url: "postgres://pg-primary.internal:5432/appdb"
    username: "resql"
    password_env: "RESQL_PRIMARY_PASSWORD"
    max_connections: 20
  - name: analytics
    url: "postgres://pg-analytics.internal:5432/warehouse"
    username: "resql_ro"
    password_env: "RESQL_ANALYTICS_PASSWORD"
    max_connections: 5

cors:
  allowed_origins: "https://ops.internal, https://console.internal"

logging:
  level: "info,resql_on_rust=debug,sqlx=warn"
  format: "json"
```
