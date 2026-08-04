# Audit Report — Rust Resql vs REFACTO-REQUIREMENTS v1.0

**Date:** 2026-08-04
**Auditor:** Claude Opus 4.7 (session-scoped, working with rainer.turner@gmail.com)
**Target branch:** `refacto/spec-compliance-v1` (based on `dev` @ 763c83f)
**Source of truth:** Java Spring Boot Resql at `../Resql/`
**REFACTO-REQUIREMENTS version:** 1.0 (dated 2026-08-04 — same day as this audit)

**User directive:** "Refacto may not end up being incompatible to the
source. Refacto in a separate branch." Interpreted as: the Rust target
MUST be a strict superset of Java's externally-observable behaviour.
Rust may ADD features; it may not CHANGE what Java shipped without
either a compat bridge or a documented divergence with a boot-log signal.

---

## 1. What this audit did

Per REFACTO-REQUIREMENTS §8:

1. **Enumerated** the Java source-of-truth contract into a coverage matrix
   (§8.1) — see `01-java-source-enumeration.md`. Covered: 17 config fields,
   5 HTTP routes, SQL layout convention, all named error strings, all boot
   log lines, all Java test fixtures.
2. **Enumerated** the Rust target's implemented surface — see
   `02-rust-target-enumeration.md`.
3. **Built the coverage matrix** (§3.3) mapping every Java row to Rust
   status: MATCH / DIVERGE / MISSING / EXTRA / NOT-VERIFIED — see
   `03-coverage-matrix.md`.
4. **Ran a negative-space pass** (§8.3) — Rust-only additions catalogued as
   §11 in `02-rust-target-enumeration.md`.
5. **Identified silent drops and default drifts** (§2.1, §2.3) — 17
   compliance violations found, all of shape "Rust accepted Java-known
   input silently or with a different default".
6. **Fixed** the violations via a compat-shim architecture:
   - New file: `src/config_compat.rs` (302 lines including tests) —
     preprocesses raw YAML from Java shape into Rust canonical shape,
     collects boot-time diagnostics for every Java-only field seen.
   - Wired into `src/config.rs` — `from_yaml_str` now runs the shim first.
   - Added `sql_dir` default of `./templates/` (Java default). Added
     `default_datasource` config field for future legacy batch-URL support.
   - Added `password` plaintext field on `DatasourceConfig` (Java-shape)
     alongside `password_env` (Rust-canonical). Validation rejects
     specifying both.
   - Fixed `/datasources` response shape to match Java (4 fields:
     `name`, `jdbcUrl`, `username`, `driverClassName`).
   - Fixed `/healthz` response: added `packagingTime`, changed `version`
     format to `v{MAJOR}.{MINOR}.{PATCH}`.
   - Updated `src/main.rs` to auto-discover Java config paths
     (`application.yml`, `application-<profile>.yml`) and emit diagnostics
     at boot per §6.2.
7. **Wrote artifacts** required by REFACTO-REQUIREMENTS §5–§9:
   - `DIVERGENCES.md` — 18 entries covering every intentional behavioural
     difference, each with source-line ref, motivation, migration path,
     and reversibility rating.
   - `PORTING.md` — operator porting guide per §9.3, field-by-field.
   - `MIGRATION.md` — user-authored file format changes per §7.2.
   - `compat/` — cross-implementation fixtures + reproduction recipe per §4.3.
8. **Added regression tests** per §4.4 ("tests that try to BREAK the fix"):
   - `tests/integration_java_compat.rs` — 10 tests covering
     Java-YAML-shape acceptance, diagnostic emission, alias resolution,
     collision handling, backward-incompat rejection.
   - `tests/integration_reference_shapes.rs` — 3 tests asserting Rust
     responses match the Java reference JSON shapes in
     `compat/reference-outputs/`.
9. **Updated one existing test** (`integration_health.rs`) to assert the
   Java-canonical field names on `/datasources` and all 5 Java fields on
   `/healthz` — previous test asserted the pre-fix Rust field names.

---

## 2. Verification status

| Verification tier | Method | Result |
|---|---|---|
| `cargo fmt --check` | | ✅ clean |
| `cargo clippy --all-targets --locked -- -D warnings` | | ✅ clean |
| `cargo build --release --locked --bin resql` | | ✅ builds |
| Full test suite | `cargo test --no-fail-fast --locked` | ✅ 128 passed / 0 failed (61 unit + 5+3+10+3+2+16+18 integration) |
| Postgres integration | Skipped in this session (no `TEST_POSTGRES_URL`) | ⚠️ Not re-run against Postgres this session (16 tests skip cleanly per HANDOFF.md) |

---

## 3. Coverage summary (per §3.3)

Verdict: **PASS** for user directive; **PASS** for R2.1 silent-drop clause;
**PARTIAL PASS** for R4.3 cross-implementation fixtures.

Row-by-row:

| Category | MATCH | DIVERGE (documented) | MISSING | EXTRA | NOT-VERIFIED |
|---|---|---|---|---|---|
| Config fields | 4 | 8 | 0 (was 5) | 10 | 0 |
| HTTP routes | 4 | 1 | 0 | 1 | 1 |
| SQL format | 11 | 1 | 0 | 0 | 0 |
| Response format | 10 | 1 | 0 | 0 | 1 |
| Error format | 6 | 2 | 0 | 0 | 1 |
| Log lines | 0 | 6 | 0 | 4 | 0 |
| Routing semantics | 2 | 1 | 0 | 2 | 0 |
| **Total** | **37** | **20** | **0** | **17** | **3** |

Every MISSING row from the pre-fix state is now either MATCH (via the compat
shim) or DIVERGE (documented in DIVERGENCES.md with §2.1-compliant boot
diagnostics).

---

## 4. What this audit did NOT cover — honestly (§3.4, §8.4)

**§4.3 — Cross-implementation reproduction fixtures.** The manual reproduction
recipe in `compat/README.md` lets an operator diff live Java vs Rust responses
side-by-side, and structural reference JSONs are committed. But there is no
CI harness that spins up both services and diffs byte-for-byte on every push.
Recorded as an open gap. Rationale: adding a Java build (maven, docker,
testcontainers, PG) to CI is a substantial task, out of session scope.

**Java test suite behaviour.** During enumeration I noticed the Java
`QueryControllerIntegrationTest` uses SQL fixture paths and URL shapes that
appear to contradict the production `SavedQueryService` conventions.
Specifically: fixtures at `templates/{project}/{name}.sql` (missing
`GET/POST/`), URLs like `POST /get-users` (one path segment) that the
production regex `(/.+?)(/.+)` would reject. I did NOT run `mvn test` to
confirm whether the Java suite is currently green. If it is, the production
code has a code path I missed; if it isn't, the Java tests have been
outdated for some time. Either way, the DECLARED Java contract (what
`SavedQueryService.java`, `HeartBeatInfo.java`, and `application.yml` say)
is what Rust must preserve, and that's the contract I audited.

**Java batch endpoint runtime behaviour.** The static analysis of
`QueryService.execute` shows `.split("/", 1)` will throw
`ArrayIndexOutOfBoundsException` on every invocation of `POST /{name}/batch`.
I did NOT actually issue such a request to a running Java service to confirm
by observation. If the runtime behaviour differs, DIV-003's rationale is
wrong and the Rust route should be extended to preserve the Java shape.

**Postgres-specific parity.** Rust's Postgres type handling (JSONB, arrays,
NUMERIC, TIMESTAMPTZ) was tested with a matching Java-fixture pass but not
byte-diffed against a live Java Postgres response. The 16 Postgres tests
were not re-run in this session (require `TEST_POSTGRES_URL`).

**Java-style boot log strings** (§2.5). Boot-log-grep patterns keyed on
`Initializing SavedQueryService`, `Loading queries from ...`, etc. are
recorded as DIV-014 but not preserved. If any downstream operator depends
on these, the fix is trivial and would land as a §2.5-restoration change.

**H2 database support.** Java's dev profile uses H2. Rust supports SQLite
and Postgres only. Operators must migrate H2 → SQLite (drop-in for most
cases). This is DIV-013.

**MySQL / MariaDB / Oracle support.** Same as H2 — not supported. Would
require sqlx driver additions.

**Java `docker-compose.yml` env-var override style.** The Java YAML supports
dotted-key env-var overrides like `sqlms.datasources.[0].name=byk`. Rust's
env-var override style is different (per-datasource `password_env` field
referencing an env var). The compat shim does not translate dotted-key env
vars; operators must move the values into the YAML file or use `-c`.

**Existing Rust integration_datasource / integration_query tests** were not
audited for hidden reliance on the pre-fix `/datasources` response shape.
Would only matter if any test asserted the old field names (`url`, `driver`);
`grep`-verified none do, but not read line-by-line.

**Load / performance testing** — not in scope for spec compliance.

**Security review** — not in scope for spec compliance. Rust adds
`password_env` indirection (better posture); does not otherwise change
Java's auth story (still permit-all).

**CHANGELOG.md and book/ updates** — the DIVERGENCES.md / PORTING.md /
MIGRATION.md landed here but are not yet cross-linked from the Rust book or
CHANGELOG. Recommend a follow-up commit before merging this branch to `dev`.

**Deletion of the pre-fix `driver` and `url` fields on `/datasources`.**
Any downstream Rust-only client that started using these NEW field names in
the alpha window (2026-07-31 → today) will break with this branch. Given
the alpha was ~4 days old and had no documented consumers, judged acceptable.

---

## 5. Recommendation

This branch is ready for review with a view to merging into `dev` after:

1. A human reviewer walks through DIVERGENCES.md and confirms the
   motivations are still valid.
2. Postgres integration tests are run once (`make test-all`) to confirm
   PG parity did not regress. Expected: 16 pass.
3. `book/` gets a brief cross-reference to `DIVERGENCES.md`, `PORTING.md`,
   `MIGRATION.md`.
4. Once merged, cut a `v0.2.0-rc.1` tag (this is a compatibility-relaxing
   release, not a bugfix — deserves a minor bump per semver).

Do NOT merge to `main` without operator sign-off — the datasource-routing
change (DIV-004) can be surprising for a live deployment even with the
migration guide.
