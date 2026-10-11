# RIM

RIM is a programmable communication workspace. Chat is the medium; the product is what people
build on top of it: a command palette over every action, keybinds for anything, macros that
chain commands, pipelines that react to events, client plugins that add panels and commands,
and a bring-your-own-model AI agent that acts through the same command registry behind a human
confirmation. A person is a key pair rather than an account on one server, DMs and private
groups are end-to-end encrypted, and servers are interchangeable relays and workspace hosts, so
no server is central. It is not a Discord, Slack or Matrix replacement; it is for people who want
a workspace they can program and run themselves. `docs/vision.md` has the product principles and
`ROADMAP.md` the direction.

This repository is the server: Rust (edition 2024), Actix Web and async-graphql, SeaORM on
PostgreSQL 17. The desktop and web clients (Tauri v2 and React) live in the separate `rim-ui`
repository.

## What the server does

- **Chat.** Workspaces; public and private channels with browse, join, leave and archive; DMs
  and groups; threads, replies, edits, deletes and forwards; attachments with per-workspace
  quotas; reactions, pins, mentions, read state, presence and typing; link previews; custom
  emoji; Postgres full-text search; a message change feed for client caches. Contracts:
  `docs/messaging.md`, `docs/search.md`, `docs/message-change-feed.md`, `docs/pagination.md`.
- **Roles and moderation.** Owner, Moderator, Member and Guest; kick, ban, timeout, slow mode,
  moderator deletes and message reports, each written to the audit log (`docs/messaging.md`).
- **Notification rules.** One server-side decision per message: per-conversation levels, keyword
  alerts, do-not-disturb, an inbox and an email digest (`docs/messaging.md`, `docs/email.md`).
- **The programmable layer.** A command registry with typed arguments, sensitivity tiers,
  capability checks and confirmations; user commands and macros with piping; keybinds with
  layers; workspace state (key-value with TTL and events); pipelines; channel lenses. The AI
  agent calls the same registry (`docs/command_model.md`, `docs/keybind_model.md`,
  `docs/context_model.md`, `docs/capability_security_model.md`).
- **Plugins.** The server stores per-workspace plugin registries whose entries carry manifests
  signed by their publisher, and serves web plugin bundles. Plugins themselves run in the client
  (`docs/plugin_model.md`).
- **Key identity.** Identity and device keys with a signed key log, challenge login that issues
  the session cookie, key-only signup, device linking by code, profile cards, rotation and
  recovery (`docs/specs/key-identity-v1.md`, `docs/specs/device-linking-v1.md`).
- **End-to-end encryption.** DMs and private groups use MLS; the server stores key packages,
  orders one commit per epoch and keeps ciphertext, and verifies franked reports of encrypted
  messages. Channels stay plaintext (`docs/specs/e2ee-mls-v1.md`).
- **Relays and hosts.** Every server has its own key and id. A relay keeps its users' accounts
  and queues what other servers send them, with a request inbox for strangers; a host serves
  workspaces to members whose relay is elsewhere, through the user's relay as an untrusted
  courier or directly. A public relay adds proof-of-work signups and abuse tools
  (`docs/server-identity.md`, `docs/relay.md`, `docs/specs/server-to-server-v1.md`).
- **Operations.** `/healthz` and `/readyz` probes, Prometheus metrics on a separate listener,
  audit log retention with export, instance feature switches, background jobs, a compose stack
  with backup and restore (`docs/self-hosting.md`, `docs/operations/`).

## Crates

| Crate | Role |
|---|---|
| `rim_server` | Actix Web startup, config, CORS, CSRF and cookies, the composition root, the CLI, and the non-GraphQL transports (uploads, health, metrics, server-to-server) |
| `rim_api` | async-graphql schema, resolvers, auth context |
| `rim_core` | Domain models, repository traits, strongly-typed ids |
| `rim_services` | Service traits, `*Cmd`/`*Query`/`*Dto` types, `ServiceError` |
| `rim_infra_services` | Service implementations and background jobs |
| `rim_infra_db` | SeaORM repository implementations |
| `entity` | SeaORM entities (generated) plus hand-written mappers |
| `migration` | SeaORM migrations and their CLI |
| `rim_auth` | Password hashing, session and CSRF tokens |
| `rim_identity` | Key identity statements: key logs, device certificates, profile cards, login proofs |
| `rim_s2s` | Server identity and the signed server-to-server transport (RFC 9421 profile) |
| `rim_logging` | Structured tracing setup and the audit log file |

Layering rules and their two documented exceptions are in `CLAUDE.md` and
`docs/architecture.md`.

## Quickstart (local development)

Prerequisites: Rust (the version is pinned in `rust-toolchain.toml`) and Docker or a local
PostgreSQL 17.

```sh
docker run -d --name rim-postgres \
  -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=rim \
  -p 5432:5432 postgres:17-alpine

export DATABASE_URL='postgres://postgres:postgres@localhost:5432/rim'
export RIM_SERVER_HOST=127.0.0.1
cargo run -p migration -- up

# Signups are invite-only: print a single-use signup link for the first account
cargo run -p rim_server -- invite create

# GraphQL at /api, WebSocket at /api/ws, the schema in SDL at /api/sdl
cargo run -p rim_server
```

A server bound to anything but a loopback address needs `RIM_PUBLIC_BASE_URL` or
`RIM_KEY_LOGIN_AUDIENCES`, or it refuses to start (`docs/server-identity.md`). The first start
creates the server key and the franking key file in `RIM_DATA_DIR`, and writes a server recovery
key file there to move offline. The packaged desktop app is accepted from any server; a client
served from another web origin, such as the rim-ui dev server, needs `RIM_CORS_ALLOWED_ORIGIN`. Without `RIM_SMTP_*` email is logged (recipient and subject) instead
of sent.

To deploy a server, follow `docs/self-hosting.md`: `deploy/compose.yml` runs rim-server,
PostgreSQL, the web client and Caddy on one domain, and covers the recovery key, the first
account, updates, backups and monitoring. A public relay adds `deploy/compose.relay.yml`
(`deploy/README.md`).

## Command line

Run without arguments, `rim_server` (`/app/rim-server` in the image) starts the server. The
subcommands read the same configuration and exit:

```text
rim_server --version
rim_server invite create [--email <address>] [--max-uses <1-50>] [--expires-in-days <1-30>] [--public-url <url>]
rim_server invite list
rim_server invite revoke <invite-id>
rim_server server-key show | rotate | recover --recovery-key-file <file> [--new-recovery-key]
rim_server peer reset | block | unblock <server-id>
rim_server franking-key show | rotate
rim_server principal block <principal-id> --reason <text>
rim_server principal unblock <principal-id>
rim_server principal list
rim_server abuse-reports list [--open]
rim_server abuse-reports add [--principal <principal-id>] [--server <server-id>] --reason <text>
rim_server abuse-reports resolve <report-id>
rim_server audit purge [--dry-run] [--older-than-days <1-3650>]
```

`invite create` prints a signup link; with `--email` it admits only a password signup with that
address. `docs/server-identity.md` explains the key and peer commands, `docs/relay.md` and
`docs/operations/relay-public.md` the abuse tools, `docs/operations/audit-retention.md` the
purge.

## Configuration

The server reads its configuration from the environment, or from an untracked `.env` file in
development. `DATABASE_URL` is the only variable without the `RIM_` prefix; the log filter comes
from `RUST_LOG` (default `info`). The comment block above `ENV` in the `Dockerfile` lists the
variables with their defaults, and `deploy/.env.example` lists the ones the compose stack sets.
`rim_server/src/config.rs` is the source of truth. Four more are set by `deploy/compose.yml` or
needed rarely:

| Variable | Effect |
|---|---|
| `RIM_TRUST_FORWARDED_HEADERS` | `true` behind a reverse proxy that is the only way in, so per-IP limits see the client's address |
| `RIM_FORCE_SECURE_COOKIES` | `true` marks session and CSRF cookies `Secure` whatever the request scheme |
| `RIM_DATABASE_MAX_CONNECTIONS` | Size of the PostgreSQL pool (default 10) |
| `RIM_AUDIT_LOG_PATH` | Directory of `audit.log` (default: the working directory) |

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# The integration suites need a running PostgreSQL and DATABASE_URL
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

`CONTRIBUTING.md` has the rules for a pull request and what each gate checks; `CLAUDE.md` has
the conventions, the migration and entity-generation rules, and the GraphQL schema export for
rim-ui.

## Specifications

The normative contracts, for anyone building a client or another server:

- `docs/specs/key-identity-v1.md`: keys, signed statements, challenge login
- `docs/specs/device-linking-v1.md`: giving a new device its certified key
- `docs/specs/e2ee-mls-v1.md`: end-to-end encryption with MLS
- `docs/specs/server-to-server-v1.md`: server identity, the signed HTTP API, relays and hosts
- `docs/specs/plugin-protocol-v1.md`: the client plugin protocol
- `docs/specs/web-plugins-v1.md`: sandboxed web plugins
- `docs/specs/workspace-plugins-v1.md`: signed manifests and workspace plugin registries

`docs/README.md` lists every document in `docs/`.

## License

Apache License 2.0, see `LICENSE` and `NOTICE`. The license does not cover the RIM name, so
forks should use a different name. To contribute, see `CONTRIBUTING.md`.
