# 001 — Domain deep-dive

## Filed
2026-07-29 — First task after repo scaffold; needed to capture the public
interface Resql-on-Rust must preserve before writing any code.

## Landed
2026-07-29 — commit `<pending>`. Produced [`../../docs/DESIGN.md`](../../docs/DESIGN.md).

## Severity
High. Everything else in the roadmap depends on a stable contract for
the URL surface + parameter binding + error shape.

## Motivation
Consumers of the Bürokratt Resql API already write against a specific
URL shape (`<METHOD> /<project>/<name>`), a specific request/response
shape (JSON in, JSON array out, `snake_case` → `camelCase` columns),
and a specific error shape (`{"error": "<ClassName>", "message": "..."}`).
The Rust rewrite must keep those stable. Every "small deviation" costs
downstream consumers.

## Fix / Design
Read the JVM Resql source end-to-end. Capture:

- HTTP endpoint list with request + response shapes.
- SQL file → endpoint mapping rules (including case-sensitivity).
- Parameter binder (`:name` placeholder, missing/extra param behaviour).
- Multi-datasource routing (broken in JVM version).
- Error class → HTTP status → response body mapping.
- Startup validation vs runtime failures.
- Config surface (every YAML key that has an effect).

Then produce `docs/DESIGN.md` that:

- States the fixed public interface.
- Lists non-goals explicitly.
- Lists what's broken in JVM Resql we're intentionally fixing.
- Names implementation stack (axum / sqlx / tokio) so we don't drift.

## Acceptance
- [x] Every JVM Resql endpoint reproduced in DESIGN with request/response.
- [x] SQL file layout rule specified (`sql/<project>/<GET|POST>/<name>.sql`).
- [x] Parameter binder rules specified (comment-aware, cast-aware, repeated-param behaviour).
- [x] Error catalog mapped 1:1 to JVM exception class names.
- [x] Multi-datasource routing (project → header → map) specified.
- [x] Non-goals list bounds the scope.
- [x] "Fixed vs JVM" table so future maintainers know what NOT to reintroduce.

## Estimated effort
0.5 days.

## Dependencies
None — first task.

## Non-scope
- Any implementation code.
- New features not in JVM Resql (deferred to backlog).
- OpenTelemetry wire-up (task 004).

## Risks
Missing an undocumented behaviour that consumers rely on. Mitigation:
DESIGN's "Fixed vs JVM" table is scoped narrowly — only obvious bug
fixes count; anything ambiguous stays JVM-compatible until a consumer
reports a real dependency.
