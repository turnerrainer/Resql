# syntax=docker/dockerfile:1.7-labs

# ─────────────────────────────────────────────────────────────
# Build stage — full toolchain, cached target dir.
# ─────────────────────────────────────────────────────────────
FROM rust:1.88-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin resql

# ─────────────────────────────────────────────────────────────
# Runtime stage — minimal Debian, non-root, tini, self-contained.
# ─────────────────────────────────────────────────────────────
# `apt-get upgrade -y` pulls Debian security patches on top of the base
# image so time-lagged base-image releases don't ship known-fixed CVEs.
# Without this, Trivy's `--ignore-unfixed --severity HIGH,CRITICAL` gate
# in publish.yml can fire on issues Debian has ALREADY patched but the
# base image hasn't yet rebuilt (e.g. CVE-2026-86145 / CVE-2026-89161 in
# libpcre2 blocked v0.4.0-alpha publish on 2026-09-13).
FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates tini curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/resql /app/resql

# Ship a working multi-database demo out of the box: two SQLite datasources
# (`users` and `audit`) wired to two URL projects. Operators mount their
# own resql.yaml + sql/ in production; the demo just proves the wiring works.
COPY resql.yaml /app/resql.yaml
COPY sql /app/sql

EXPOSE 8080
RUN useradd -m -u 1000 resql && chown -R resql:resql /app
USER resql
ENV RESQL_CONFIG=/app/resql.yaml

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://127.0.0.1:8080/health || exit 1

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/resql"]
