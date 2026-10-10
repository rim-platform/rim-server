# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

## Project Overview

RIM is a programmable communication workspace backend — not another chat app. Chat is the communication layer; the product is commands, keybinds, macros, and client-side plugins. See docs/vision.md for full product vision. Rust workspace (edition 2024) with Actix Web + async-graphql, backed by PostgreSQL 17 via SeaORM.

## Build / Run / Test Commands

```sh
# Build
cargo build --workspace

# Run server
cargo run -p rim_server

# Migrations (requires DATABASE_URL)
sea migrate up
sea migrate down
sea migrate status
sea migrate generate MIGRATION_NAME

# Generate new migration file (ALWAYS use this, never create migration files manually):
sea migrate generate <migration_name>
# `sea migrate generate` has repeatedly rewritten migration/src/lib.rs and silently dropped
# adjacent Box::new(...) registrations in the process. After generating, `cargo test -p migration`
# verifies no registration was dropped (registration_complete.rs, no DATABASE_URL needed) — it
# fails loudly, naming the offender, if one goes missing.

# Entity generation (NEVER regenerate directly into entity/src/ — it contains hand-written mappers)
# Generate to temp dir first, then manually merge new/changed entities into entity/src/:
sea generate entity -l -o /tmp/rim-entities

# Format + lint (CI enforces these)
cargo fmt --all
cargo clippy --workspace --all-targets -- -D warnings

# Tests
cargo test --workspace                    # all unit tests
cargo test --all -- --test-threads=1      # single-threaded (CI style, needed for shared DB)
cargo test -p rim_services send_message   # single test by substring
cargo test -p rim_services conversation::tests::creates_conversation  # specific test path
cargo test -p rim_core -- --nocapture     # with stdout

# Feature-gated integration tests (require running Postgres)
# NOTE: These tests PANIC if DATABASE_URL is unset; this is intentional.
# To skip them entirely, simply omit the --features flag. Do NOT set the feature
# flag without a valid DATABASE_URL or CI will fail loudly (by design).
cargo test -p rim_infra_db --features db-tests -- --test-threads=1
cargo test -p rim_api --features e2e-tests -- --test-threads=1
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

`rim_server` is a library with a thin binary; `rim_api`'s e2e tests depend on it (a dev-dependency
cycle cargo allows) to run two servers in one process (`rim_api/tests/s2s_e2e.rs`, second
database `<DATABASE_URL name>b`, created when missing).

Use `-p <crate>` when iterating to avoid rebuilding the entire workspace. The root `Cargo.toml` is a virtual manifest (no root package), so plain `cargo test` at the root runs every member, same as `--workspace`.

## Architecture

Layered architecture with strict dependency direction (top depends on bottom):

```
rim_server  (HTTP/Actix bootstrap, CORS, CSRF, session cookies)
    ↓                                          ↘  (composition root + non-GraphQL transports — see notes below)
rim_api  (GraphQL schema, Query/Mutation resolvers, auth context injection)
    ↓
rim_infra_services  (business logic implementations, validation, orchestration)
    ↓
rim_infra_db  (SeaORM repository implementations) + entity (generated SeaORM entities)
    ↓
PostgreSQL
```

Cross-cutting contracts that define boundaries between layers:
- **rim_core** — domain models, repository traits, strongly-typed IDs via `model_id!()` macro
- **rim_services** — service traits, DTOs (`*Dto`), commands (`*Cmd`), queries (`*Query`), `ServiceResult<T>`/`ServiceError`

**Two documented exceptions to the strict chain** (both intentional, both bounded):

1. **Composition root (FL1).** `rim_server` legitimately depends on `rim_infra_services` to instantiate concrete services at boot — see `rim_server/src/app.rs` (composition root: `DefaultAppServiceProvider::new` at ~line 179, plus `PresenceManager`/`TypingTracker`/`DefaultStorageService` wiring nearby). The wiring layer must know the concrete impls; everything downstream sees only traits. If this ever leaks beyond `app.rs`, refactor into a dedicated `rim_app` crate that holds wiring and is the only thing both `rim_server` and `rim_infra_services` import.

2. **Non-GraphQL transports (FL2).** `rim_server` reaches into `rim_services` directly (skipping `rim_api`) for transports that are not GraphQL: `file_handlers.rs` (multipart upload) and `graphql_support.rs` consume `rim_services::auth::service::RequestAuthenticator`, `rim_services::storage::service::StorageService`, `rim_services::message_attachment::service::MessageAttachmentService`, and `rim_services::moderation::service::ModerationService` (the report-scoped attachment download). This is architecturally defensible — multipart upload bypasses the GraphQL pipeline by design — but it shortens the chain. The server-to-server routes (`s2s/`) and the relay's forwarding route (`remote_handlers.rs`) do the same with `rim_services::relay::service::RelayService` and `rim_services::host::service::HostService`. Any new non-GraphQL transport (WebSocket file streaming, Webhooks, etc.) follows the same pattern: depend on `rim_services` traits, not `rim_infra_services` impls.

### Key patterns

- **Provider pattern**: `AppServiceProvider` trait (rim_services) → `DefaultAppServiceProvider` (rim_infra_services) wires all service implementations
- **Strongly-typed IDs**: all entities use `model_id!(TypeName)` macro in `rim_core/src/macros.rs` generating UUID v7-based wrappers
- **Cursor-based pagination**: standardized `(timestamp, id)` tuples; contract in `docs/pagination.md`
- **Error mapping chain**: DB errors → `RepoError` → `ServiceError` → GraphQL errors
- **Auth flow**: session cookie → `RequestAuthenticator` → `Actor` (principal + user_id) injected into GraphQL context
- **CSRF protection**: enforced on mutations via `X-CSRF-Token` header

### Crate responsibilities

| Crate | Role |
|---|---|
| `rim_server` | Actix Web server wiring, config loading, request handler |
| `rim_api` | async-graphql schema, types, pagination, auth context |
| `rim_core` | Domain models, repository traits, ID macros |
| `rim_services` | Service traits, DTOs/Cmd/Query types, `ServiceError` |
| `rim_infra_services` | Default service implementations + unit tests (mockall) |
| `rim_infra_db` | SeaORM-backed repository implementations |
| `entity` | Generated SeaORM entities — do not edit manually |
| `migration` | SeaORM migrations + CLI |
| `rim_auth` | Cryptographic primitives (Argon2, SHA2, base64) |
| `rim_identity` | Key identity statements (Ed25519 key log, device certificates, profile cards, login proofs); verification only, no I/O. Contract: `docs/specs/key-identity-v1.md` |
| `rim_s2s` | Server identity and the S2S transport without storage or Actix: server key log and `rims1_` ids, the signed descriptor, server-signed statements, the RFC 9421 signature profile. Contract: `docs/specs/server-to-server-v1.md`; operator view: `docs/server-identity.md` |
| `rim_logging` | Structured tracing initialization (JSON output) |

## Conventions

- **Imports**: `std` → external crates → local modules, separated by blank lines. Globs only for SeaORM entities and async-graphql.
- **Naming**: `*Cmd` (write commands), `*Query` (read queries), `*Dto` (transport objects)
- **Timestamps**: `chrono::DateTime<chrono::Utc>`
- **Error handling**: `?` propagation, `ServiceResult<T>`/`ServiceError` at service boundaries, `anyhow::Result` in infra layers. No `unwrap`/`expect` in library code.
- **Async traits**: use `async_trait`, must be `Send + Sync`
- **Comments**: say what the item is and what it does, briefly. No history, no "implements
  section N of the spec", no references to docs, plans, stages or PR numbers.
- **Commit messages**: a plain imperative first line (≤ 72 chars) and a body that says why. No
  trailers of any kind (no `Co-Authored-By`).
- **rim_core purity**: keep free of transport/Actix/GraphQL concerns
- **Module visibility default**: new modules declare as `pub(crate) mod foo;` by default. Promotion to `pub mod foo;` requires a documented external consumer (named in a comment or PR description). This keeps the public surface area of each crate minimal — Rust's compiler-enforced privacy is one of the cheapest tools we have for preventing layer leaks like F7.
- **Mockable async traits**: put `#[async_trait]` *above* `#[cfg_attr(any(test, feature = "mocks"), automock)]`, consistently — this is what the large majority of mockable traits in `rim_core`/`rim_services` already do. Mixed order is a compile footgun: `automock` generates a different mock shape depending on whether it sees the trait before or after `async_trait` desugars it. With `async_trait` above (the chosen order), the generated `Mock*` type's `.returning(...)` closures must return a boxed future (`Box::pin(async move { ... })`); a handful of pre-existing traits (e.g. `UserRepository`, `ServerMemberRepository`, `PasswordResetTokenRepository`) have the reverse order and their mocks instead expect `.returning(...)` closures to return a plain `Result<T, E>` directly. Check the actual generated `Mock*` type (or an existing test using it) before assuming which shape applies — don't guess from the trait name alone.

## Local Dev Environment

Requires `.env` file with: `DATABASE_URL`, `RIM_SERVER_HOST`, `RIM_SERVER_PORT`; the log filter comes from `RUST_LOG` (default `info`). Config loaded via `dotenvy` in `rim_server/src/config.rs`.

## GraphQL Schema Export (CRITICAL for frontend codegen)

The server exposes `GET /api/sdl` which returns the full GraphQL schema in SDL format.
The frontend uses this to generate TypeScript types via `@graphql-codegen`.

**When you add or change GraphQL types, enums, inputs, or resolvers:**

1. The running server automatically reflects changes at `/api/sdl`
2. The frontend team must re-export and regenerate:
   ```bash
   curl -s http://localhost:8080/api/sdl > ../rim-ui/schema.graphql
   cd ../rim-ui && npm run codegen
   ```

**Key rules for GraphQL types:**
- `#[derive(Enum)]` on Rust enums auto-converts variants to SCREAMING_SNAKE in GraphQL
  (e.g., Rust `Sev2` → GraphQL `SEV_2`, Rust `ActionItem` → GraphQL `ACTION_ITEM`)
- `#[derive(SimpleObject)]` converts Rust `snake_case` fields to GraphQL `camelCase`
- `build_schema()` in `rim_api/src/lib.rs` builds the schema; `/api/sdl` serves its `sdl()`
- GraphQL types live in `rim_api/src/types/<domain>.rs` (all re-exported from `rim_api/src/types/mod.rs`,
  so `crate::types::X` works everywhere) — check there for field names. Resolvers live in
  `rim_api/src/query/<domain>.rs` and `rim_api/src/mutation/<domain>.rs`, one `#[Object]` per domain,
  merged into `Query`/`Mutation` with `#[derive(MergedObject)]` in each `mod.rs`
- **The member order of `Query`/`Mutation` is the field order in the SDL.** Append a new domain object to
  the last `QueryGroupN`/`MutationGroupN` (or add resolvers to an existing object); never reorder members, or
  the frontend codegen diff churns. Groups hold at most five objects: a flat `MergedObject` of ~30 members
  overflows rustc's default recursion limit (see the module docs in `query/mod.rs`)
- The SDL endpoint in `rim_server/src/server.rs` serves it at `/api/sdl`
- **Always verify enum value names** in the SDL match what the frontend expects

## Code Review

Every code change is reviewed by the session that coordinates the work, not by a separate
review agent: it reads the whole diff, fixes what it finds, checks the implementer's gate
output (fmt, clippy, tests, the feature-gated suites), and only then opens and merges the PR.
Subagents implement and run the gates; they never spawn reviewers.

Review covers: layered architecture compliance, pattern consistency, cross-repo GraphQL schema match, error handling, import ordering, copy-paste detection.

## CI

GitHub Actions runs: build+test (single-threaded), fmt check, clippy (-D warnings), coverage (75% line gate via cargo-llvm-cov — temporarily lowered from 95%; raise it as coverage improves), cargo audit, Docker build validation.
