# Getting started

Five steps: install, run the demo, verify, add your own SQL file, call it.

## 1. Install

Use one of the two officially supported paths:

**Docker** (recommended):

```bash
docker pull turnerrainer/resql:0.1.1-alpha
```

**From source** (Rust 1.88+):

```bash
git clone https://github.com/turnerrainer/Resql.git
cd Resql
cargo build --release --locked
```

The built binary is at `target/release/resql`.

## 2. Run the demo

```bash
docker run --rm -p 8080:8080 turnerrainer/resql:0.1.1-alpha
```

From source:

```bash
./target/release/resql --config resql.yaml
```

## 3. Verify

The shipped image wires two datasources (`users` and `audit`) so you can
see multi-database routing without any config.

```bash
curl http://localhost:8080/health
# {"appName":"resql","version":"v0.1.0","packagingTime":0,"appStartTime":..., "serverTime":..., "status":"UP"}

curl http://localhost:8080/datasources
# [{"name":"audit","jdbcUrl":"sqlite::memory:","username":"","driverClassName":"org.sqlite.JDBC"},
#  {"name":"users","jdbcUrl":"sqlite::memory:","username":"","driverClassName":"org.sqlite.JDBC"}]

curl http://localhost:8080/openapi.json | jq '.paths | keys'
# ["/audit/tail","/audit/write","/audit/write/batch","/users/echo","/users/echo/batch","/users/hello"]

# Hits the users datasource (URL project = users):
curl "http://localhost:8080/users/hello?name=world"
# [{"greeting":"hello from users db, world!"}]

curl -X POST http://localhost:8080/users/echo \
     -H "content-type: application/json" \
     -d '{"msg":"pong"}'
# [{"echoed":"pong","servedFrom":"users"}]

# Hits the audit datasource (URL project = audit):
curl "http://localhost:8080/audit/tail?n=42"
# [{"entry":"audit entry 42","rowId":42}]

# Force any endpoint onto any datasource with X-Datasource:
curl "http://localhost:8080/users/hello?name=world" \
     -H "X-Datasource: audit"
# [{"greeting":"hello from users db, world!"}]   ← same SQL, different pool
```

## 4. Add your first endpoint

The demo image bakes `sql/users/*` and `sql/audit/*` in. For your own
endpoints you'll want to mount a directory over `/app/sql`.

Create a file `./mysql/users/GET/find-by-login.sql`:

```sql
/*
description: Find users by login, optional status filter.
params:
  login:
    type: string
    required: true
  status:
    type: string
    required: false
returns:
  - name: id
    type: integer
    nullable: false
  - name: email
    type: string
*/
SELECT id, email FROM users
WHERE login = :login
  AND (:status IS NULL OR status = :status);
```

Every `.sql` file must open with a declaration block — see
[Declarations & OpenAPI](./declarations.md).

Run with your directory mounted and a Postgres datasource wired up:

```bash
docker run --rm -p 8080:8080 \
  -v "$PWD/mysql:/app/sql:ro" \
  -v "$PWD/resql.yaml:/app/resql.yaml:ro" \
  -e USERS_DB_PASSWORD="secret" \
  turnerrainer/resql:0.1.1-alpha
```

Where `resql.yaml` (see [Configuration](./configuration.md) for the full
reference) points at your database and names the env-var holding the
password:

```yaml
sql_dir: /app/sql
datasources:
  - name: users
    url: "postgres://localhost:5432/appdb"
    username: "app"
    password_env: "USERS_DB_PASSWORD"
```

## 5. Call it

```bash
curl -X POST http://localhost:8080/users/find-by-login \
     -H "content-type: application/json" \
     -d '{"login":"alice"}'
# [{"id":1,"email":"alice@example.com"}]
```

The URL segment `users` selects the `users` datasource by name. Override
per-request with the `X-Datasource` header if you need a different backend
for the same SQL file.
