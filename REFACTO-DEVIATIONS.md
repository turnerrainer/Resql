# REFACTO-DEVIATIONS — Rust Resql

REFACTO-REQUIREMENTS.md §10.2 requires per-project recording of any
deviation from the spec's MUST-level requirements. This file is that
record. "No silent opt-outs — same principle as §2.1 applied to process."

**Spec version audited against:** 1.0 (2026-08-04).

**Deviations count:** 1 as of 2026-08-04.

---

## DEV-001 — §4.3 Cross-implementation reproduction fixtures — PARTIAL

- **Requirement.** R4.3 (§4.3): "For each subsystem the target claims to
  preserve, at least one end-to-end fixture MUST run against BOTH source
  and target and produce byte-identical output (or output-equivalent
  modulo documented divergences)."
- **Compliance status.** Partial.
- **What's in place.**
  - Structural reference-shape JSONs in `compat/reference-outputs/` (four
    files: healthz, datasources, error body, query response).
  - Rust integration tests in `tests/integration_reference_shapes.rs`
    assert the target's live responses have the same field names, same
    value types, and same key semantics as the Java references.
  - Manual reproduction recipe in `compat/README.md` walks an operator
    through starting both services against a shared Postgres and diffing
    responses.
  - Java-shape `application.yml` fixture is embedded in
    `tests/integration_java_compat.rs` (as `JAVA_DEV_APPLICATION_YML`) and
    the tests confirm the compat shim loads it byte-faithfully.
- **What's missing.**
  - No CI job that spins up a real Java Resql instance, points both Java
    and Rust at a shared datasource, issues the same requests to both,
    and diffs byte-for-byte. The reproduction is manual.
- **Rationale.** Adding a Java build (Maven, JDK 17, docker, PostgreSQL
  testcontainers) to the Rust CI workflow is a substantial infrastructure
  change and was out of scope for the single-session compliance pass. The
  structural fixtures + manual recipe provide reasonable coverage; the
  hard bytes-equal guarantee is deferred to a follow-up ticket.
- **Mitigation.** The audit called this out explicitly in
  `audit-2026-08-04/00-audit-report.md` §4 "not covered", so downstream
  reviewers see the gap.
- **When to close.** As part of the work that lands `tasks/backlog/006`
  or a new `tasks/backlog/00X-cross-impl-ci.md` — spin up a docker-compose
  in CI with `java-resql` and `rust-resql` services + a corpus runner
  step that fails the build on any un-documented divergence.
- **Reversibility.** Adding the harness later has no code impact on the
  service; it's purely CI. No architectural implications.
