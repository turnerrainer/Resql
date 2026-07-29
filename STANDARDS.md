# STANDARDS

This project follows the ecosystem baseline at
[`../DEV-REQUIREMENTS.md`](../DEV-REQUIREMENTS.md). Only project-specific
extras are listed here.

## Project-specific extras

### SQL file layout

The `sql/<project>/<GET|POST>/<path...>.sql` layout is load-bearing. Do
not change it without a design doc and a migration plan for existing
consumers — the URL surface is derived from it.

### Datasource routing precedence

`X-Datasource` header > `project_datasource_map` > project name.
This precedence is fixed by the [DESIGN doc](./docs/DESIGN.md); any change
requires a task, an ADR-shape note in DESIGN, and matching updates to
`book/src/configuration.md` and `book/src/sql-files.md`.

### Error response shape

The `{"error": "<ExceptionClassName>", "message": "…"}` shape is fixed
for interoperability with the original JVM Resql. Do not add fields
without bumping the minor version and updating
`book/src/failure-modes.md`.

### snake_case → camelCase column renaming

Applied to every result column. Some consumers may depend on it. If a
future task adds an opt-out, it must default to the current behaviour.

### Datasource driver support

Postgres and SQLite are the only supported drivers. Adding another
(MySQL, MSSQL) requires:

1. A design task with justification (why not use the existing two).
2. A CI matrix expansion so the driver's integration tests run.
3. `deny.toml` review — new transitive deps get audited.

## Ecosystem baseline reference

If any rule below appears to conflict with `../DEV-REQUIREMENTS.md`, the
baseline wins. Report the conflict as a task; do not resolve it silently.

- Rust standards → DEV-REQUIREMENTS §2
- Testing → §3
- Documentation → §4
- Security → §5
- CI/CD → §6
- Container → §7
- Release process → §8
- Publishing → §9
- Task tracking → §10
- Git discipline → §11
- Secrets handling → §12
- Audit discipline → §13
- Chat style → §14
- Memory system → §15
