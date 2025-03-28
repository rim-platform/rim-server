# Stage 1: Build environment
FROM rust:1.85-slim-bullseye AS builder
LABEL stage=builder
WORKDIR /app

RUN apt-get update && apt-get install -y libssl-dev pkg-config && rm -rf /var/lib/apt/lists/*

COPY . .
RUN cargo build --release

# Stage 2: Runtime environment
FROM debian:bullseye-slim AS production
WORKDIR /app

# Runtime dependencies
RUN apt-get update && apt-get install -y ca-certificates libssl-dev && rm -rf /var/lib/apt/lists/*

# Non-root user setup
RUN addgroup --system rust && adduser --system --group rust
USER rust:rust

# Copy binary
COPY --from=builder /app/target/release/rim-server /app/rim-server

# Environment variables
ENV RUST_LOG=info
ENV APP_PORT=8000

# Expose application port
EXPOSE ${APP_PORT}

# Run the application with the provided port
CMD ["./rim-server"]
