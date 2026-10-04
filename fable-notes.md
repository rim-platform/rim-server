# Architecture Review — rim-server

*Reviewed by Claude Fable 5 on 2026-07-08. Companion notes for the frontend are in `../rim-ui/fable-notes.md`.*

## Overall verdict

This is a genuinely well-architected codebase — noticeably above what is typical at this stage of a product. The backend is a disciplined ports-and-adapters design where the discipline is real, not aspirational: `rim_core` is verifiably free of infrastructure concerns, dependency direction is a clean DAG rooted at `rim_core`, the two documented layering exceptions (FL1 composition root, FL2 non-GraphQL transports) are honest and bounded, and conventions like "no unwrap in library code" actually hold when checked (zero in `rim_core`/`entity` production code, near-zero elsewhere outside test modules).

The concerns below are not about the architecture's shape — the shape is right. They are about **scaling friction**: change amplification when adding a service, a few god files, one wiring smell, and one dependency-hygiene bug. Listed in priority order.

## Findings

### 1. `mocks` feature compiled into production binaries — fix first

`rim_api/Cargo.toml:10`, `rim_server/Cargo.toml:11`, and `rim_infra_services/Cargo.toml:28` all declare `rim_services = { features = ["mocks"] }` as a *normal* dependency. That compiles `mockall` and every generated `MockXxxService` into release binaries: bloat, slower builds, and a class of test-only code reachable from prod. Move to `dev-dependencies` (or `cfg(test)`-gated features per consuming crate). ~30-minute fix.

### 2. Command↔macro circular init in `provider.rs` — the one real design smell

`DefaultAppServiceProvider::new` (`rim_infra_services/src/provider.rs:194-269`) builds `command_service_phase1` without macros, builds the macro service, then rebuilds the command service with macros wired in — leaving **two live command-service instances**. Keybind/pipeline hold the macro-less one, and correctness rests on a comment saying they never call `list_commands(Surface::Ai)`. That invariant will eventually be violated silently.

Fix structurally: extract the shared piece (likely command registry/lookup) into its own component, or wire the macro service in post-construction via a setter/`OnceLock` on a single command-service instance.

### 3. Change amplification when adding a service

Adding one service today touches four to five sync points:

- `AppServiceProvider` trait (34 methods) + provider struct (31 `Arc` fields)
- `build_schema` **and** its near-duplicate `export_sdl` in `rim_api/src/lib.rs` (~40 duplicated lines of `.data()` wiring and loader construction)
- The hand-built `AppRepos` set in `rim_server/src/app.rs`, which duplicates `AppRepos::with_handle` (`rim_infra_services/src/repos.rs:392-506`)

Cheap wins: have `export_sdl` call `build_schema` (or extract shared wiring into one function), and make `app.rs` use `AppRepos::with_handle`. The provider trait itself is fine — big but boring.

### 4. God files

- `rim_api/src/types.rs` — 2,793 lines (all GraphQL types + conversions)
- `rim_api/src/mutation.rs` — 2,412 lines, 64 resolvers in one impl block
- `rim_api/src/query.rs` — 1,369 lines, 53 resolvers
- `rim_infra_services/src/auth.rs` — 1,447 lines (`DefaultAuthService`: login + three signup modes + invites + bootstrap in one struct)

async-graphql supports `#[derive(MergedObject)]` — splitting Query/Mutation into per-domain objects (MessageMutations, IncidentMutations, …) is mechanical and pays for itself quickly; same for splitting `types.rs` by domain. `DefaultAuthService` is the service most worth decomposing.

### 5. Hand-maintained lists that will drift

- TRUNCATE table list in `rim_infra_db/tests/support/mod.rs:98-133` — must mirror every entity table. Instead, query `information_schema.tables` at test time and truncate everything.
- `CSRF_EXEMPT_MUTATIONS` string list in `rim_server/src/graphql_support.rs` — consider a resolver-side attribute/directive so the exemption lives next to the mutation it exempts.

### 6. Smaller items worth a pass

- `rim_auth/src/csrf.rs:67-79`: `verify()` computes an HMAC, drops it, then builds a second identical one. Functionally correct (it does compare against the *provided* signature in constant time — verified) but it reads like an interrupted refactor, and security-critical code shouldn't be muddled.
- Subscription authorization (`rim_api/src/subscription.rs::user_can_see_event`) compares IDs via `to_string()` in a codebase with otherwise excellent typed-ID discipline.
- `rim_infra_db` falls back to classifying DB errors by `Display`-string matching (`classify_db_err_by_display`) — brittle across sqlx/driver upgrades.

### 7. Single-node lock-in — fine, because it's honest

Broadcaster (tokio broadcast), `ConfirmationStore`, command rate limiters, presence/typing trackers are all in-memory, all behind traits, all documented as "swap for Redis later." Right call for now. Know that horizontal scaling is a project, not a config change — and the `ConfirmationStore` (dangerous-command proofs) is the piece where a naive multi-node deploy would be a **security** regression, not just a UX one.

## Strengths to preserve

- Guard tests (`db_tests_guard.rs`, `e2e_tests_guard.rs`) that panic when integration-test features are enabled without `DATABASE_URL` — converting silent skips into loud CI failures.
- CSRF enforcement parses the GraphQL AST to find mutation fields rather than trusting client `operationName`, fail-closed on parse failure.
- The `*_mapper.rs` pattern keeping SeaORM types out of the domain layer.
- Viewer-dependent `ReactionLoader` keyed on `(MessageId, UserId)` — a subtle dataloader correctness detail most teams get wrong.
- Layered error-mapping chain (DB → `RepoError` → `ServiceError` → GraphQL extension codes) with internal detail never leaked to clients.
- Zero TODO/FIXME markers; module-visibility discipline (`pub(crate)` by default) enforced in practice.

## Top actions

1. Move `mocks` to dev-dependencies (trivial — do it today).
2. Break the command↔macro construction cycle before more code grows around the two-instance workaround.
3. Deduplicate schema/repo wiring (`export_sdl` vs `build_schema`, `app.rs` vs `AppRepos::with_handle`).

*Note: reconcile with the existing `fable-findings.md` before turning this into a task list — some findings may overlap.*
