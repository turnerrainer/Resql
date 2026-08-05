# HANDOFF

**Written:** 2026-07-29 (Postgres/Liquibase addendum 2026-07-30)
**Last verified green (local):** 2026-07-30 — cargo test 93/0/0 with `TEST_POSTGRES_URL` set (49 unit + 44 integration incl. 16 Postgres); fmt + clippy -D warnings clean; cargo audit + deny clean; mdbook + linkcheck clean; docker build + smoke pass on the multi-DB demo.
**Branch:** `refacto/spec-compliance-v1` (working). `dev` (default) still points at v0.1.0-alpha.1 (2026-07-31 publish).
**Release status:** v0.1.0-alpha.1 published 2026-07-31 (`docker.io/turnerrainer/resql:0.1.0-alpha.1`, `ghcr.io/turnerrainer/resql:0.1.0-alpha.1`, cosign-signed). **v0.1.0-alpha.2 is prepared on `refacto/spec-compliance-v1` but not yet published** — it adds task 007 (atomic batch + native Postgres array binding), task 003 (per-file `@transactional` marker), and the Java-compat layer landed since alpha.1.

**Pending operator follow-ups:**
- **Rotate the Docker Hub PAT.** The one used for `DOCKERHUB_TOKEN` was pasted in the release chat and should be considered compromised. Generate a new PAT, then re-set the secret: `echo -n '<new-pat>' | gh secret set DOCKERHUB_TOKEN --repo turnerrainer/Resql`.
- **GHCR one-time repo link** (only needed before the *next* release): open https://github.com/users/turnerrainer/packages/container/resql/settings → Change visibility → Public → Manage Actions access → Add repository → `turnerrainer/Resql` → Write. First publish worked because `GITHUB_TOKEN`'s package-write scope was sufficient; subsequent versions in the personal namespace need the explicit link.

Next contributor (human or Claude) must:

1. Read [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) front-to-back before touching anything. That's the authoritative ruleset for all Buerostack Rust projects.
2. Read this file for Resql-specific state.
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

- Multi-database routing works out of the box; the shipped `resql.yaml` wires two datasources (`users` + `audit`) so `docker run` demonstrates the feature (JVM version silently hardcoded a single datasource name).
- Startup refuses to boot on any misconfigured datasource.
- Config file never holds passwords — env-var references only.
- Body-size cap (default 1 MiB, structured 413 on overflow).
- Cold-start ~15 MB / <100 ms (vs ~180 MB / ~4 s).

## Verification set (all should exit 0)

Fast path (SQLite integration only, no external services):

```bash
cargo fmt --check
cargo clippy --all-targets --locked -- -D warnings
cargo build --release --locked --bin resql
cargo test --no-fail-fast --locked
cargo audit --deny warnings         # requires: cargo install cargo-audit
cargo deny check all                # requires: cargo install cargo-deny
mdbook build book                   # requires mdbook 0.4.40 + linkcheck 0.7.7
```

Full path (also runs Postgres integration suite — needs docker):

```bash
make test-all       # spins Postgres 16 + applies Liquibase changelog, then cargo test
make pg-down        # tear down when finished
```

CI runs the full path on every push. Local devs can stick to the fast
path — Postgres tests skip cleanly without `TEST_POSTGRES_URL`.

Live smoke:

```bash
docker build -t resql:local .
docker run -d --name resql-smoke -p 18080:8080 resql:local
sleep 2
curl http://localhost:18080/health
curl http://localhost:18080/datasources    # two datasources listed
curl "http://localhost:18080/users/hello?name=world"
curl -X POST -H "content-type: application/json" -d '{"msg":"pong"}' \
     http://localhost:18080/users/echo
curl "http://localhost:18080/audit/tail?n=42"
docker stop resql-smoke && docker rm resql-smoke
```

## Publish steps (operator action — you)

Two one-time repository setups, then all future publishes are automatic.

### One-time GitHub setup

```bash
gh repo create turnerrainer/Resql --public \
    --description "SQL-files-as-REST-endpoints microservice (Rust)"

gh api repos/turnerrainer/Resql/pages \
    -X POST -f 'build_type=workflow'

gh api repos/turnerrainer/Resql/actions/permissions/workflow \
    -X PUT \
    -F 'default_workflow_permissions=write' \
    -F 'can_approve_pull_request_reviews=false'
```

### One-time Docker Hub setup

1. Create the repo: https://hub.docker.com/repositories/turnerrainer → **Create repository** → name `resql` → **Public**.
2. Create a PAT: https://app.docker.com/settings/personal-access-tokens → **Generate new token** → **Restricted access** to `resql` only → **Read + Write + Delete**. Copy the token immediately (only shown once).
3. Set the two secrets:

```bash
gh secret set DOCKERHUB_USERNAME --repo turnerrainer/Resql --body 'turnerrainer'
echo -n '<paste-token-here>' | gh secret set DOCKERHUB_TOKEN --repo turnerrainer/Resql
```

(Second command reads from stdin so the token does not appear in `ps` output or shell history.)

### Push the branch and tag

```bash
cd /home/rainer/Desktop/Buerostack/Resql-on-Rust   # on-disk dir name is legacy; project name is Resql
git remote add origin git@github.com:turnerrainer/Resql.git   # if not already
git push -u origin dev
git push origin v0.1.0-alpha.2
```

The `publish.yml` workflow triggers on the tag push. Watch it:

```bash
gh run watch --repo turnerrainer/Resql
```

### One-time GHCR link (after first publish)

Personal-namespace packages need an explicit repo link before `GITHUB_TOKEN` can push subsequent versions:

- Open https://github.com/users/turnerrainer/packages/container/resql/settings
- **Change visibility** → Public
- **Manage Actions access** → **Add repository** → `turnerrainer/Resql` → **Write**

### Verify the publish

```bash
docker logout && docker system prune -f
docker pull docker.io/turnerrainer/resql:0.1.0-alpha.2
docker run --rm -p 18080:8080 -d --name resql-live docker.io/turnerrainer/resql:0.1.0-alpha.2
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
- ✅ Task 006 — Postgres integration tests + Liquibase-managed schema (2026-07-30)
- ✅ Task 003 — Per-file `@transactional` marker (2026-08-05, ships in alpha.2)
- ✅ Task 007 — Atomic `/batch` + native Postgres array binding (2026-08-05, ships in alpha.2). `?mode=bulk` routing was skipped as under-specified — see task file's completion notes.

Open backlog:

| Task | Sev | Location | Notes |
|---|---|---|---|
| 004 | Med | `tasks/backlog/004-opentelemetry-wire-up.md` | Actually export OTLP traces (currently no OTel deps) |
| 005 | Low | `tasks/backlog/005-mysql-driver-optional.md` | Consider MySQL — needs STANDARDS §"driver support" review |

## Where to look for more detail

| Topic | File |
|---|---|
| Cross-project ruleset (authoritative) | [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) |
| Domain design (Resql-specific) | [`./docs/DESIGN.md`](./docs/DESIGN.md) |
| Project-specific standards addendum | [`./STANDARDS.md`](./STANDARDS.md) |
| Public docs | https://turnerrainer.github.io/Resql/ |
| Full change history | [`./CHANGELOG.md`](./CHANGELOG.md) |
| Private security disclosure | [`./SECURITY.md`](./SECURITY.md) |
| CI workflows | [`.github/workflows/`](./.github/workflows/) |
