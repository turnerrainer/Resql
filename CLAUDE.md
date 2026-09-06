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

## Current + next version

- **Current in `Cargo.toml`**: `0.1.2-alpha`.
- **Last published container**: `docker.io/turnerrainer/resql:0.1.1-alpha` (README mentions this; `0.1.2-alpha` shipped as a git tag but the container tag lags).
- **Next**: `0.2.0-alpha` — MINOR bump because the v1 security audit changed default behaviour in ways that will make existing configs behave differently or refuse to boot. See below.

## v1 security-audit changes (BREAKING for existing configs)

Between `0.1.2-alpha` and the upcoming `0.2.0-alpha`, five audit fixes landed
that flipped defaults or introduced required schema. Every one of them is a
CHANGELOG [Unreleased] bullet with an `(RN)` tag. The user-visible impact:

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

## Doing tasks in this repo

Standard Rust workflow. Before touching code:

- `cargo +1.88.0 fmt --all -- --check` — CI pins **rustfmt 1.88.0**. Other versions reflow differently and CI will red-fail. If you don't have it: `rustup toolchain install 1.88.0 --component rustfmt --component clippy --profile minimal --no-self-update`.
- `cargo clippy --all-targets --locked -- -D warnings`
- `cargo test --no-fail-fast --locked` (SQLite integration only — Postgres tests skip cleanly without `TEST_POSTGRES_URL`)

`dev` is the default branch and is **push-protected** — you MUST land changes via PR. There is no direct push.

## Post-audit repo hygiene facts

- All 6 v1 audit PRs (#13–#18) merged into `dev` on 2026-09-06.
- Feature branches from that cycle were deleted post-merge (local + remote).
- h2ck.me v1 verdicts (all ✅) live at [`h2ckme/Resql-on-Rust/v1/PR-REVIEWS/`](https://github.com/h2ckme/Resql-on-Rust/tree/main/v1/PR-REVIEWS). The v2 adversarial re-audit opens ~2 weeks after the merged tree gets a container tag.
- `refacto/spec-compliance-v1` is the operator's long-running working branch — do not touch without asking.

## Where to look next

| Question | File |
|---|---|
| Full breaking-change bullets + operator upgrade notes | `CHANGELOG.md` `[Unreleased]` section |
| Domain design (what Resql is/isn't) | `docs/DESIGN.md` |
| Operator handoff (release procedure, publish steps, verification set) | `HANDOFF.md` |
| Project rules (test hygiene, commit style, etc.) | `STANDARDS.md` |
| Cross-project ruleset | `../DEV-REQUIREMENTS.md` |
| h2ck.me audit trail (PR reviews, break-the-fix probes) | https://github.com/h2ckme/Resql-on-Rust |
