# =============================================================================
# RIM Server — Production Dockerfile
# Multi-stage build: chef → planner → builder → runtime
# (the builder compiles both the server and the `migration` binary)
# =============================================================================

# ---------- Stage 1: Chef (cargo-chef for dependency caching) ----------------
FROM rust:bookworm AS chef
RUN cargo install cargo-chef --locked
WORKDIR /app

# ---------- Stage 2: Planner (compute dependency recipe) ---------------------
FROM chef AS planner
COPY . .
RUN cargo chef prepare --recipe-path recipe.json

# ---------- Stage 3: Builder (compile dependencies, then source) -------------
FROM chef AS builder

# Install build dependencies
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev \
    && rm -rf /var/lib/apt/lists/*

# Cook dependencies first (cached unless Cargo.toml/Cargo.lock change)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy full source and build
COPY . .
RUN cargo build --release -p rim_server -p migration

# ---------- Stage 4: Runtime (minimal Debian image) -------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates libssl3 curl \
    && rm -rf /var/lib/apt/lists/*

# Non-root user for security
RUN groupadd --gid 1001 rim && \
    useradd --uid 1001 --gid rim --create-home --shell /bin/false rim

WORKDIR /app

# Copy binaries from builder
COPY --from=builder /app/target/release/rim_server /app/rim-server
COPY --from=builder /app/target/release/migration /app/migration

# Create data directories for file storage and keys; /data is where the relay compose file
# mounts its volume, so a new volume starts owned by rim
RUN mkdir -p /app/data /data && chown -R rim:rim /app /data

USER rim

# Runtime configuration via environment variables:
#   DATABASE_URL          — PostgreSQL connection string (required)
#   RIM_SERVER_HOST       — Bind address (default: 0.0.0.0)
#   RIM_SERVER_PORT       — Listen port (default: 8080)
#   RUST_LOG              — Log filter, e.g. info or rim_server=debug (default: info)
#   RIM_CORS_ALLOWED_ORIGIN — CORS origin (optional)
#   RIM_DATA_DIR          — Data directory (default: ./data)
#   RIM_SESSION_TTL_DAYS  — Session lifetime (default: 30)
#   RIM_SERVER_KEY_FILE   — Server key file (default: $RIM_DATA_DIR/server-key.json); keep the
#                           data directory on a volume, the key is the server's identity
#   RIM_FRANKING_KEY_FILE — Franking key file (default: $RIM_DATA_DIR/franking-key.json)
#   RIM_PUBLIC_BASE_URL   — https origin; turns server-to-server traffic on. This or
#                           RIM_KEY_LOGIN_AUDIENCES is required: the server binds 0.0.0.0 and
#                           refuses to start without an audience for key logins
#   RIM_KEY_LOGIN_AUDIENCES — comma-separated host[:port] clients reach this server at
#   RIM_ROLE              — relay, host or both (default: both)
#   RIM_REGISTRATION_POW_BITS — turns on proof-of-work signups with that many leading zero bits
#   RIM_RELAY_*           — per-user quotas of a relay; deploy/compose.relay.yml lists them
#   RIM_METRICS_ADDR      — ip:port of the Prometheus /metrics listener (default: off). In a
#                           container bind 0.0.0.0:9090 and do not publish it: keep it on an
#                           internal network, never behind the public proxy
#   RIM_AUDIT_RETENTION_DAYS — days command audit rows are kept, 1-3650 (default: forever); older
#                           rows are exported to $RIM_DATA_DIR/audit-exports and deleted daily

ENV RIM_SERVER_HOST=0.0.0.0 \
    RIM_SERVER_PORT=8080 \
    RIM_DATA_DIR=/app/data \
    RIM_STORAGE_LOCAL_PATH=/app/data/uploads

EXPOSE 8080

# /api accepts only POST. A query (not a mutation) needs no session and no CSRF token.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS -X POST -H 'Content-Type: application/json' \
        --data '{"query":"{__typename}"}' http://localhost:8080/api || exit 1

ENTRYPOINT ["/app/rim-server"]
