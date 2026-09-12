# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately, **not** via a public
GitHub issue.

- **Email:** rainer.turner@gmail.com
- **Subject line:** `Resql security: <one-line summary>`

If you believe the issue is time-sensitive (active exploitation, credential
compromise), say so in the subject line and I will prioritise.

## What to include

- Affected version(s) — `docker inspect` output or `VERSION` file.
- Minimal reproduction (config snippet + curl command is ideal).
- Impact (what an attacker gains, what they need to already have).
- Any patches or mitigations you've considered.

## Response

- Acknowledgement within **72 hours**.
- Initial triage + severity classification within **7 days**.
- Fix released as a patch version (`0.X.Y+1`) or, for pre-1.0, an
  incremented `-rc.N`, with a CHANGELOG entry crediting the reporter
  (unless anonymity is requested).

## Supported versions

Only the latest release on the `main` branch (currently `0.1.1-alpha`)
is supported for security fixes. Pre-release channels (`rc`, `beta`,
`alpha`, `preview`) receive fixes on the same schedule as `main`.

## Authentication model — READ THIS BEFORE DEPLOYING

**Resql has no built-in authentication by default. Deploy accordingly.**

Resql is designed to sit **inside a trust boundary**, behind a reverse
proxy (Ruuter in Bürokratt, nginx / Traefik / Envoy elsewhere) that
authenticates every request before it reaches Resql. The service model
is "any HTTP request that reaches Resql is a saved-query invocation" —
the same posture a Postgres or MySQL server takes about its wire
protocol.

There are two safe deployment topologies:

**1. Behind a same-network reverse proxy (recommended).** Resql listens
on `127.0.0.1:8080` (loopback only) or an internal cluster address.
The reverse proxy terminates TLS, authenticates the caller, applies
rate limiting, and forwards approved requests. Resql sees only
already-authenticated traffic. This is the FileFerry / Ruuter deployment
pattern for Bürokratt.

**2. Direct exposure with the built-in bearer gate.** When (1) is
infeasible — dev, staging without Ruuter, a small deployment with no
proxy — enable the optional inter-service bearer:

```yaml
security:
  # Env var name whose value is the shared secret. Set the env var to
  # a random 32-byte value (`openssl rand -base64 32`).
  inter_service_token_env: RESQL_INTER_SERVICE_TOKEN
```

With this set, every request except `/health` and `/healthz` MUST carry
`Authorization: Bearer <value-of-env-var>` or receive `401
UnauthorizedException` in the standard header-envelope shape. Comparison
is constant-time on equal-length inputs. If the env var is missing or
empty at boot, Resql refuses to start — the "silently disabled because
env var forgot" failure mode is closed.

**Never do:**

- Expose Resql on `0.0.0.0` **without** either (1) a reverse proxy that
  authenticates upstream, or (2) `security.inter_service_token_env` set.
  Any Resql that becomes network-reachable without one of those is one
  `curl` away from unauth SQL execution against every registered
  datasource. Starting from v0.3.x, the boot log emits a WARN in this
  posture and (per `SecurityConfig::trust_network`) can be configured
  to refuse to start.
- Rely on `security.inter_service_token_env` alone in an internet-facing
  deployment — the token is a shared secret, not a per-caller identity.
  Rotate it on a schedule; put it behind a proxy that also applies rate
  limiting; consider mTLS at the proxy for genuinely public exposure.

### Endpoints and their auth status

| Route | Method | Purpose | Auth (bearer gate off) | Auth (bearer gate on) |
|---|---|---|---|---|
| `/health` | GET | LB liveness | none | **none (deliberate bypass)** |
| `/healthz` | GET | LB liveness (JVM compat) | none | **none (deliberate bypass)** |
| `/datasources` | GET | ops-view of registered pools | 404 by default; when enabled (`admin.datasources_public: true`), redacted list | 401 unless bearer valid |
| `/openapi.json` | GET | generated OpenAPI spec | 404 by default; when enabled (`admin.openapi_public: true`), spec | 401 unless bearer valid |
| `/:project/*tail` | GET/POST | saved-query invocation | **none** | 401 unless bearer valid |

Health probes always bypass the bearer gate so LB rolling deploys don't
flap the endpoint. Everything else is protected when the gate is on.

## CI supply-chain posture

Every push, PR, and daily cron runs:

- `cargo audit --deny warnings` — RustSec advisory database.
- `cargo deny check all` — license allow-list, ban list (no `openssl`,
  no unmaintained `serde_yaml`), no wildcard versions.

Every published container:

- Built reproducibly from `Dockerfile` at the tagged commit.
- Layer timestamps rewritten to `SOURCE_DATE_EPOCH` from the commit.
- Multi-arch (`linux/amd64,linux/arm64`) native builds (no QEMU).
- Trivy scan of HIGH/CRITICAL severity gates image signing.
- Cosign keyless signature via GitHub Actions OIDC.
- SBOM (SPDX) + build provenance attestations attached.

## Runtime posture

- Passwords are read from env vars named in config — never stored in
  the config file itself. See `password_env` on each datasource.
- The inter-service bearer secret is also read from an env var
  (`security.inter_service_token_env`) — same posture. Never embed the
  literal token in `resql.yaml`.
- Container runs as non-root (UID 1000), `no-new-privileges`, `cap_drop: ALL`,
  read-only rootfs, tmpfs on `/tmp` for scratch writes.
- Request body is capped (`server.max_body_bytes`, default 1 MiB) —
  overflow returns 413 in the header-envelope shape.
- Per-request wall-clock timeout (`server.request_timeout_seconds`,
  default 30 s) — a slow query no longer pins a connection indefinitely.
- Postgres `statement_timeout` is set to the request timeout via
  connection init hook, so a query outliving the middleware
  cancellation is still killed at the server.
- Every response carries the browser-defence header set
  (`Content-Security-Policy`, `Strict-Transport-Security`,
  `X-Frame-Options`, `X-Content-Type-Options`, `Referrer-Policy`) as
  defence in depth against reverse-proxy misconfigurations.
- Errors return an empty JSON array body + `X-Resql-Error-Code` /
  `X-Resql-Error-Message` response headers. Downstream DSLs that
  branch on `body.length > 0` can no longer fail-open on DB errors.
- `/datasources` is 404 by default and, when enabled, redacts URLs to
  scheme+host and drops usernames.
- The SQL loader refuses symlinks in the SQL tree and canonicalises
  every opened file path, so a symlink to `/etc/passwd` planted in
  the SQL dir is never read as SQL.
- The `X-Datasource` header override is off by default. When enabled,
  every (project, target-datasource) pair must be allow-listed —
  otherwise the override returns 403 without leaking which datasources
  exist.
