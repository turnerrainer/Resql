# Getting started

Five steps: install, run the demo, verify, add your own SQL file, call it.

## 1. Install

Use one of the two officially supported paths:

**Docker** (recommended):

```bash
docker pull turnerrainer/resql-on-rust:0.1.0-alpha.1
```

**From source** (Rust 1.88+):

```bash
git clone https://github.com/turnerrainer/Resql-on-Rust.git
cd Resql-on-Rust
cargo build --release --locked
```

The built binary is at `target/release/resql-on-rust`.

## 2. Run the demo

```bash
docker run --rm -p 8080:8080 turnerrainer/resql-on-rust:0.1.0-alpha.1
```

From source:

```bash
./target/release/resql-on-rust --config resql.yaml
```

## 3. Verify

The shipped image wires two datasources (`users` and `audit`) so you can
see multi-database routing without any config.

```bash
curl http://localhost:8080/health
# {"appName":"resql-on-rust","version":"0.1.0-alpha.1","appStartTime":..., "serverTime":..., "status":"UP"}

curl http://localhost:8080/datasources
# [{"name":"audit","url":"sqlite::memory:","driver":"sqlite"},
#  {"name":"users","url":"sqlite::memory:","driver":"sqlite"}]

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
SELECT id, email FROM users WHERE login = :login;
```

Run with your directory mounted and a Postgres datasource wired up:

```bash
docker run --rm -p 8080:8080 \
  -v "$PWD/mysql:/app/sql:ro" \
  -v "$PWD/resql.yaml:/app/resql.yaml:ro" \
  -e USERS_DB_PASSWORD="secret" \
  turnerrainer/resql-on-rust:0.1.0-alpha.1
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
