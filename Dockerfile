# syntax=docker/dockerfile:1.7-labs

# ─────────────────────────────────────────────────────────────
# Build stage — full toolchain, cached target dir.
# ─────────────────────────────────────────────────────────────
FROM rust:1.88-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config libssl-dev ca-certificates \
    && rm -rf /var/lib/apt/lists/*

COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin resql-on-rust

# ─────────────────────────────────────────────────────────────
# Runtime stage — minimal Debian, non-root, tini, self-contained.
# ─────────────────────────────────────────────────────────────
FROM debian:bookworm-slim
WORKDIR /app
RUN apt-get update && apt-get install -y --no-install-recommends \
        libssl3 ca-certificates tini curl \
    && rm -rf /var/lib/apt/lists/*

COPY --from=builder /build/target/release/resql-on-rust /app/resql-on-rust

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
CMD ["/app/resql-on-rust"]
