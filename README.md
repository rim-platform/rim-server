# RIM

RIM is a self-hosted, programmable communication workspace for dev, SRE, and platform teams.

Chat is the substrate. On top of it sits a programmable runtime — a command palette, typed
commands, chainable macros, reactive pipelines, user-defined keybinds, and capability-gated
client plugins — and a native incident layer built on that runtime. A bring-your-own-model AI
agent speaks the same command registry, so it can *operate* the workspace (acknowledge,
escalate, take an on-call shift) with every mutating step behind a human confirmation.

The go-to-market story is **"Slack + incident.io in one self-hosted box, with an AI agent that
can actually run the incident."** The programmable runtime is the moat underneath it. See
`ROADMAP.md` for the strategy and `docs/vision.md` for the product principles.

This repository is the Rust backend (Actix Web + async-graphql + SeaORM on PostgreSQL 17). The
Tauri v2 desktop client lives in the separate `rim-ui` repository.

## What's in the backend today

- **Communication core** — workspaces, channels, DMs/groups, threads, attachments, reactions,
  pins, mentions, read state, presence, typing, link previews, custom emoji, Postgres full-text
  search, invite-only registration with operator bootstrap, password reset, SMTP email.
- **Programmable runtime** — command registry with typed args and sensitivity tiers, user
  commands, macros with piping, keybinds, workspace state (KV + TTL + events), pipelines, lenses.
- **Incidents** — phases, severity, timeline notes, action items, AI postmortem draft.
- **On-call** — schedules, rotation layers, overrides, escalation policies with timers.
- **Alert ingestion** — webhook receivers for generic JSON, Prometheus Alertmanager, and Grafana.
- **AI operator** — incident and on-call commands exposed to the agent, gated by capability
  checks and single-use confirmation proofs, recorded in the incident timeline.

## Workspace layout

| Crate | Role |
|---|---|
| `rim_server` | Actix Web bootstrap, config, CORS/CSRF/cookies, composition root, non-GraphQL transports (uploads, alert webhooks) |
| `rim_api` | async-graphql schema, resolvers, auth context |
| `rim_core` | Domain models, repository traits, strongly-typed IDs |
| `rim_services` | Service traits, `*Cmd`/`*Query`/`*Dto` types, `ServiceError` |
| `rim_infra_services` | Service implementations |
| `rim_infra_db` | SeaORM repository implementations |
| `entity` | SeaORM entities (generated) plus hand-written mappers |
| `migration` | SeaORM migrations + CLI |
| `rim_auth` | Password hashing, session/CSRF tokens |
| `rim_logging` | Structured tracing setup |

Layering rules and the two documented exceptions are in `CLAUDE.md` and `docs/architecture.md`.

## Quickstart (local)

Prereqs: Rust (pinned in `rust-toolchain.toml`), Docker or a local PostgreSQL 17, and
`sea-orm-cli` for migrations.

```sh
docker run -d --name rim-postgres \
  -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=testdb \
  -p 5432:5432 postgres:17-alpine

export DATABASE_URL='postgres://postgres:postgres@localhost:5432/testdb'
sea migrate up

# Issue a single-use signup link for the first (operator) account
cargo run -p rim_server -- bootstrap-user --email you@example.com

# Run the server (GraphQL at /api, WebSocket at /api/ws, SDL at /api/sdl)
cargo run -p rim_server
```

Configuration is read from the environment (or a local, untracked `.env`) — see
`rim_server/src/config.rs`. The essentials are `DATABASE_URL`, `RIM_SERVER_HOST`,
`RIM_SERVER_PORT`, and `RIM_LOG_LEVEL`. Email delivery needs `RIM_SMTP_*` (`docs/email.md`);
without it, emails are logged (recipient and subject only) instead of sent. Behind a reverse
proxy, set `RIM_TRUST_FORWARDED_HEADERS=true` (and make the proxy the only entry point);
otherwise per-IP limits on login and signup see every client as the proxy's address.

## Development

```sh
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace

# Integration suites need a running Postgres and DATABASE_URL
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

CI runs fmt, clippy, the full test suite single-threaded, a 95% line-coverage gate,
`cargo audit`, and a Docker build. See `CLAUDE.md` for migration and entity-generation rules.

## Docs

Product and plan:
- `docs/vision.md` — product principles and positioning
- `ROADMAP.md` — strategy, tracks, and open decisions

Contracts (current):
- `docs/architecture.md` — system architecture
- `docs/command_model.md`, `docs/keybind_model.md`, `docs/plugin_model.md`, `docs/context_model.md`
- `docs/capability_security_model.md` — capabilities, sensitivity tiers, confirmations
- `docs/pagination.md` — cursor pagination contract
- `docs/search.md`, `docs/alerts.md`, `docs/email.md`, `docs/oncall.md` — feature references

Dated records (point-in-time, may be stale): `docs/specs/`, `docs/design/`, `docs/research/`,
`docs/decisions/`, `docs/security/`, and `docs/next-steps-*.md`.
