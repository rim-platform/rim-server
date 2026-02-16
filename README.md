# RIM

RIM is a desktop-first, modular chat workspace. The long-term goal is to keep the backend stable
while enabling teams and individuals to extend the desktop UI with plugins (panels, message actions,
composer actions) and integrations.

This repository currently contains the Rust server workspace (Actix Web + async-graphql) and the
supporting domain/service/db crates.

## Project Direction

- Desktop-first client: planned to be built with Tauri for low CPU/RAM usage.
- Extensibility: a plugin host in the desktop client (sandboxed, capability-based permissions).
- Integrations: Slack/Discord-style external apps (webhooks + APIs) come later.
- Policies: server-enforced moderation/restrictions (e.g., kids mode) come later.

See `ROADMAP.md` for the implementation plan.

## Workspace Layout

- `rim_server`: Actix Web server wiring + GraphQL endpoint
- `rim_api`: async-graphql schema and handlers
- `rim_core`: domain models + repository traits + shared macros (IDs)
- `rim_services`: service-layer types, DTOs/commands/queries, `ServiceError`
- `rim_infra_db`: SeaORM-backed repository implementations
- `rim_infra_services`: infra service implementations
- `entity`: SeaORM entities (generated)
- `migration`: SeaORM migrations + CLI

## Quickstart (Local)

Prereqs:

- Rust toolchain (edition 2024)
- Postgres (CI uses Postgres 17)

Start Postgres (example):

```sh
docker run --rm --name rim-postgres \
  -e POSTGRES_USER=postgres \
  -e POSTGRES_PASSWORD=postgres \
  -e POSTGRES_DB=testdb \
  -p 5432:5432 \
  postgres:17-alpine

export DATABASE_URL='postgres://postgres:postgres@localhost:5432/testdb'
```

Run migrations:

```sh
cargo run -p migration -- up
```

Run the server:

```sh
cargo run -p rim_server
```

## Common Commands

```sh
# Build
cargo build --workspace

# Format + lint (CI style)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace
```

## Docs

- `ROADMAP.md`
- `docs/pagination.md`
