# Introduction

**Resql-on-Rust** turns a directory of `.sql` files into a REST API. There
is no controller code to write. Drop a file at
`sql/<project>/<GET|POST>/<name>.sql`; the file becomes an HTTP endpoint
at `<METHOD> /<project>/<name>` on next startup. JSON payload keys bind
to `:named` SQL parameters. Result columns are re-cased snake → camel and
returned as a JSON array.

**Version:** 0.1.0-rc.1
**License:** [Apache-2.0](https://www.apache.org/licenses/LICENSE-2.0)
**Container:** `docker.io/turnerrainer/resql-on-rust:0.1.0-rc.1`
**Source:** [github.com/turnerrainer/Resql-on-Rust](https://github.com/turnerrainer/Resql-on-Rust)

## One-command demo

The published image ships with a working SQLite-backed `demo` project.

```bash
docker run --rm -p 8080:8080 turnerrainer/resql-on-rust:0.1.0-rc.1
curl "http://localhost:8080/demo/hello?name=world"
# [{"greeting":"hello, world!"}]
```

## What it replaces

Resql-on-Rust is a Rust rewrite of the original
[Bürokratt Resql](https://github.com/buerokratt/Resql) Spring Boot service.
It keeps the same public shape (SQL file → endpoint, `:named` binding,
snake→camel result columns, JSON array response) and fixes a shortlist
of pain points captured in [docs/DESIGN.md](https://github.com/turnerrainer/Resql-on-Rust/blob/main/docs/DESIGN.md).

Notable behavioural improvements over the Spring Boot original:

| Behaviour | JVM Resql | Resql-on-Rust |
|---|---|---|
| Datasource routed by URL project | ❌ hardcoded to `"byk"` | ✅ project name → datasource, plus `X-Datasource` header + `project_datasource_map` |
| Missing config → startup fails loudly | Partial | Full: refuses to boot on any misconfigured datasource |
| Request body cap | Uncapped | Configurable ceiling, structured 413 on overflow |
| Datasource passwords in config file | Plaintext | Env-var references only; startup refuses if unset |
| Cold-start memory | ~180 MB | ~15 MB |
| Cold-start time | ~4 s | <100 ms |

## Where to read next

1. [Getting started](./getting-started.md) — install, run, add your first SQL file.
2. [Configuration](./configuration.md) — every YAML key + env var.
3. [Writing SQL endpoints](./sql-files.md) — file layout, parameter binding, batch API.
4. [Failure modes](./failure-modes.md) — every HTTP status and error class you might see.
