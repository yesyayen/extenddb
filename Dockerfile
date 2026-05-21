# Copyright 2026 ExtendDB contributors
# SPDX-License-Identifier: Apache-2.0
#
# Multi-stage build for ExtendDB.
#
#   Stage 1 (builder): cargo build --release --bin extenddb
#   Stage 2 (runtime): debian:bookworm-slim with the static-ish binary
#                       plus tini, ca-certificates, and curl for healthchecks
#
# This image runs `extenddb serve` only. Operator runs `extenddb init`
# explicitly as a separate one-shot before first start. See
# samples/docker/README.md for the bootstrap walkthrough.
#
# Build:
#   docker build -t extenddb:dev .
#
# Run (after init):
#   docker run --rm -p 8000:8000 \
#     -v extenddb-config:/etc/extenddb \
#     -v extenddb-state:/var/lib/extenddb \
#     extenddb:dev

# ---- Stage 1: builder ----
FROM rust:1.88-bookworm AS builder

WORKDIR /src

# Install git so build.rs can read the commit hash.
# pkg-config and libssl-dev are NOT required: ExtendDB uses rustls.
RUN apt-get update \
    && apt-get install -y --no-install-recommends git ca-certificates \
    && rm -rf /var/lib/apt/lists/*

# Copy the entire workspace. .dockerignore strips target/, tests/, docs/,
# samples/, devtools/, .venv/, etc.
COPY . .

# Cache cargo registry across builds via BuildKit.
# Build only the binary; library crates are pulled in transitively.
RUN --mount=type=cache,target=/usr/local/cargo/registry \
    --mount=type=cache,target=/src/target \
    cargo build --release --bin extenddb \
    && cp /src/target/release/extenddb /usr/local/bin/extenddb \
    && /usr/local/bin/extenddb --version

# ---- Stage 2: runtime ----
FROM debian:bookworm-slim AS runtime

ARG EXTENDDB_UID=1000
ARG EXTENDDB_GID=1000

# Runtime dependencies:
#   ca-certificates: for outbound TLS (e.g. to RDS). Server-side TLS uses rustls and needs no system roots.
#   tini:            PID 1 reaper, signal forwarding.
#   curl:            HEALTHCHECK uses it to hit /health. ~250 KB.
RUN apt-get update \
    && apt-get install -y --no-install-recommends \
        ca-certificates \
        tini \
        curl \
    && rm -rf /var/lib/apt/lists/*

# Non-root user. HOME=/var/lib/extenddb so init resolves ~/.extenddb
# to a writeable, container-friendly state directory.
RUN groupadd --system --gid ${EXTENDDB_GID} extenddb \
    && useradd --system --uid ${EXTENDDB_UID} --gid extenddb \
        --home-dir /var/lib/extenddb --shell /usr/sbin/nologin extenddb \
    && mkdir -p /etc/extenddb /var/lib/extenddb \
    && chown -R extenddb:extenddb /etc/extenddb /var/lib/extenddb \
    && chmod 0750 /etc/extenddb /var/lib/extenddb

COPY --from=builder /usr/local/bin/extenddb /usr/local/bin/extenddb
COPY docker/entrypoint.sh /usr/local/bin/extenddb-entrypoint
RUN chmod 0755 /usr/local/bin/extenddb /usr/local/bin/extenddb-entrypoint

USER extenddb
WORKDIR /var/lib/extenddb
ENV HOME=/var/lib/extenddb

# Default DynamoDB API port. Operator can override with EXTENDDB__SERVER__PORT
# at serve time and remap the published port.
EXPOSE 8000

# Container runtimes default to SIGTERM, but stating it explicitly is a
# defence against future Docker default changes. extenddb honors SIGTERM
# (the entrypoint forwards it to the daemon).
STOPSIGNAL SIGTERM

# State volumes:
#   /etc/extenddb:     owns extenddb.toml (written by `init`, read by `serve`)
#   /var/lib/extenddb: owns ~/.extenddb/{tls,run} state
VOLUME ["/etc/extenddb", "/var/lib/extenddb"]

# Healthcheck: curl with -k since the cert is self-signed by default.
# Operators with a CA-signed cert can override at run time.
HEALTHCHECK --interval=10s --timeout=3s --start-period=20s --retries=3 \
    CMD curl -kfsS https://127.0.0.1:8000/health || exit 1

# tini reaps zombies and forwards signals to the entrypoint script.
# Default CMD is "serve"; pass any other extenddb subcommand (e.g.
#   docker run ... extenddb init --pg-host postgres ...
# ) and the entrypoint execs it directly.
ENTRYPOINT ["/usr/bin/tini", "--", "/usr/local/bin/extenddb-entrypoint"]
CMD ["serve"]

# OCI labels (org.opencontainers.image.*). The CI workflow overrides these
# via docker/metadata-action with revision/version/created at publish time;
# the values here are the static fallback for local builds.
LABEL org.opencontainers.image.title="ExtendDB" \
      org.opencontainers.image.description="DynamoDB-compatible API server backed by PostgreSQL" \
      org.opencontainers.image.url="https://github.com/ExtendDB/extenddb" \
      org.opencontainers.image.source="https://github.com/ExtendDB/extenddb" \
      org.opencontainers.image.documentation="https://github.com/ExtendDB/extenddb/tree/main/docs" \
      org.opencontainers.image.licenses="Apache-2.0" \
      org.opencontainers.image.vendor="ExtendDB contributors"
