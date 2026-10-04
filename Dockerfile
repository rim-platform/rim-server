# =============================================================================
# RIM Server — Production Dockerfile
# Multi-stage build: chef → planner → builder → migrator → runtime
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

# Create data directory for file storage
RUN mkdir -p /app/data && chown -R rim:rim /app

USER rim

# Runtime configuration via environment variables:
#   DATABASE_URL          — PostgreSQL connection string (required)
#   RIM_SERVER_HOST       — Bind address (default: 0.0.0.0)
#   RIM_SERVER_PORT       — Listen port (default: 8080)
#   RIM_LOG_LEVEL         — Log level (default: INFO)
#   RIM_CORS_ALLOWED_ORIGIN — CORS origin (optional)
#   RIM_DATA_DIR          — Data directory (default: ./data)
#   RIM_STORAGE_BACKEND   — Storage backend: local | s3 (default: local)
#   RIM_SESSION_TTL_DAYS  — Session lifetime (default: 30)

ENV RIM_SERVER_HOST=0.0.0.0 \
    RIM_SERVER_PORT=8080 \
    RIM_LOG_LEVEL=INFO \
    RIM_DATA_DIR=/app/data \
    RIM_STORAGE_BACKEND=local \
    RIM_STORAGE_LOCAL_PATH=/app/data/uploads

EXPOSE 8080

HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -f http://localhost:8080/api?query=%7B__typename%7D || exit 1

ENTRYPOINT ["/app/rim-server"]
