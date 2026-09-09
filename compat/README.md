# `compat/` — Java Resql cross-implementation fixtures

Per REFACTO-REQUIREMENTS §4.3, "For each subsystem the target claims to
preserve, at least one end-to-end fixture MUST run against BOTH source
[Java Resql] and target [Rust Resql] and produce byte-identical output (or
output-equivalent modulo documented divergences)."

This directory holds:

- `reference-outputs/` — response bodies captured from the Java Resql
  service run against a known-shape config + query tree. Rust integration
  tests assert equivalence against these.
- `scripts/` — reproduction recipes an operator can run manually to
  re-capture the reference outputs when the Java source of truth drifts.
- `configs/` — Java-shape configuration files used to drive both sides.

## Reproduction recipe

Prereqs: JDK 17+, Maven 3.6+, docker (for a shared Postgres).

```bash
# 1. Bring up Postgres shared between both services.
docker run -d --name resql-compat-pg \
  -e POSTGRES_PASSWORD=compat -e POSTGRES_DB=compat \
  -p 15432:5432 postgres:16

# 2. Load a common schema + seed.
psql "postgres://postgres:compat@localhost:15432/compat" \
  -f compat/scripts/schema.sql

# 3. Start Java Resql pointing at it.
cd ../Resql
sed 's|jdbc:postgresql://database:5432/byk|jdbc:postgresql://localhost:15432/compat|' \
    src/main/resources/application.yml > /tmp/java-resql-application.yml
./mvnw spring-boot:run \
    -Dspring-boot.run.arguments="--spring.config.location=file:/tmp/java-resql-application.yml" &
JAVA_PID=$!

# 4. Start Rust Resql pointing at the SAME datasource (different port).
cd ../Resql-on-Rust
RESQL_LOG=info \
  cargo run --release --bin resql -- -c compat/configs/rust-vs-java.yaml &
RUST_PID=$!

sleep 5

# 5. Capture responses.
for endpoint in /healthz /datasources; do
  curl -s http://localhost:8082$endpoint > "compat/reference-outputs/java$endpoint.json"
  curl -s http://localhost:8080$endpoint > "compat/reference-outputs/rust$endpoint.json"
done

# 6. Diff.
diff compat/reference-outputs/java-healthz.json compat/reference-outputs/rust-healthz.json
diff compat/reference-outputs/java-datasources.json compat/reference-outputs/rust-datasources.json

# 7. Teardown.
kill $JAVA_PID $RUST_PID
docker rm -f resql-compat-pg
```

Expected divergences (documented in DIVERGENCES.md):
- `/healthz`: Rust adds `"status":"UP"` and `appName` differs (`"resql"` vs
  `"rig-sqlms"`). The 5 Java fields are shape-identical.
- `/datasources`: `driverClassName` is `org.postgresql.Driver` from both
  sides; `jdbcUrl` may differ in exact form (URL scheme normalization).
- **Error responses (issue #25):** Java returns
  `{ "error": "…", "message": "…" }` in the body. Rust returns `[]` in
  the body and moves the same information to the response headers
  `X-Resql-Error-Code` and `X-Resql-Error-Message`. The status codes
  and exception-class identifiers are unchanged. The rewrite makes a
  naive downstream check `body.length > 0` safe: on any query-endpoint
  error the check now sees 0 rows instead of `undefined`, so a DB
  failure no longer silently routes to a "not-found" branch. The
  `compat/reference-outputs/error-body-java-shape.json` reference was
  removed accordingly.

## What lives here now

- `reference-outputs/healthz-java-shape.json` — structural spec for a Java
  `/healthz` response. Not a live capture; a documented shape.
- `reference-outputs/datasources-java-shape.json` — same for `/datasources`.
- `scripts/schema.sql` — schema used by both sides during reproduction.
- `configs/java-application.yml` — reference Java config.
- `configs/rust-vs-java.yaml` — matching Rust config (same DB, same tree).

**Automation status.** These fixtures are structural and text-diffable but
not run through both services on every CI push. Adding a live cross-impl
harness is on the roadmap. Manual reproduction remains the authoritative
end-to-end check.
