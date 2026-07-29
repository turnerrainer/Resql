# Security policy

## Reporting a vulnerability

Please report suspected vulnerabilities privately, **not** via a public
GitHub issue.

- **Email:** rainer.turner@gmail.com
- **Subject line:** `Resql-on-Rust security: <one-line summary>`

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

Only the latest release on the `main` branch (currently `0.1.0-rc.1`)
is supported for security fixes. Pre-release channels (`rc`, `beta`,
`alpha`, `preview`) receive fixes on the same schedule as `main`.

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
  the config file itself.
- Container runs as non-root (UID 1000), `no-new-privileges`, `cap_drop: ALL`,
  read-only tmpfs on `/tmp`.
- Request body is capped (`server.max_body_bytes`, default 1 MiB) —
  overflow returns 413 without buffering.
- Datasource passwords are masked in `/datasources` responses.
