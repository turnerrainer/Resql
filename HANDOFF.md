# HANDOFF

**Written:** 2026-07-29 (Postgres/Liquibase addendum 2026-07-30; v1 audit close-out 2026-09-06)
**Last verified green (local):** 2026-07-30 — cargo test 93/0/0 with `TEST_POSTGRES_URL` set (49 unit + 44 integration incl. 16 Postgres); fmt + clippy -D warnings clean; cargo audit + deny clean; mdbook + linkcheck clean; docker build + smoke pass on the multi-DB demo. (Post-v1-audit, `dev` also carries the R1–R7+R9 fix stack — CI green on every merged PR.)
**Branches:** `dev` (default) is the release integration line. `refacto/spec-compliance-v1` is the operator's long-running working branch.
**Release status:** `Cargo.toml` on `dev` is at `0.1.2-alpha` (git tag shipped, container not yet re-published — last published container is `0.1.1-alpha`). **Next release: `0.2.0-alpha`** — closes the h2ck.me v1 pre-publication security audit. **MINOR bump because config defaults changed in ways that will break some existing deployments** — full list in [`CHANGELOG.md`](./CHANGELOG.md) `[Unreleased]` and a shorter operator-facing table in the [README `Upgrading to 0.2.0-alpha`](./README.md#upgrading-to-020-alpha) section.

**Pending operator follow-ups (in order):**
1. **Bump `Cargo.toml`** `version = "0.1.2-alpha"` → `"0.2.0-alpha"` and rename the `## [Unreleased]` heading in `CHANGELOG.md` to `## [0.2.0-alpha] - <YYYY-MM-DD>` with the reference-link at the bottom of the file.
2. **Tag + publish**: `git tag v0.2.0-alpha && git push origin v0.2.0-alpha`. `publish.yml` fires on tag push and pushes both `docker.io/turnerrainer/resql:0.2.0-alpha` and the GHCR image.
3. **Update `README.md` Version block** to point at `0.2.0-alpha` once the container is live.
4. **Rotate the Docker Hub PAT.** The one used for `DOCKERHUB_TOKEN` was pasted in the release chat and should be considered compromised. Generate a new PAT, then re-set the secret: `echo -n '<new-pat>' | gh secret set DOCKERHUB_TOKEN --repo turnerrainer/Resql`.
5. **GHCR one-time repo link** (only needed before the *next* release): open https://github.com/users/turnerrainer/packages/container/resql/settings → Change visibility → Public → Manage Actions access → Add repository → `turnerrainer/Resql` → Write. First publish worked because `GITHUB_TOKEN`'s package-write scope was sufficient; subsequent versions in the personal namespace need the explicit link.
6. **Announce the config breaks** to any downstream that consumes Resql images. The README table is the canonical short list; [`CLAUDE.md`](./CLAUDE.md#v1-security-audit-changes-breaking-for-existing-configs) has grep recipes + a paste-in Python auditor for a live `resql.yaml`.

Next contributor (human or Claude) must:

1. Read [`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md) front-to-back before touching anything. That's the authoritative ruleset for all Buerostack Rust projects.
2. Read this file for Resql-specific state.
3. Run the verification set (below); every command exits 0.

## What this repo IS today

Interface-compatible Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql) Spring Boot service.

- SQL files under `sql/<project>/<GET|POST>/<name>.sql` become endpoints at `<METHOD> /<project>/<name>`.
- `:named` parameters bind from JSON body (POST) or query string (GET).
- Result columns are snake→camel renamed; response is a JSON array.
- Multi-datasource: project name → datasource, override via `X-Datasource` header (default OFF from 0.2.0-alpha; requires `allow_datasource_header: true` **and** a per-project `datasource_header_allowlist` entry) or `project_datasource_map`.
- Health at `/health` (also `/healthz` alias).
- Datasource listing at `/datasources` — **404 by default** from 0.2.0-alpha; set `admin.datasources_public: true` to expose it. Response is redacted regardless (`jdbcUrl` → `<scheme>://<host>[:port]`, `username` → `""`).
- Batch endpoint at `<POST-path>/batch`. Failures now return the generic `Batch failed at statement N of M, rolled back` (no driver-detail leak).

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
git push origin v0.2.0-alpha         # or whichever version is being released
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
docker pull docker.io/turnerrainer/resql:0.2.0-alpha
docker run --rm -p 18080:8080 -d --name resql-live docker.io/turnerrainer/resql:0.2.0-alpha
sleep 2
curl http://localhost:18080/health
# From 0.2.0-alpha, /datasources is 404 unless admin.datasources_public:true is set.
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

---

## h2ck.me security-audit pipeline

**Added**: 2026-09-06. Describes the ongoing pre-publication security audit + fix + review flow with the `h2ckme` private GitHub org. If you land in this repo cold and see open `fix/*` PRs referencing the h2ck.me audit, start here.

### What it is

h2ck.me runs a versioned audit → fix → validate cycle against every Bürostack-fleet service before it goes public. Each round is a `vN/` folder in the corresponding private repo under [`github.com/h2ckme`](https://github.com/h2ckme):

- `vN/AUDIT.md` — findings by severity, file:line pointers, attack scenarios.
- `vN/FIX-KIT.md` — runnable attack sandbox, diff-shaped fix code, per-finding acceptance criteria, "break-the-fix" input catalogue.
- `vN/PR-REVIEWS/<pr-number>-<head-sha7>.md` — one per PR reviewed (append-only across force-pushes).

**Fleet-wide index** — [`h2ckme/security-fleet` → `REVIEW-INDEX.md`](https://github.com/h2ckme/security-fleet/blob/main/REVIEW-INDEX.md).

### Where feedback lives

Reviews are file-based inside the private h2ckme org — h2ck.me does not post GitHub PR comments. The loop:

1. **Per-PR write-up** at [`h2ckme/Resql-on-Rust/v1/PR-REVIEWS/<pr#>-<sha7>.md`](https://github.com/h2ckme/Resql-on-Rust/tree/main/v1/PR-REVIEWS) — verdict (✅ / ⚠️ / ❌), acceptance-marker table, break-the-fix probes, nits for v2. Append-only across force-pushes (a new SHA writes a new file).
2. **Round roll-up** at [`h2ckme/Resql-on-Rust/v1/feedback/RESPONSE-YYYY-MM-DD.md`](https://github.com/h2ckme/Resql-on-Rust/tree/main/v1/feedback) — closes a batch of verifications; links to each per-PR file. Open this first when h2ck.me signals "verification done."
3. **Audit + fix-kit context**: [`h2ckme/Resql-on-Rust/v1/AUDIT.md`](https://github.com/h2ckme/Resql-on-Rust/blob/main/v1/AUDIT.md) + [`v1/FIX-KIT.md`](https://github.com/h2ckme/Resql-on-Rust/blob/main/v1/FIX-KIT.md).

**h2ckme access**: private org; your GitHub account has read via org membership. Clone with `git clone git@github.com:h2ckme/Resql-on-Rust.git`.

### v1 PRs — CLOSED (merged 2026-09-06)

All 6 audit PRs merged into `dev`. Feature branches deleted post-merge.

| PR | Findings | h2ck.me verdict |
|---|---|---|
| [#13](https://github.com/turnerrainer/Resql/pull/13) | R1 datasource-header ACL, R2 CORS default-deny, R3 narrow methods+headers | ✅ pass |
| [#14](https://github.com/turnerrainer/Resql/pull/14) | R4 SQL loader symlink reject | ✅ pass |
| [#15](https://github.com/turnerrainer/Resql/pull/15) | R5 `/datasources` 404-by-default + redact | ✅ pass |
| [#16](https://github.com/turnerrainer/Resql/pull/16) | R6 request timeout + R7 pool exhaustion (Postgres `statement_timeout` hook) | ✅ pass |
| [#17](https://github.com/turnerrainer/Resql/pull/17) | R9 batch endpoint generic error, no schema leak | ✅ pass |
| [#18](https://github.com/turnerrainer/Resql/pull/18) | docs — this pipeline description | ✅ (docs, no security marker) |

### Next action for a maintainer landing here

1. **Release** — bump `Cargo.toml` to `0.2.0-alpha`, close the `[Unreleased]` CHANGELOG block, tag `v0.2.0-alpha`, push. See "Pending operator follow-ups" at the top of this file for the full 6-step release checklist.
2. **Announce the breaking config defaults** to downstream consumers of the container. Canonical short list is the [README `Upgrading to 0.2.0-alpha`](./README.md#upgrading-to-020-alpha) table; grep recipes + a paste-in Python auditor for a live `resql.yaml` are in [`CLAUDE.md`](./CLAUDE.md#v1-security-audit-changes-breaking-for-existing-configs).
3. **Wait ~2 weeks after publish**, then h2ck.me opens `v2/` as an adversarial re-audit of the merged + tagged tree.

### If a review says ⚠️ or ❌

- ⚠️ pass-with-note = merge is OK but a docs/operator item is worth doing. Named in the write-up's "Nits" section.
- ❌ request-changes = don't merge; address on the same fix branch, push a new SHA. h2ck.me writes a fresh `<pr>-<new-sha7>.md` review — old file stays as audit trail.

### h2ck.me does NOT touch this repo

Explicit boundary: h2ck.me writes only to `h2ckme/*` (private org). It never pushes code, opens PRs, edits files, or posts comments in `turnerrainer/*`. All fixes come from you or a fixer of your choice, on a branch you push.
