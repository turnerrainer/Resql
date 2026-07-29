# Resql-on-Rust

SQL-files-as-REST-endpoints microservice. Drop a `.sql` file, get an HTTP
endpoint. Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service.

**Version:** 0.1.0-rc.1
**License:** [Apache-2.0](./LICENSE)
**Container:** `docker.io/turnerrainer/resql-on-rust:0.1.0-rc.1`

## One-command demo

```bash
docker run --rm -p 8080:8080 turnerrainer/resql-on-rust:0.1.0-rc.1
curl "http://localhost:8080/demo/hello?name=world"
# [{"greeting":"hello, world!"}]
```

## Docs

- **[Book](https://turnerrainer.github.io/Resql-on-Rust/)** — install, configure, extend.
- [Design doc](./docs/DESIGN.md) — what Resql-on-Rust is and isn't.
- [HANDOFF](./HANDOFF.md) — entry point for the next contributor.
- [Standards](./STANDARDS.md) — project rules on top of the [ecosystem baseline](../DEV-REQUIREMENTS.md).
- [Security disclosure](./SECURITY.md) — private channel for vulnerabilities.
