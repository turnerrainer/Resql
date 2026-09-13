# syntax=docker/dockerfile:1.7-labs

# ─────────────────────────────────────────────────────────────
# Build stage — musl cross-compile via messense/rust-musl-cross.
# One builder variant per target arch (BuildKit picks via
# `$TARGETARCH` — auto-set by `docker buildx build --platform
# linux/amd64,linux/arm64`; the ARG declaration below is what
# lets us interpolate it into the FROM line). The messense image
# ships pre-built musl toolchains for both arches so
# libsqlite3-sys builds bundled SQLite against musl headers/libs
# cleanly.
# ─────────────────────────────────────────────────────────────
ARG TARGETARCH

FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:x86_64-musl AS builder-amd64
ENV RUSTC_TARGET=x86_64-unknown-linux-musl

FROM --platform=$BUILDPLATFORM messense/rust-musl-cross:aarch64-musl AS builder-arm64
ENV RUSTC_TARGET=aarch64-unknown-linux-musl

FROM builder-${TARGETARCH} AS builder
WORKDIR /build
COPY Cargo.toml Cargo.lock ./
COPY src ./src
RUN cargo build --release --locked --bin resql --target $RUSTC_TARGET \
    && cp target/$RUSTC_TARGET/release/resql /tmp/resql \
    && strip /tmp/resql

# ─────────────────────────────────────────────────────────────
# Runtime — Google distroless/static. Package set: ca-certificates
# + tzdata + /etc/passwd. NO libc, NO libssl, NO libgcc — a fully
# static musl binary needs none of them, and their absence closes
# every glibc / OpenSSL CVE that would otherwise land in the Trivy
# report. Target: 0 findings at any severity.
#
# distroless/static has no shell + no tini; the resql binary runs
# as PID 1. That's safe because (a) we don't fork child processes
# (no zombies to reap), and (b) tokio's shutdown_signal() handler
# already forwards SIGTERM/SIGINT to a graceful axum shutdown.
# ─────────────────────────────────────────────────────────────
FROM gcr.io/distroless/static-debian12:nonroot
WORKDIR /app

COPY --from=builder --chown=nonroot:nonroot /tmp/resql /app/resql

# Ship a working multi-database demo out of the box: two SQLite datasources
# (`users` and `audit`) wired to two URL projects. Operators mount their
# own resql.yaml + sql/ in production; the demo just proves the wiring works.
COPY --chown=nonroot:nonroot resql.yaml /app/resql.yaml
COPY --chown=nonroot:nonroot sql /app/sql

EXPOSE 8080
ENV RESQL_CONFIG=/app/resql.yaml

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD ["/app/resql", "health"]

CMD ["/app/resql"]
