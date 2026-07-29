# Changelog

All notable changes to this project will be documented in this file.
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/),
and this project adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.0-rc.1] - 2026-07-29

Initial Rust rewrite of the [Bürokratt Resql](https://github.com/buerokratt/Resql)
Spring Boot service. Interface-compatible with the original for the SQL-file-to-endpoint,
`:named`-parameter, and snake→camel result semantics.

### Added

- SQL file loader (`sql/<project>/<GET|POST>/<name>.sql` → `<METHOD> /<project>/<name>`).
- Named-parameter binder with awareness of string literals, comments, and Postgres `::` casts.
- Repeated-parameter support: `SELECT :x, :x, :y` binds `x` once.
- Multi-datasource routing with three-tier resolution:
  1. `X-Datasource` request header (when `allow_datasource_header: true`).
  2. `project_datasource_map` entry.
  3. Project name as datasource name.
- Postgres and SQLite pools (dispatched by URL scheme).
- Batch endpoint at `<POST-path>/batch`.
- Health endpoint (`/health` and `/healthz` alias).
- Datasource listing endpoint (`/datasources`) with password masking.
- CORS layer + configurable body-size cap (413 on overflow).
- Structured JSON error responses matching JVM Resql shape.
- Docker image (multi-stage, non-root, tini, self-contained demo).
- CI: tests (matrix amd64 + arm64), security (audit + deny + daily cron), publish (multi-arch, provenance, SBOM, cosign, Trivy), docs (mdBook to Pages).
- mdBook: introduction, getting-started, configuration, sql-files, failure-modes.

### Fixed (vs JVM Resql)

- Datasource-by-project routing (JVM version hardcoded `"byk"`).
- Startup refuses to boot on any misconfigured datasource (JVM version silently ignored several).
- Passwords never appear in config file (JVM defaulted keystore password to `"123456"`).
- Request body cap prevents unbounded memory growth.

### Security

- `deny.toml` bans `openssl`, `openssl-sys`, `serde_yaml` (unmaintained).
- `cargo audit --deny warnings` runs on every push, PR, and daily cron.
- Container image signed with cosign keyless via GHA OIDC.
- Trivy HIGH/CRITICAL scan gates image signing.

[Unreleased]: https://github.com/turnerrainer/Resql-on-Rust/compare/v0.1.0-rc.1...HEAD
[0.1.0-rc.1]: https://github.com/turnerrainer/Resql-on-Rust/releases/tag/v0.1.0-rc.1
