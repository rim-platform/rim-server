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

# Cook dependencies first (cached unless Cargo.toml/Cargo.lock change)
COPY --from=planner /app/recipe.json recipe.json
RUN cargo chef cook --release --recipe-path recipe.json

# Copy full source and build
COPY . .
RUN cargo build --release -p rim_server -p migration

# ---------- Stage 4: Runtime (minimal Debian image) -------------------------
FROM debian:bookworm-slim AS runtime

RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates curl \
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
#   RIM_CORS_ALLOWED_ORIGIN — one browser origin accepted besides the server's own (optional);
#                           the own origin is RIM_PUBLIC_BASE_URL's, else the request's Host
#   RIM_DATA_DIR          — Data directory (default: ./data)
#   RIM_STORAGE_LOCAL_PATH — Directory of uploaded files (default: $RIM_DATA_DIR/files; this
#                           image sets /app/data/uploads)
#   RIM_MAX_WORKSPACE_STORAGE_BYTES — Bytes of attachments and plugin bundles one workspace may
#                           store (default: 1073741824)
#   RIM_DATABASE_MAX_CONNECTIONS — Size of the PostgreSQL pool (default: 10)
#   RIM_TRUST_FORWARDED_HEADERS — Read the client address from X-Forwarded-For / Forwarded when
#                           true; only behind a reverse proxy that is the sole way in
#                           (default: false)
#   RIM_FORCE_SECURE_COOKIES — Mark session and CSRF cookies Secure whatever the request scheme
#                           when true (default: false)
#   RIM_AUDIT_LOG_PATH    — Directory of audit.log (default: the working directory)
#   RIM_SESSION_TTL_DAYS  — Session lifetime (default: 30)
#   RIM_SERVER_KEY_FILE   — Server key file (default: $RIM_DATA_DIR/server-key.json); keep the
#                           data directory on a volume, the key is the server's identity
#   RIM_FRANKING_KEY_FILE — Franking key file (default: $RIM_DATA_DIR/franking-key.json)
#   RIM_PUBLIC_BASE_URL   — https origin; turns server-to-server traffic on. This or
#                           RIM_KEY_LOGIN_AUDIENCES is required: the server binds 0.0.0.0 and
#                           refuses to start without an audience for key logins
#   RIM_KEY_LOGIN_AUDIENCES — comma-separated host[:port] clients reach this server at
#   RIM_HANDLE_DOMAIN     — domain of handles, @<username>:<domain> (default: the host of
#                           RIM_PUBLIC_BASE_URL); another domain delegates to this server
#   RIM_ROLE              — relay, host or both (default: both)
#   RIM_REGISTRATION_POW_BITS — turns on proof-of-work signups with that many leading zero bits
#   Per-user quotas of a relay:
#   RIM_RELAY_QUEUE_BYTES_PER_USER — bytes queued per local user before the oldest items are
#                           dropped, at least 1048576 (default: 268435456)
#   RIM_RELAY_RETENTION_DAYS — days queued items and DM requests are kept, 1-365 (default: 30)
#   RIM_RELAY_CARD_MAX_BYTES — largest profile card, 1024-131072 (default: 131072)
#   RIM_RELAY_KEY_PACKAGES_PER_DEVICE — unclaimed key packages per device, 1-1000 (default: 100)
#   RIM_RELAY_DEVICES_PER_USER — devices per user that are not revoked, 1-64 (default: 32)
#   RIM_RELAY_FORWARDS_PER_USER_PER_DAY — requests a user sends through /api/remote per day
#                           (default: 50000)
#   RIM_RELAY_DM_REQUESTS_PER_DAY — DM requests one sender files here, and invites a local user
#                           sends to other servers, per day (default: 20)
#   RIM_ENABLE_GRAPHIQL   — Serve the GraphiQL explorer at GET /api when true (default: false)
#   RIM_OPEN_REGISTRATION — Let anyone sign up, rate-limited, when true (default: invite-only)
#   RIM_SIGNUP_REQUIRE_KEYS — Refuse every password signup, so accounts sign up with a key
#                           (default: true); password sign-in of existing accounts still works
#   RIM_DEFAULT_WORKSPACE — id of a workspace here every new account joins as a member
#                           (default: none); one that does not exist stops the server at boot
#   RIM_NOTIFICATION_DIGEST_INTERVAL_SECS — Seconds between digest runs, at least 60 (default: 900)
#   RIM_SMTP_HOST         — SMTP server; without it or RIM_SMTP_FROM mail is only logged
#   RIM_SMTP_PORT         — SMTP port (default: 587)
#   RIM_SMTP_USERNAME     — SMTP user name (default: none, unauthenticated)
#   RIM_SMTP_PASSWORD     — SMTP password (default: none)
#   RIM_SMTP_FROM         — From address, e.g. RIM <noreply@example.org>
#   RIM_SMTP_TLS          — starttls, implicit or none (default: starttls)
#   RIM_HOST_ALLOWED_RELAYS — Server ids allowed on host endpoints, comma-separated (default: any)
#   RIM_DM_INBOUND        — DMs from other servers, and from people here who share no workspace
#                           with the recipient: open, requests (their first message is a request
#                           the recipient accepts), partners (only people on the RIM_DM_PARTNERS
#                           servers and this one, with a request) or none (default: requests);
#                           with partners or none other servers get no identity of anyone here
#   RIM_DM_OUTBOUND       — DMs people here start with people on other servers: allowed, partners
#                           (only with people on the RIM_DM_PARTNERS servers) or none
#                           (default: allowed)
#   RIM_DM_PARTNERS       — Server ids the partners settings of RIM_DM_INBOUND and
#                           RIM_DM_OUTBOUND name, comma-separated (default: none)
#   RIM_S2S_DENIED_PEERS  — Server ids refused on every server-to-server endpoint, comma-separated
#   RIM_S2S_PEER_RATE_PER_MIN — Server-to-server requests per peer per minute (default: 600)
#   RIM_S2S_DEVICE_RATE_PER_MIN — Forwarded requests per device per minute (default: 120)
#   RIM_S2S_SUBSCRIPTIONS_PER_PEER — Delegations one relay may hold on this host (default: 10000)
#   RIM_S2S_MAX_BODY_BYTES — Largest server-to-server request body (default: 8388608)
#   RIM_S2S_PEER_DEAD_AFTER_HOURS — Hours of failed deliveries before a relay is dead and gets nothing queued, 1 to 168 (default: 24)
#   RIM_S2S_ALLOW_HTTP    — Allow server-to-server traffic over http when true; development only
#   RIM_S2S_ALLOW_PRIVATE_ADDRESSES — Allow peers on private addresses when true; development only
#   RIM_METRICS_ADDR      — ip:port of the Prometheus /metrics listener (default: off). In a
#                           container bind 0.0.0.0:9090 and do not publish it: keep it on an
#                           internal network, never behind the public proxy
#   RIM_AUDIT_RETENTION_DAYS — days command audit rows are kept, 1-3650 (default: forever); older
#                           rows are exported to $RIM_DATA_DIR/audit-exports and deleted daily
#   RIM_FEATURES_DISABLED — comma-separated optional features to switch off: attachments,
#                           link_previews, email, custom_emoji, workspace_plugins, ai_commands
#                           (default: none); an unknown name stops the server at boot
#   RIM_LINK_PREVIEW_CONCURRENCY — link preview fetches the server runs at once, 1-64
#                           (default: 8); outside the range stops the server at boot
#   RIM_RATE_REQUESTS_PER_MIN_PER_IP — requests per client address per minute to /api routes,
#                           with a burst of the same size (default: off); over it 429
#   RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION — mutation fields per session per minute (default: off)
#   RIM_RATE_MESSAGES_PER_MIN_PER_USER — messages per user per minute (default: off)
#   RIM_RATE_MESSAGES_BURST — messages a user may send at once (default: the per-minute number)
#   RIM_RATE_WS_CONNECTIONS_PER_USER — open WebSocket connections per user (default: off)
#   RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION — subscriptions per WebSocket connection (default: off)
#   RIM_WS_SESSION_RECHECK_SECS — seconds between rechecks of the sessions WebSocket connections
#                           run under, closing those that ended elsewhere (default: 30, 0 = off)
#   RIM_API_MAX_BATCH     — GraphQL requests in one batch (default: 20)
#   RIM_INSTANCE_NOTICE   — a short text shown to people on this instance, at most 500
#                           characters (default: none); longer stops the server at boot
#   RIM_TERMS_URL         — absolute http(s) URL of the terms of service; set, every signup must
#                           accept it (default: none); a malformed URL stops the server at boot
#   RIM_TERMS_FILE        — path of a Markdown file of the terms of service, served at /api/terms,
#                           which every signup then accepts (default: none); a missing, empty or
#                           oversized file, a FILL: placeholder in it, or RIM_TERMS_URL set too
#                           stops the server at boot
#   RIM_ABUSE_CONTACT     — email address or https URL for abuse reports, in the signed
#                           descriptor and serverInfo (default: none); anything else stops the
#                           server at boot
#   RIM_MINIMUM_AGE       — age, 13-21, every signup confirms the person has reached; self-
#                           declared (default: none); outside the range stops the server at boot
#   Instance limits, each off when unset or 0; past one a write answers VALIDATION:
#   RIM_LIMIT_WORKSPACES_PER_USER — workspaces one user owns
#   RIM_LIMIT_MEMBERS_PER_WORKSPACE — members of a workspace (the direct space has none)
#   RIM_LIMIT_CHANNELS_PER_WORKSPACE — channels of a workspace, public and private
#   RIM_LIMIT_DEVICES_PER_USER — devices on one account; the stricter of this and
#                           RIM_RELAY_DEVICES_PER_USER applies
#   RIM_LIMIT_DM_REQUESTS_PER_DAY — DM requests to strangers per user per day; the stricter of
#                           this and RIM_RELAY_DM_REQUESTS_PER_DAY applies
#   RIM_LIMIT_MACROS_PER_WORKSPACE — macros of a workspace
#   RIM_LIMIT_PIPELINES_PER_WORKSPACE — pipelines of a workspace
#   RIM_LIMIT_PLUGINS_PER_WORKSPACE — plugins of a workspace's registry
#   RIM_LIMIT_INVITES_PER_USER_PER_DAY — workspace invites one user creates per day
#   RIM_LIMIT_INVITE_MAX_USES — the uses one invite may have
#   RIM_LIMIT_MESSAGE_BYTES — bytes of a message's text, at most 65536; more stops the server
#                           at boot
#   RIM_LIMIT_PENDING_INVITES_PER_USER — pending invitations to encrypted conversations one user
#                           holds; unlike the others it is on unless set to 0 (default: 50)
#   RIM_LIMIT_ACCOUNTS    — local accounts the instance holds, those in their deletion grace
#                           period included; at it every signup answers REGISTRATION_FULL
#   RIM_PROBATION_HOURS   — hours after creation during which an account creates no workspace
#                           and sends no DM request to a stranger (default: off)
#   RIM_RATE_PROBATION_MESSAGES_PER_MIN — messages per minute of an account in probation,
#                           instead of the normal rate (default: the normal rate)
#   RIM_ACCOUNT_DELETION_GRACE_DAYS — days between an account's deletion request and its purge,
#                           0-30 (default: 14); outside the range stops the server at boot
#   RIM_RESERVED_NAMES    — comma-separated usernames and display names signups and profile
#                           edits refuse, also followed by -, _, . or a space (default: none);
#                           an empty entry stops the server at boot
#   RIM_PASSWORD_HASH_CONCURRENCY — Argon2 password hashes and checks run at once, 1-64
#                           (default: the number of CPUs); each holds 64 MiB while it runs, and
#                           a request that waits over 10 seconds for a turn answers UNAVAILABLE
#
# Probes, unauthenticated, on RIM_SERVER_PORT:
#   GET /healthz          — 200 {"status":"ok","version":…} while the process runs
#   GET /readyz           — 200 when the database answers, no migration is pending and the
#                           server and franking key files are readable; else 503 naming the
#                           failing checks
#
# `/app/rim-server --version` prints the release version.

ENV RIM_SERVER_HOST=0.0.0.0 \
    RIM_SERVER_PORT=8080 \
    RIM_DATA_DIR=/app/data \
    RIM_STORAGE_LOCAL_PATH=/app/data/uploads

EXPOSE 8080

# Liveness: /healthz answers 200 while the process serves requests. Readiness (database,
# pending migrations, key files) is /readyz; the compose files probe that one.
HEALTHCHECK --interval=30s --timeout=5s --start-period=10s --retries=3 \
    CMD curl -fsS http://localhost:8080/healthz || exit 1

ENTRYPOINT ["/app/rim-server"]
