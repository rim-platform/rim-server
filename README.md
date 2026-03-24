# RIM

RIM is a programmable communication workspace — not another chat app.

Chat is the medium, but the product is defined by commands, keybinds, macros, plugins,
and context actions that turn conversations into workflows. Think VS Code meets team
communication: a command palette, user-defined shortcuts, chainable macros, and a
client-side plugin system with a marketplace.

This repository contains the Rust backend workspace (Actix Web + async-graphql) and the
supporting domain/service/db crates.

## Why RIM Exists

Every team chat app dictates how you work. RIM lets you build your own workflow:

- **Command palette** — fuzzy-search any action, not just channels.
- **Custom keybinds** — bind any command to any key, scoped by context.
- **Macro commands** — chain multiple actions into one keystroke.
- **Client-side plugins** — extend the app itself, not just post messages.
- **Tauri v2** — lightweight desktop runtime, not Electron.

See `docs/vision.md` for the full product vision and competitive positioning.

## Project Direction

- Desktop-first client built with Tauri v2 for low CPU/RAM usage.
- Keyboard-first UX — the user should never need a mouse for common workflows.
- Extensibility through a plugin host in the desktop client (sandboxed, capability-based).
- Communication is the foundation, not the product — commands, keybinds, and plugins
  are the product.
- Integrations (webhooks, APIs, bots) come in later phases.

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
sea migrate up
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

- `docs/vision.md` — product vision and competitive positioning
- `ROADMAP.md` — implementation phases
- `docs/architecture.md` — system architecture
- `docs/command_model.md` — command system contract
- `docs/keybind_model.md` — keybind system contract
- `docs/plugin_model.md` — plugin system contract
- `docs/capability_security_model.md` — capability and security model
- `docs/context_model.md` — typed runtime context
- `docs/pagination.md` — cursor pagination contract
