# syntax=docker/dockerfile:1.7-labs

# ─────────────────────────────────────────────────────────────
# Build stage — cross-compile Rust → musl for TARGETARCH using
# cargo-zigbuild (zig as C cross-compiler). libsqlite3-sys's
# bundled SQLite builds cleanly for either amd64 or arm64 from a
# single amd64 host, no QEMU emulation.
#
# `--platform=$BUILDPLATFORM` pins the builder to the runner's
# native arch (amd64 on GitHub-hosted runners). buildx still spawns
# one build per --platform value in the outer invocation; each gets
# its own auto-set TARGETARCH inside the RUN below.
#
# NOTE: the previous attempt used per-arch builder stages selected
# via `FROM builder-${TARGETARCH}`. BuildKit resolves the stage
# graph BEFORE per-stage TARGETARCH is populated, so that pattern
# fails at graph-solve time ("failed to parse stage name
# 'builder-': invalid reference format"). Single builder + runtime
# arch-dispatch inside RUN is the working shape.
# ─────────────────────────────────────────────────────────────
FROM --platform=$BUILDPLATFORM rust:1.88-slim AS builder
WORKDIR /build

RUN apt-get update && apt-get upgrade -y && apt-get install -y --no-install-recommends \
        ca-certificates curl xz-utils \
    && rm -rf /var/lib/apt/lists/*

# Zig, pinned. Used by cargo-zigbuild as the C cross-compiler for
# libsqlite3-sys' bundled SQLite build.
ARG ZIG_VERSION=0.13.0
RUN curl -fsSL "https://ziglang.org/download/${ZIG_VERSION}/zig-linux-x86_64-${ZIG_VERSION}.tar.xz" \
        | tar -xJ -C /opt \
    && ln -s "/opt/zig-linux-x86_64-${ZIG_VERSION}/zig" /usr/local/bin/zig

# cargo-zigbuild wraps cargo build with zig-as-CC for the target.
# --locked avoids picking up unexpected dep updates during install.
RUN cargo install --locked cargo-zigbuild --version 0.23.4 \
    && rustup target add x86_64-unknown-linux-musl aarch64-unknown-linux-musl

COPY Cargo.toml Cargo.lock ./
COPY src ./src

# TARGETARCH is auto-populated by buildx per platform invocation.
# Cargo.toml already has `strip = "symbols"` in [profile.release] so
# no explicit strip step is needed (cross-arch strip on an amd64 host
# would need target-specific binutils anyway).
ARG TARGETARCH
RUN case "$TARGETARCH" in \
        amd64) TARGET=x86_64-unknown-linux-musl ;; \
        arm64) TARGET=aarch64-unknown-linux-musl ;; \
        *) echo "unsupported TARGETARCH: '$TARGETARCH'" >&2 && exit 1 ;; \
    esac \
    && cargo zigbuild --release --locked --bin resql --target "$TARGET" \
    && cp "target/${TARGET}/release/resql" /tmp/resql

# ─────────────────────────────────────────────────────────────
# Runtime — Google distroless/static. ca-certificates + tzdata +
# /etc/passwd, nothing else. Trivy scan target: 0 findings, any
# severity, any status.
#
# distroless/static has no shell + no tini; the resql binary runs
# as PID 1. Safe because (a) we don't fork children (no zombies to
# reap), and (b) tokio's shutdown_signal() forwards SIGTERM/SIGINT
# to a graceful axum shutdown.
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
