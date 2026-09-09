# CLAUDE.md — quick brief for LLMs

Read this file before touching anything. It's the shortest possible
briefing on what this repo is, where the pain lives, and what has
recently changed in ways that break user configs.

If you need more context after this file, follow the pointers in
[Where to look next](#where-to-look-next).

## What this repo is

SQL-files-as-REST-endpoints microservice. A directory tree of `.sql`
files under `sql/<project>/<GET|POST>/<name>.sql` becomes an HTTP API
at `<METHOD> /<project>/<name>`. Interface-compatible Rust rewrite of
the [Bürokratt Resql](https://github.com/buerokratt/Resql) Spring Boot
service. Axum 0.7 + Tokio + sqlx (Postgres/SQLite).

Entry points worth knowing:
- `src/server.rs` — router + middleware chain, request handlers.
- `src/loader.rs` — SQL-file walker, YAML front-matter parser.
- `src/query.rs` — parameter binding, `:name → $N/?N` rewriter, batch executor.
- `src/db.rs` — pool factory + `after_connect` hooks.
- `src/config.rs` — `resql.yaml` schema + validation.
- `src/error.rs` — `ResqlError` variants + `IntoResponse`.

## Current version

- **Latest published**: `0.2.0-alpha` — live on `docker.io/turnerrainer/resql:0.2.0-alpha` + `ghcr.io/turnerrainer/resql:0.2.0-alpha`, cosign-signed, published 2026-09-06.
- **In `Cargo.toml` on `dev`**: `0.2.0-alpha`. Next merged commit that bumps `Cargo.toml` will auto-tag and auto-publish via `.github/workflows/auto-tag.yml` — no manual `git tag` needed.

## v1 security-audit changes (BREAKING for existing configs)

Between `0.1.2-alpha` and the shipped `0.2.0-alpha`, five audit fixes landed
that flipped defaults or introduced required schema. Every one is a CHANGELOG
`[0.2.0-alpha]` bullet with an `(RN)` tag. The user-visible impact — configs
written against 0.1.x-alpha may behave differently or refuse to boot:

### 1. `allow_datasource_header` default `true → false` (R1)

Old configs that omitted this field silently accepted `X-Datasource`
overrides. New default rejects them.

- **Grep to find affected configs**: `grep -L 'allow_datasource_header' resql.yaml` (files missing the key) — or check for `allow_datasource_header: true` explicitly.
- **Fix if you want the old behaviour**: set `allow_datasource_header: true` **and** add a `datasource_header_allowlist:` map — see next item.

### 2. `datasource_header_allowlist` newly required when header routing is on (R1)

Even with `allow_datasource_header: true`, the request is now rejected with
`403 ForbiddenDatasourceOverrideException` unless the target datasource is
allow-listed for the project.

- **Config shape**:

  ```yaml
  allow_datasource_header: true
  datasource_header_allowlist:
    users:   [users, users_replica]     # project 'users' may override to these
    audit:   [audit]                    # project 'audit' may override only to itself
  ```

- **Missing entry → 403**. The error message names only the offender; it does not enumerate registered datasources (deliberate — no schema disclosure).

### 3. `cors.allowed_origins` default `"*" → ""` (R2)

Old default emitted `Access-Control-Allow-Origin: *`. New default attaches
no CORS layer at all — browsers refuse cross-origin reads.

- **Grep**: `grep -L 'allowed_origins' resql.yaml` finds configs that were relying on the wildcard default.
- **Fix**: set `cors.allowed_origins: "*"` (still supported, still wide-open) or an explicit comma-separated allowlist, e.g. `cors.allowed_origins: "https://app.example.com,https://ops.example.com"`.

### 4. CORS methods + headers narrowed (R3)

When CORS is configured, the layer now advertises **only `GET` + `POST`** and only these request headers: `content-type`, `authorization`, `x-datasource`, `traceparent`. Any client relying on preflight for other methods or headers will break.

- **Not user-configurable** — this is a hard-coded narrowing to what the router actually serves. If you need more, add it in `build_cors()` in `src/server.rs`.

### 5. `/datasources` endpoint 404 by default, redacted when enabled (R5)

- **Old**: `/datasources` always returned the full list including `username` and `jdbcUrl` (with password masked). Callers who fingerprinted Resql via this endpoint will now see 404.
- **New**: hidden entirely unless `admin.datasources_public: true`. Even when enabled:
  - `jdbcUrl` is redacted to `<scheme>://<host>[:port]` (SQLite → just `sqlite:`).
  - `username` is always `""`.

- **Grep**: `grep -L 'datasources_public\|^admin:' resql.yaml`. Configs without an `admin:` block get the safer 404 default.
- **Fix if you need the endpoint**:

  ```yaml
  admin:
    datasources_public: true
  ```

  The real per-pool topology (with password masked) still shows up in the
  startup INFO log — `datasource connected datasource=<name> url=<masked>`
  — so operators can still verify wiring without exposing it on the wire.

### 6. `request_timeout_seconds: 0` now rejected at boot (R6+R7)

Old configs with `request_timeout_seconds: 0` silently disabled the
timeout. New behaviour: refuse to boot with a config validation error.
The middleware now returns `504 Gateway Timeout` on deadline expiry
(previously never fired). Postgres pools also `SET statement_timeout`
in their `after_connect` hook.

- **Grep**: `grep -E 'request_timeout_seconds:\s*0' resql.yaml` — any hit needs to change.
- **Fix**: pick a real ceiling, e.g. `request_timeout_seconds: 30`.

### 7. Batch endpoint error messages generalised (R9)

`POST /:project/:path/batch` failures now return
`Batch failed at statement N of M, rolled back` instead of the raw
driver text. HTTP status (`400`) and error kind (`BadSqlGrammarException`)
are unchanged — callers that only branch on status are unaffected.

- **Only affects clients that regex the message body** for constraint / column / table names. Those clients need to switch to querying the target schema directly or reading the operator log (WARN entry contains the raw underlying error with `resql.batch.index` + `resql.batch.total` fields).

## Post-0.2.0-alpha breaking change (BREAKING for callers, not configs)

Landing in the next minor bump (`0.3.0-alpha`), tracked by issue #25.
Unlike the v1 audit items above, this one is a **wire-shape** change
— configs are unaffected; **callers that read the error response body
break**. Everything semantically load-bearing (HTTP status codes,
exception-class identifiers) is preserved; only the transport for the
error detail moved from body to headers.

### 8. Error envelope moved from response body to response headers (issue #25)

The runtime cause of a class of silent fail-open bugs in downstream
Ruuter DSLs. The naive pattern:

```yaml
- condition: ${resql_res.response.body.length > 0}
  next: found
next: not_found
```

used to route a DB error into the `not_found` branch because the old
error body `{"error":"…","message":"…"}` is an object, `.length` is
`undefined`, and `undefined > 0` is `false`. Consequences ranged from
a wrong 404 to a DoS-amplifying fall-through in gateway-shaped
systems.

- **Old (`≤ 0.2.0-alpha`)**:
  ```
  HTTP/1.1 400 Bad Request
  Content-Type: application/json

  {"error":"BadSqlGrammarException","message":"…"}
  ```
- **New (`0.3.0-alpha`)**:
  ```
  HTTP/1.1 400 Bad Request
  Content-Type: application/json
  X-Resql-Error-Code: BadSqlGrammarException
  X-Resql-Error-Message: … (printable-ASCII sanitised; CR/LF stripped, non-printable → `?`)

  []
  ```

HTTP status and the exception-class identifiers themselves are
**unchanged**. Full un-sanitised message text stays in the server log
for the request's `trace_id`.

- **Grep to find affected callers** — hunt for reads of the old body
  fields anywhere downstream:
  ```bash
  # In Ruuter DSL / any consumer code:
  grep -rn 'body\.error\|body\["error"\]\|body\.message\|body\["message"\]' .
  # curl / http-shell scripts:
  grep -rn 'jq .error\|jq -r .message' .
  ```
- **Fix** — read the two response headers instead:
  ```yaml
  # Ruuter DSL:
  - assign:
      error_code:    ${resql_res.response.headers["x-resql-error-code"]}
      error_message: ${resql_res.response.headers["x-resql-error-message"]}
  ```
  ```bash
  # curl:
  code=$(curl -sS -D - -o /dev/null "$url" | awk 'BEGIN{IGNORECASE=1} /^x-resql-error-code:/ {print $2}' | tr -d '\r')
  ```
- **Safest migration order for DSL configs**:
  1. Ship the DSL update that reads from the header (works on both
     `≤ 0.2.0-alpha` — header is missing so var is empty — and `≥ 0.3.0-alpha`).
  2. Then bump Resql to `0.3.0-alpha`. Old body reads on the DSL side
     silently return "no rows" *once*, but by step 1 you've already
     migrated to the header path.
- **Callers who only branch on HTTP status** are unaffected. This is
  the recommended pattern going forward and matches every other
  well-shaped API.
- **Fastest audit grep**: any `body.error` / `body.message` read in
  Ruuter DSL or client code is a break. Rewrite to the header names
  above.

See `DIVERGENCES.md` DIV-022 for the source-of-truth rationale, and
[`book/src/failure-modes.md`](book/src/failure-modes.md) for the
operator-facing docs.

## Fastest way to audit a live config

```bash
# From the config directory:
python3 - <<'PY'
import yaml, sys
cfg = yaml.safe_load(open('resql.yaml'))
issues = []
if not cfg.get('allow_datasource_header', False) and \
   'x-datasource' in str(cfg).lower():
    pass  # header disabled, no issue
elif cfg.get('allow_datasource_header') and not cfg.get('datasource_header_allowlist'):
    issues.append("allow_datasource_header:true without datasource_header_allowlist → every override 403s")
if cfg.get('cors', {}).get('allowed_origins', '') == '':
    issues.append("cors.allowed_origins missing/empty → no cross-origin (was '*' pre-0.2)")
if cfg.get('server', {}).get('request_timeout_seconds') == 0:
    issues.append("request_timeout_seconds:0 → boot fails on 0.2.0-alpha")
if not cfg.get('admin', {}).get('datasources_public', False):
    issues.append("/datasources returns 404 by default (was 200 pre-0.2)")
if not issues:
    print("no v0.2.0-alpha migration issues found")
else:
    print("\n".join(f"- {i}" for i in issues))
PY
```

## Best-practice `resql.yaml` for a hardened deployment

This is what a `resql.yaml` should look like on a production-facing 0.2.0-alpha
box behind a reverse proxy. Every field is exposed so the intent is explicit;
comments explain why each choice is the safe posture. Copy this whole block,
then delete anything that doesn't apply — omitted fields fall back to their
(safe) defaults from `src/config.rs`.

```yaml
# ─── server surface ──────────────────────────────────────────────────────────
server:
  # Bind to loopback if a reverse proxy fronts the service (nginx, envoy,
  # a k8s ingress). Only bind 0.0.0.0 if this container is the ingress.
  bind: "127.0.0.1:8080"

  # 1 MiB is enough for parameter-heavy POST bodies; larger bodies almost
  # always indicate a client bug or an abuse. Structured 413 on overflow.
  max_body_bytes: 1048576

  # Wall-clock cap per request. Anything slower gets 504 + the pool
  # connection released. Postgres pools also SET statement_timeout to
  # this value in their after_connect hook, so cancelled queries die
  # at the server too (belt-and-braces vs pool exhaustion). Never 0.
  request_timeout_seconds: 30

# ─── SQL tree ────────────────────────────────────────────────────────────────
# Set this explicitly. The default (`./templates/`) exists for Java-compat
# only. Any symlinks inside this tree are refused at load time (R4).
sql_dir: "./sql"

# ─── datasource routing ──────────────────────────────────────────────────────
# Header-driven routing is a lateral-move lane inside the trust boundary.
# Keep it OFF unless a specific caller genuinely needs it. If you enable it,
# every (project, target-datasource) pair must be in the allowlist — an
# override to an un-allow-listed pair returns 403 (R1).
allow_datasource_header: false
# datasource_header_allowlist:               # only when allow_datasource_header: true
#   users:  [users, users_replica]
#   audit:  [audit]

# Explicit project→datasource map. Reads that route by project name still
# work without this (project name resolves to the same-named datasource by
# default), but stating it out loud is safer against typos.
project_datasource_map:
  users: users
  audit: audit

# Optional. Only set if you accept requests at the Java-legacy batch URL
# shape `POST /:name/batch`. Leaving unset returns an actionable error
# on that URL shape.
# default_datasource: users

# ─── datasources ─────────────────────────────────────────────────────────────
# Passwords come from env vars — `password_env`, NOT `password`. Setting
# `password` directly is Java-compat only; the compat shim emits a WARN
# every time and the plaintext ends up on disk in a config file that
# tends to leak into image layers and backups.
datasources:
  - name: users
    url:          "postgres://users-db.internal:5432/users"
    username:     "resql_users"
    password_env: "RESQL_USERS_PASSWORD"
    max_connections: 10          # default; raise carefully — pool exhaustion attack surface
    acquire_timeout_seconds: 5   # default; sane back-pressure

  - name: audit
    url:          "postgres://audit-db.internal:5432/audit"
    username:     "resql_audit"
    password_env: "RESQL_AUDIT_PASSWORD"

# ─── CORS ────────────────────────────────────────────────────────────────────
# Default is empty → no CORS layer at all → browsers refuse cross-origin
# reads (R2). Set an explicit origin allowlist only if a browser client
# needs to call this Resql directly (usually it shouldn't — put a
# same-origin proxy in front). Wildcard "*" still works but is a smell.
cors:
  allowed_origins: ""
  # allowed_origins: "https://app.example.com,https://ops.example.com"

# ─── admin endpoints ─────────────────────────────────────────────────────────
# /datasources returns 404 by default → unauth callers can't fingerprint
# Resql via this endpoint (R5). Only enable it if operators inside the
# trust boundary rely on it — and even then the response is redacted
# (`jdbcUrl` → scheme+host, `username` → empty). The un-redacted view
# lives in the startup INFO log.
admin:
  datasources_public: false

# ─── logging ─────────────────────────────────────────────────────────────────
logging:
  # Rust log-directive style. Bump the resql target to debug in staging,
  # keep noisy dependencies at info.
  level: "info,resql=debug"

  # `text` is human-readable; switch to `json` in prod for aggregation.
  format: "json"

  # One INFO line per request with method/route/status/duration/trace_id.
  # Leave on — it's the primary operational access log.
  access_log: true

  # Off in production — error `source()` chains can leak schema names
  # from the underlying driver. On in dev only when actively debugging.
  print_stack_trace: false

  # Cap on any body content shipped into a log line.
  max_body_bytes: 2048

  # Case-insensitive JSON field names redacted at every nesting depth.
  # Defaults are already good; extend for domain-specific secret shapes.
  redact_body_fields:
    - password
    - pass
    - secret
    - token
    - access_token
    - refresh_token
    - api_key
    - authorization

# ─── OpenAPI ─────────────────────────────────────────────────────────────────
openapi:
  title:       "My-Service Resql"
  description: "SQL-files-as-REST endpoints for the my-service backend."
  server_url:  "https://api.example.com/resql"
```

### Two common alternatives

**Browser-facing Resql** — reverse proxy is same-origin so no CORS layer, but if you must expose Resql cross-origin, pin the allowlist. Never `"*"` on a service that touches user data:

```yaml
cors:
  allowed_origins: "https://app.example.com"
```

**Legitimate `X-Datasource` header routing** — e.g. a workflow engine that reads a run's target-DB name off its trace context and passes it forward. Every (project, target) pair must be listed:

```yaml
allow_datasource_header: true
datasource_header_allowlist:
  runs: [runs_primary, runs_replica]
  # projects NOT listed here reject every X-Datasource override with 403
```

### What NOT to do

- **Never** set `password:` (plaintext). Always `password_env:`.
- **Never** set `allow_datasource_header: true` without also setting `datasource_header_allowlist`. Enabling the header without the allowlist means every override 403s — worse UX than leaving the header off entirely.
- **Never** set `request_timeout_seconds: 0`. Boot fails; the check exists because the alternative is pool exhaustion.
- **Never** set `admin.datasources_public: true` on an internet-facing service. If ops needs the view, expose it via a separate internal-only route on the reverse proxy.
- **Never** set `cors.allowed_origins: "*"` on any service that returns per-user data.
- **Never** run Resql with `bind: 0.0.0.0:...` on a shared host without a firewall between it and the internet.

## Doing tasks in this repo

Standard Rust workflow. Before touching code:

- `cargo +1.88.0 fmt --all -- --check` — CI pins **rustfmt 1.88.0**. Other versions reflow differently and CI will red-fail. If you don't have it: `rustup toolchain install 1.88.0 --component rustfmt --component clippy --profile minimal --no-self-update`.
- `cargo clippy --all-targets --locked -- -D warnings`
- `cargo test --no-fail-fast --locked` (SQLite integration only — Postgres tests skip cleanly without `TEST_POSTGRES_URL`)

`dev` is the default branch and is **push-protected** — you MUST land changes via PR. There is no direct push.

## Post-audit repo hygiene facts

- All 6 v1 audit PRs (#13–#18) merged into `dev` on 2026-09-06. Release PR (#20) tagged `v0.2.0-alpha`; container published by `publish.yml` the same day.
- `.github/workflows/auto-tag.yml` (added by #21) auto-tags + auto-publishes on any future merge that bumps `Cargo.toml` version. Merges that don't bump are idempotent — the workflow short-circuits.
- Feature branches from that cycle were deleted post-merge (local + remote).
- h2ck.me v1 verdicts (all ✅) live at [`h2ckme/Resql-on-Rust/v1/PR-REVIEWS/`](https://github.com/h2ckme/Resql-on-Rust/tree/main/v1/PR-REVIEWS). The v2 adversarial re-audit opens ~2 weeks after publish (so ~2026-09-20).
- `refacto/spec-compliance-v1` is the operator's long-running working branch — do not touch without asking.

## Where to look next

| Question | File |
|---|---|
| Full breaking-change bullets + release notes | `CHANGELOG.md` |
| Domain design (what Resql is/isn't) | `docs/DESIGN.md` |
| Divergence-from-Java catalog (DIV-001..DIV-022) | `DIVERGENCES.md` |
| Project rules (test hygiene, commit style, etc.) | `STANDARDS.md` |
| Cross-project ruleset | `../DEV-REQUIREMENTS.md` |
| h2ck.me audit trail (PR reviews, break-the-fix probes) | https://github.com/h2ckme/Resql-on-Rust |
