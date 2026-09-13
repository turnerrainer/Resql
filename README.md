# Resql

SQL-files-as-REST-endpoints microservice. Drop a `.sql` file, get an HTTP
endpoint. Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service.

**Latest published:** 0.4.0-alpha (`docker.io/turnerrainer/resql:0.4.0-alpha`, also `ghcr.io/turnerrainer/resql:0.4.0-alpha`, cosign-signed) — closes the h2ck.me v1 runtime break-test + log-attack pass, adopts every applicable fleet stronghold. **Breaking defaults + new boot-refuse check** — see [Upgrading to 0.4.0-alpha](#upgrading-to-040-alpha) before you re-deploy.
**License:** [Apache-2.0](./LICENSE)

## One-command demo

The shipped image wires **two** SQLite datasources (`users` and `audit`)
so multi-database routing works out of the box.

```bash
docker run --rm -p 8080:8080 turnerrainer/resql:0.4.0-alpha
curl "http://localhost:8080/users/hello?name=world"
# [{"greeting":"hello from users db, world!"}]
curl "http://localhost:8080/audit/tail?n=42"
# [{"entry":"audit entry 42","rowId":42}]     ← different backend, same server
```

## Upgrading to 0.4.0-alpha

The v1 runtime break-test + log-attack pass (h2ck.me) drove a second
wave of hardening. The one change most likely to bite is the new
**fleet §3.1 boot-refuse check**: a non-loopback `server.bind` without
an authentication story now fails to start. **Run
`resql doctor -c path/to/resql.yaml`** against every config before
pulling the next container — it's a side-effect-free dry run that
exits `1` on the same failures `serve` would.

| Change | Old behaviour | New behaviour | Break if you had… |
|---|---|---|---|
| Boot on non-loopback bind | started regardless of auth story | **refuses** unless `security.inter_service_token_env` set OR `security.trust_network: true` OR bind is loopback (§3.1) | …`0.0.0.0:...` bind and no `security` block. Fix: add `security.trust_network: true` if a proxy authenticates upstream, or `inter_service_token_env` for the built-in bearer gate |
| `/openapi.json` | `200` with full spec | `404` (FN3) | …a client bootstrapping from the spec. Fix: `admin.openapi_public: true` |
| Method mismatch on saved query | `400 ResqlRuntimeException` | `405` with `Allow:` header (FN6) | …a client regex'ing the "does not exist" body/message |
| 413 (body too large) | bare `text/plain` | header-envelope shape (FN5) | …a client parsing the body text |
| 504 (request timeout) | empty body, no headers | header-envelope shape (FN5) | …a client parsing the (empty) body |
| Shipped `docker-compose.yml` | writable rootfs | `read_only: true` + tmpfs `/tmp` (FN4) | …a container that wrote outside `/tmp` |
| Shipped `resql.yaml` | `allow_datasource_header: true`, `cors: "*"` | matches the code-level safe defaults + `security.trust_network: true` for the demo (FN1 + shipped-config fix) | (opt in to either lane explicitly per site) |

**New surfaces that are OFF by default** (opt in per deployment):

- `security.inter_service_token_env: <ENV_VAR>` — built-in `Authorization: Bearer` gate. F-RES-3.
- `rate_limit: { requests_per_second, burst }` — global token bucket, 429 + `Retry-After` on exhaust. R8.
- `admin.openapi_public: true` — re-enable `GET /openapi.json`.

**Boot-time WARN catalogue** — every knowingly permissive knob emits a
WARN at boot (§8.1): wildcard CORS, non-loopback bind, `datasources_public`,
`print_stack_trace`, plaintext datasource `password:`. Ops teams see the
same audit line at boot they'd see in a review.

**Auditing an existing config** — the fastest gate is `resql doctor
-c resql.yaml --strict` — exit 0 = clean, exit 1 = hard error, exit 2 =
warnings only (with `--strict`). Wire it into CI.

**Writing a new config** — a fully-annotated hardened reference
`resql.yaml`, deployment topologies (proxy-fronted vs direct-exposure),
and a *do-not-do* checklist live in [`CLAUDE.md`](./CLAUDE.md#best-practice-resqlyaml-for-a-hardened-deployment)
+ [Configuration → Complete example](https://turnerrainer.github.io/Resql/configuration.html#complete-example--proxy-fronted-internal-service).

## Docs

- **[Book](https://turnerrainer.github.io/Resql/)** — install, configure, extend.
- [Design doc](./docs/DESIGN.md) — what Resql is and isn't.
- [Claude / LLM brief](./CLAUDE.md) — shortest possible orientation, the v1 audit breaking-change list, **and a best-practice `resql.yaml` reference**.
- [Standards](./STANDARDS.md) — project rules on top of the [ecosystem baseline](../DEV-REQUIREMENTS.md).
- [Security disclosure](./SECURITY.md) — private channel for vulnerabilities.
