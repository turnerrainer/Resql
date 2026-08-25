# Resql

SQL-files-as-REST-endpoints microservice. Drop a `.sql` file, get an HTTP
endpoint. Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service.

**Version:** 0.1.0-alpha.4
**License:** [Apache-2.0](./LICENSE)
**Container:** `docker.io/turnerrainer/resql:0.1.0-alpha.4`

## One-command demo

The shipped image wires **two** SQLite datasources (`users` and `audit`)
so multi-database routing works out of the box.

```bash
docker run --rm -p 8080:8080 turnerrainer/resql:0.1.0-alpha.4
curl "http://localhost:8080/users/hello?name=world"
# [{"greeting":"hello from users db, world!"}]
curl "http://localhost:8080/audit/tail?n=42"
# [{"entry":"audit entry 42","rowId":42}]     ← different backend, same server
```

## Docs

- **[Book](https://turnerrainer.github.io/Resql/)** — install, configure, extend.
- [Design doc](./docs/DESIGN.md) — what Resql is and isn't.
- [HANDOFF](./HANDOFF.md) — entry point for the next contributor.
- [Standards](./STANDARDS.md) — project rules on top of the [ecosystem baseline](../DEV-REQUIREMENTS.md).
- [Security disclosure](./SECURITY.md) — private channel for vulnerabilities.
