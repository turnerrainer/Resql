# HANDOFF

**Written:** 2026-07-29
**Last verified green (local):** 2026-07-29 — cargo test 77/0/0 (49 unit + 28 integration); fmt + clippy -D warnings clean; mdbook build clean.
**Branch:** `dev` — ready to tag `v0.1.0-rc.1`.
**Release status:** Local artifacts complete; the tag push + Docker Hub + GHCR publish is the operator step (§9 in `../DEV-REQUIREMENTS.md`).

Next contributor (human or Claude) must:

1. Read [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) front-to-back before touching anything. That's the authoritative ruleset for all Buerostack Rust projects.
2. Read this file for Resql-on-Rust-specific state.
3. Run the verification set (below); every command exits 0.

## What this repo IS today

Interface-compatible Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql) Spring Boot service.

- SQL files under `sql/<project>/<GET|POST>/<name>.sql` become endpoints at `<METHOD> /<project>/<name>`.
- `:named` parameters bind from JSON body (POST) or query string (GET).
- Result columns are snake→camel renamed; response is a JSON array.
- Multi-datasource: project name → datasource, override via `X-Datasource` header or `project_datasource_map`.
- Health at `/health` (also `/healthz` alias).
- Datasource listing at `/datasources` (passwords masked).
- Batch endpoint at `<POST-path>/batch`.

## What's fixed vs JVM Resql

- Datasource-by-project routing works (JVM version had hardcoded `"byk"`).
- Startup refuses to boot on any misconfigured datasource.
- Config file never holds passwords — env-var references only.
- Body-size cap (default 1 MiB, structured 413 on overflow).
- Cold-start ~15 MB / <100 ms (vs ~180 MB / ~4 s).

## Verification set (all should exit 0)

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked --bin resql-on-rust
cargo test --no-fail-fast --locked
cargo audit --deny warnings         # requires: cargo install cargo-audit
cargo deny check all                # requires: cargo install cargo-deny
( cd book && mdbook build )
```

Live smoke:

```bash
docker build -t resql-on-rust:local .
docker run -d --name resql-smoke -p 18080:8080 resql-on-rust:local
sleep 2
curl http://localhost:18080/health
curl "http://localhost:18080/demo/hello?name=world"
curl -X POST -H "content-type: application/json" -d '{"msg":"pong"}' \
     http://localhost:18080/demo/echo
docker stop resql-smoke && docker rm resql-smoke
```

## Publish steps (operator action — you)

Two one-time repository setups, then all future publishes are automatic.

### One-time GitHub setup

```bash
gh repo create turnerrainer/Resql-on-Rust --public \
    --description "SQL-files-as-REST-endpoints microservice (Rust)"

gh api repos/turnerrainer/Resql-on-Rust/pages \
    -X POST -f 'build_type=workflow'

gh api repos/turnerrainer/Resql-on-Rust/actions/permissions/workflow \
    -X PUT \
    -F 'default_workflow_permissions=write' \
    -F 'can_approve_pull_request_reviews=false'
```

### One-time Docker Hub setup

1. Create the repo: https://hub.docker.com/repositories/turnerrainer → **Create repository** → name `resql-on-rust` → **Public**.
2. Create a PAT: https://app.docker.com/settings/personal-access-tokens → **Generate new token** → **Restricted access** to `resql-on-rust` only → **Read + Write + Delete**. Copy the token immediately (only shown once).
3. Set the two secrets:

```bash
gh secret set DOCKERHUB_USERNAME --repo turnerrainer/Resql-on-Rust --body 'turnerrainer'
echo -n '<paste-token-here>' | gh secret set DOCKERHUB_TOKEN --repo turnerrainer/Resql-on-Rust
```

(Second command reads from stdin so the token does not appear in `ps` output or shell history.)

### Push the branch and tag

```bash
cd /home/rainer/Desktop/Buerostack/Resql-on-Rust
git remote add origin git@github.com:turnerrainer/Resql-on-Rust.git   # if not already
git push -u origin dev
git push origin v0.1.0-rc.1
```

The `publish.yml` workflow triggers on the tag push. Watch it:

```bash
gh run watch --repo turnerrainer/Resql-on-Rust
```

### One-time GHCR link (after first publish)

Personal-namespace packages need an explicit repo link before `GITHUB_TOKEN` can push subsequent versions:

- Open https://github.com/users/turnerrainer/packages/container/resql-on-rust/settings
- **Change visibility** → Public
- **Manage Actions access** → **Add repository** → `turnerrainer/Resql-on-Rust` → **Write**

### Verify the publish

```bash
docker logout && docker system prune -f
docker pull docker.io/turnerrainer/resql-on-rust:0.1.0-rc.1
docker run --rm -p 18080:8080 -d --name resql-live docker.io/turnerrainer/resql-on-rust:0.1.0-rc.1
sleep 2
curl http://localhost:18080/health
docker stop resql-live
```

Update `## Last verified green` above with the publish date once done.

## For the next Claude session

If refactoring another core component: everything you need is at
[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) + this repo as a reference implementation. Common questions:

| Question | See |
|---|---|
| How is `Cargo.toml` structured? | `Cargo.toml` |
| How does the multi-stage Dockerfile work? | `Dockerfile` |
| What goes in `docker-compose.yml`? | `docker-compose.yml` |
| What do `.github/workflows/*` look like? | `.github/workflows/` |
| How is a task file structured? | any file under `tasks/done/` |
| How is the book structured? | `book/src/` |
| How is CHANGELOG formatted? | `CHANGELOG.md` |
| How is publish-to-both-registries wired? | `.github/workflows/publish.yml` + DEV-REQUIREMENTS §9 |

## Roadmap

Landed (see [CHANGELOG.md](./CHANGELOG.md)):

- ✅ Task 001 — domain deep-dive → `docs/DESIGN.md`
- ✅ Task 002 — MVP per DESIGN §8

Open backlog:

| Task | Location | Notes |
|---|---|---|
| 003 | `tasks/backlog/003-transaction-scoping.md` | Optional transaction wrapper for POST endpoints |
| 004 | `tasks/backlog/004-opentelemetry-wire-up.md` | Actually export OTLP traces (currently deps-only, unused) |
| 005 | `tasks/backlog/005-mysql-driver-optional.md` | Consider MySQL — needs STANDARDS §"driver support" review |
| 006 | `tasks/backlog/006-postgres-integration-tests.md` | testcontainers-rs Postgres in CI; SQLite tests only for now |

## Where to look for more detail

| Topic | File |
|---|---|
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (Resql-specific) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Public docs | https://turnerrainer.github.io/Resql-on-Rust/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |
