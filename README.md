# Resql

SQL-files-as-REST-endpoints microservice. Drop a `.sql` file, get an HTTP
endpoint. Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service.

**Latest published:** 0.2.0-alpha (`docker.io/turnerrainer/resql:0.2.0-alpha`, also `ghcr.io/turnerrainer/resql:0.2.0-alpha`, cosign-signed) — closes the h2ck.me v1 pre-publication security audit. **Breaking config defaults** — see [Upgrading to 0.2.0-alpha](#upgrading-to-020-alpha) before you re-deploy.
**License:** [Apache-2.0](./LICENSE)

## One-command demo

The shipped image wires **two** SQLite datasources (`users` and `audit`)
so multi-database routing works out of the box.

```bash
docker run --rm -p 8080:8080 turnerrainer/resql:0.2.0-alpha
curl "http://localhost:8080/users/hello?name=world"
# [{"greeting":"hello from users db, world!"}]
curl "http://localhost:8080/audit/tail?n=42"
# [{"entry":"audit entry 42","rowId":42}]     ← different backend, same server
```

## Upgrading to 0.2.0-alpha

The v1 security audit (h2ck.me) flipped several config defaults and
introduced new required schema. If you deploy Resql today, take five
minutes to check your `resql.yaml` against this list before pulling
the next container. Full text in [`CHANGELOG.md`](./CHANGELOG.md).

| Change | Old default | New default | Break if you had… |
|---|---|---|---|
| `allow_datasource_header` | `true` | `false` | …clients sending `X-Datasource` to route requests |
| `datasource_header_allowlist` | *n/a* | required when header routing is on | `allow_datasource_header: true` and no allowlist → **every override 403** |
| `cors.allowed_origins` | `"*"` | `""` (no CORS layer at all) | …a browser client depending on the wildcard |
| CORS methods + headers | all echoed on preflight | `GET`/`POST` + 4 named headers only | …a client preflighting `DELETE` or a custom header |
| `/datasources` endpoint | `200` with data | `404` | …a bootstrap client hitting `/datasources` for discovery |
| `admin.datasources_public` | *n/a* | `false` | (set `true` to restore the endpoint — response is redacted regardless) |
| `request_timeout_seconds: 0` | silently ignored | boot fails with a config error | …timeout accidentally set to 0 |
| Long-running requests | never timed out | `504 Gateway Timeout` at `request_timeout_seconds` | …a client that ran queries longer than the timeout |
| Batch endpoint error body | raw driver text | `"Batch failed at statement N of M, rolled back"` | …a client regex'ing the message for schema names |

**Auditing an existing config** — a paste-in script + per-key `grep`
recipes live in [`CLAUDE.md`](./CLAUDE.md#fastest-way-to-audit-a-live-config).

**Writing a new config** — a fully-annotated hardened reference
`resql.yaml`, two common variants (browser-facing, legitimate header
routing), and a *do-not-do* checklist live in [`CLAUDE.md`](./CLAUDE.md#best-practice-resqlyaml-for-a-hardened-deployment).
Start from that block and delete anything that doesn't apply.

## Docs

- **[Book](https://turnerrainer.github.io/Resql/)** — install, configure, extend.
- [Design doc](./docs/DESIGN.md) — what Resql is and isn't.
- [Claude / LLM brief](./CLAUDE.md) — shortest possible orientation, the v1 audit breaking-change list, **and a best-practice `resql.yaml` reference**.
- [Standards](./STANDARDS.md) — project rules on top of the [ecosystem baseline](../DEV-REQUIREMENTS.md).
- [Security disclosure](./SECURITY.md) — private channel for vulnerabilities.
