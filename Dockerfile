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
# Auxiliary stage — grab a `tini` binary to use as PID 1 in the
# distroless runtime (distroless/cc has no package manager).
# ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim AS tini-source
RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
        tini \
    && rm -rf /var/lib/apt/lists/*

# ─────────────────────────────────────────────────────────────
# Runtime stage — Google distroless (glibc + libssl3 + ca-certs
# baked in, no shell, no apt, no curl). Container drops from
# ~107 Debian packages to ~15, closing every fixable + unfixable
# HIGH/CRITICAL CVE that landed via curl / libldap / libkrb5 /
# libnghttp2 / perl-base / util-linux-extra on the previous
# debian:bookworm-slim base.
#
# The `:nonroot` tag runs as UID 65532 by default. HEALTHCHECK
# uses the built-in `resql health` subcommand instead of curl,
# which does a raw std::net::TcpStream + HTTP/1.0 GET — no shell
# and no external binary needed.
# ─────────────────────────────────────────────────────────────
FROM gcr.io/distroless/cc-debian12:nonroot
WORKDIR /app

COPY --from=builder --chown=nonroot:nonroot /build/target/release/resql /app/resql
COPY --from=tini-source /usr/bin/tini /usr/bin/tini

# Ship a working multi-database demo out of the box: two SQLite datasources
# (`users` and `audit`) wired to two URL projects. Operators mount their
# own resql.yaml + sql/ in production; the demo just proves the wiring works.
COPY --chown=nonroot:nonroot resql.yaml /app/resql.yaml
COPY --chown=nonroot:nonroot sql /app/sql

EXPOSE 8080
ENV RESQL_CONFIG=/app/resql.yaml

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/resql", "health"]

ENTRYPOINT ["/usr/bin/tini", "--"]
CMD ["/app/resql"]
