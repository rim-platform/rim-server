# Agent Guide (rim-server)

## Product Identity

RIM is a programmable communication workspace, not another chat app. Chat is the
communication medium; the product is commands, keybinds, macros, and plugins. See
`docs/vision.md` for the full product vision. Agents working in this repo should
understand that they are not building a Slack/Discord alternative — they are building
the backend for a programmable workspace platform.

This repo is a multi-crate Rust workspace (edition 2024). Most work happens in `rim_server`,
`rim_api`, `rim_core`, `rim_services`, `rim_infra_db`, `entity`, and `migration`.

Workspace note: the root `Cargo.toml` is a virtual manifest (no root package), so plain `cargo`
commands at the root cover every member. Use `-p <crate>` to scope.

## Build / Run / Lint / Test

```sh
# Build
cargo build --workspace
cargo build -p rim_server

# Run
cargo run -p rim_server
cargo run -p migration -- up

# Format + lint (CI style)
cargo fmt --all
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace
cargo test --all -- --test-threads=1
```

## Running A Single Test

Rust test selection is pattern-based; scope to a package first.

```sh
# Unit test by substring match
cargo test -p rim_services send_message

# Specific test path (when you know it)
cargo test -p rim_services conversation::tests::creates_conversation

# One integration test target (<crate>/tests/<name>.rs)
cargo test -p rim_infra_db --features db-tests --test user_repo

# Show stdout/logs
cargo test -p rim_core -- --nocapture
```

Common flags: `-- --nocapture`, `-- --test-threads=1`, `-p <crate>`.

## Workspace Tips

- The root manifest is virtual, so plain `cargo test` from the workspace root runs every
  member's tests, the same as `cargo test --workspace`.
- Prefer `-p <crate>` when iterating to avoid rebuilding the entire workspace.
- When debugging clippy/rustfmt failures, run the same commands as CI:
  - `cargo fmt --all -- --check`
  - `cargo clippy --workspace --all-targets -- -D warnings`

## Local Environment / Config

Local dev typically uses `.env` (loaded via `dotenvy` in `rim_server` and `rim_logging`).

- `DATABASE_URL`: Postgres connection string (migrations/tests)
- `RIM_SERVER_HOST`: bind host (see `rim_server/src/config.rs`)
- `RIM_SERVER_PORT`: bind port
- `RUST_LOG`: tracing filter, e.g. `info` or `rim_server=debug,info` (default `info`)

CI uses Postgres 17. For local dev:

```sh
docker run --rm --name rim-postgres -e POSTGRES_USER=postgres -e POSTGRES_PASSWORD=postgres -e POSTGRES_DB=testdb -p 5432:5432 postgres:17-alpine
export DATABASE_URL='postgres://postgres:postgres@localhost:5432/testdb'
cargo run -p migration -- up
```

## Migration CLI (SeaORM)

Run via `sea migrate` (expects `DATABASE_URL`):

```sh
# Generate a new migration
sea migrate generate MIGRATION_NAME

# Apply/rollback
sea migrate up
sea migrate up -n 10
sea migrate down
sea migrate down -n 10

# Reset flows
sea migrate fresh
sea migrate refresh
sea migrate reset

# Inspect
sea migrate status
```

## Entity Generation (SeaORM)

**IMPORTANT:** The `entity/` directory contains hand-written mappers alongside generated entity
code. Regenerating entities directly into `entity/` would overwrite those mappers.

To safely regenerate entities after a migration:

1. Generate entities into a temporary directory:
   ```sh
   sea-orm-cli generate entity -o /tmp/rim-entities
   ```
2. Manually merge the new/changed entity files from `/tmp/rim-entities` into `entity/src/`.
3. Preserve all existing mapper code in `entity/` — do not overwrite files blindly.

Never run `sea-orm-cli generate entity -o entity/src/` directly.

## Crates / Responsibilities

- `rim_server`: Actix Web server wiring + GraphQL endpoint
- `rim_api`: async-graphql schema + HTTP handlers
- `rim_core`: domain models + repository traits + shared macros (IDs)
- `rim_services`: service traits + DTO/command/query types; typed `ServiceError`
- `rim_infra_db`: SeaORM-backed repository implementations
- `entity`: SeaORM entities (generated)
- `migration`: SeaORM migrations + CLI

## Rust Style Guide (Local Conventions)

Formatting:
- Run `cargo fmt --all`; CI enforces `cargo fmt --all -- --check`.
- Do not hand-format around rustfmt.

Imports:
- Order: `std` then external crates then local modules; groups separated by a blank line.
- Prefer explicit imports; allow globs only where idiomatic:
  - generated SeaORM entity files: `use sea_orm::entity::prelude::*;`
  - graphql modules: `use async_graphql::*;`

Naming:
- Modules/files `snake_case`; types/traits `PascalCase`; fns/vars `snake_case`; consts `SCREAMING_SNAKE_CASE`.
- Service-layer conventions: `*Cmd` (write), `*Query` (read), `*Dto` (transport).

Types / modeling:
- Prefer strongly-typed IDs via `model_id!(TypeName);` in `rim_core/src/macros.rs`.
- Standard timestamp type: `chrono::DateTime<chrono::Utc>`.
- Keep `rim_core` free of transport/Actix/GraphQL concerns unless explicitly intended.

Error handling:
- Avoid `unwrap/expect` in library code; OK at process boundaries (`main`, bootstrap) with actionable messages.
- Prefer `?` propagation.
- Use `rim_services::ServiceResult<T>` / `rim_services::ServiceError` at service boundaries.
- Use `anyhow::Result` in infra/repo layers and map to `ServiceError` higher up.

Async:
- Traits use `async_trait` and are `Send + Sync`.
- Avoid holding locks/DB transactions across `.await` unless required.

Generated code:
- `entity/` is generated (see header in `entity/src/lib.rs`); avoid manual edits.

## CI Notes

- CI runs tests single-threaded: `cargo test --all -- --test-threads=1` (helps with shared DB).
- CI also runs `cargo audit` (blocking) to catch vulnerable dependencies; ignored advisories, each with a justification, live in `.cargo/audit.toml`.
- If you want to reproduce locally:
  - `cargo install --locked cargo-audit`
  - `cargo audit`

## Repo-Specific Rules Files

- No Cursor rules found (`.cursor/rules/` or `.cursorrules`).
- No Copilot instructions found (`.github/copilot-instructions.md`).

## Pagination Contract

Cursor pagination semantics are standardized in `docs/pagination.md`.
