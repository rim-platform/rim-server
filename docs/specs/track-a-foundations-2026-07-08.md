# Track A Foundations — Implementation Spec (2026-07-08)

**Author:** iron-man (Backend Architect) · **Implementer:** hulk · **Requested by:** nick-fury
**Source:** `ROADMAP.md` Track A items 2 & 5, the wiring dedup from the Fable review
(`fable-notes.md` findings 1–3) and `fable-findings.md` A-M8, plus a HIGH schema bug found
during Phase 5 ledger verification (item 4, added 2026-07-08 at nick-fury's request).

## Scope & constraints (read first)

Four independent items. They do **not** depend on each other — implement and test each
on its own so a regression in one does not block the others.

Hard constraints for items 1–3:

- **No schema changes.** No migrations, no entity edits. (Item 4 is the sanctioned exception —
  it is a schema fix and is scoped to exactly one column + index; see that item.)
- **No behavior change visible to the GraphQL API.** The SDL output and all resolver
  behavior must be byte-for-byte identical before/after.
- `cargo fmt --all`, `cargo clippy --workspace --all-targets -- -D warnings`, and
  `cargo test --workspace` must all pass.
- Every new function gets **tests and doc comments** (`///`). This is a CLAUDE.md rule.
- No `unwrap`/`expect` in library code. Typed errors only.
- Module visibility: new items default to `pub(crate)`; only go `pub` with a named consumer.
- **No git commits/pushes.** Leave the tree dirty for review.

Verification order per item: `cargo fmt` → `cargo clippy ... -D warnings` →
`cargo build --workspace` → `cargo test --workspace`.

---

## Item 1 — Move `mocks` out of production dependencies

### Problem
`rim_services` has an optional `mocks` feature (`mocks = ["dep:mockall"]`,
`rim_services/Cargo.toml:29`) that generates `MockXxxService` types via `mockall`.
Three crates declare `rim_services = { path = ..., features = ["mocks"] }` as a **normal**
dependency, so `cargo build --workspace` compiles `mockall` and every mock into release
binaries:

- `rim_api/Cargo.toml:10`
- `rim_server/Cargo.toml:11`
- `rim_infra_services/Cargo.toml:28`

### Findings that make this safe
- The workspace uses **resolver 3** (`Cargo.toml:9`), so dev-dependency features are **not**
  unified into normal `cargo build` — this is exactly the pattern that makes the fix clean.
- `Mock*Service` usage is confined to test code:
  - `rim_infra_services`: only in `#[cfg(test)]` modules (executor tests, `command_tests.rs`,
    per-service test modules).
  - `rim_api`: only in `rim_api/src/tests.rs` (`#[cfg(test)]`). `rim_api/tests/` does **not**
    use mock service types.
  - `rim_server`: **no** `Mock*Service` usage anywhere (no `src` usage, no `tests/` dir).
    The feature there is entirely spurious.

### Required changes

**`rim_server/Cargo.toml:11`** — drop the feature entirely (no dev-dep needed):
```toml
rim_services = { path = "../rim_services" }
```

**`rim_api/Cargo.toml`** — normal dep loses the feature; add a dev-dep that carries it.
Line 10 becomes:
```toml
rim_services = { path = "../rim_services" }
```
Add under `[dev-dependencies]` (with the existing dev-deps):
```toml
rim_services = { path = "../rim_services", features = ["mocks"] }
```

**`rim_infra_services/Cargo.toml`** — same split. Line 28 (in `[dependencies]`) becomes:
```toml
rim_services = { path = "../rim_services" }
```
Add under `[dev-dependencies]`:
```toml
rim_services = { path = "../rim_services", features = ["mocks"] }
```
(`rim_infra_services` dev-deps already carry `mockall` and `rim_core` with `mocks`; this
just adds the `rim_services` mock surface that its `#[cfg(test)]` modules consume.)

### Acceptance
- `cargo build --workspace` compiles with **no** `mockall` in the dependency graph. Verify with
  `cargo tree -p rim_server -i mockall` (and for `rim_api`, `rim_infra_services`) → must report
  "package not found" / not in graph for a non-test build. Equivalent check:
  `cargo build --workspace 2>&1` then `cargo tree --workspace -i mockall` should only show it
  reachable via dev/test edges.
- `cargo test --workspace` still compiles and passes (mocks available to tests).
- No source-file changes required for this item. If `cargo test -p rim_server` surprises us by
  referencing a mock (it should not), add the same dev-dependency to `rim_server` — but do not
  add it preemptively.

### Docs
No doc-comment changes. This item is Cargo-only.

---

## Item 2 — Break the command↔macro init cycle (single command-service instance)

### Problem
`DefaultAppServiceProvider::new` (`rim_infra_services/src/provider.rs:194–269`) uses two-phase
construction: it builds `command_service_phase1` (no macro), builds `macro_service` from it,
then rebuilds a second `command_service` with the macro wired in. Result: **two live
`DefaultCommandService` instances**. `keybind_service` and `pipeline_service` hold the
macro-less phase-1 instance; correctness rests on a comment-only invariant that those paths
never call `list_commands(Surface::Ai)`.

### Why the cycle exists (confirmed by reading the code)
- `DefaultCommandService::list_commands` calls `macro_service.synthesize_command_definitions`
  **only** on the `Surface::Ai` branch (`command/mod.rs:183–198`). That is the sole
  command→macro dependency.
- `DefaultMacroService` calls `command_service.get_command` (sensitivity lookup) and
  `command_service.execute_command` (per step) — the macro→command dependency
  (`macro_def.rs:325–463`). Macro steps may not reference other macros (validated at
  `macro_def.rs:130`), so the macro's command-service reference does **not** itself need macro
  discovery. This is why a macro-less command service "worked" for the macro service.
- Actual current consumers of the phase-1 instance:
  - `keybind`: only `command_service.get_command` (`keybind.rs:445`).
  - `pipeline`: only `command_service.execute_command` (`pipeline.rs:533`).
  Neither calls `list_commands(Surface::Ai)` today — hence the fragile invariant.

### Design: OnceLock late-binding on one instance

Replace the eager `Option<Arc<dyn MacroService>>` seed with a shared, set-once cell on a
single `DefaultCommandService`. Build the command service **once**, hand the same `Arc` to
macro/keybind/pipeline, then set the macro service into the cell after `macro_service` exists.
Because all holders share one `Arc<DefaultCommandService>`, the late `set` is visible to every
holder — including keybind and pipeline — which structurally removes the invariant.

#### 2a. `rim_infra_services/src/command/mod.rs`

Remove the `macro_service` field from `CommandServices` (lines 62–69, the field + its doc
comment). It is being replaced by the cell.

Add a field to `DefaultCommandService` (after `confirmation_store`):
```rust
    /// AI-surface macro discovery source, wired **after** construction to break
    /// the command↔macro circular init dependency (provider builds the command
    /// service first, then the macro service from it, then calls
    /// [`Self::set_macro_service`]). Wrapped in `Arc<OnceLock<_>>` so that the
    /// single write is visible through every `Arc<dyn CommandService>` clone the
    /// provider hands to keybind/pipeline/macro. `None` (unset) in test contexts
    /// and until the provider wires it. When set, `list_commands(Surface::Ai)`
    /// delegates macro synthesis to `MacroService::synthesize_command_definitions`.
    pub(super) macro_service:
        std::sync::Arc<std::sync::OnceLock<std::sync::Arc<dyn MacroService>>>,
```

`DefaultCommandService` still derives `Clone`; `Arc<OnceLock<...>>` is `Clone` and shares the
cell, so cloning the struct (or the trait-object `Arc`) keeps one shared cell. That is the
property that guarantees single-instance semantics.

Initialize the field as `std::sync::Arc::new(std::sync::OnceLock::new())` in all three
constructors: `with_confirmation_store` (the base one; `new` delegates to it) and
`with_extra_builtins`.

Add an inherent method:
```rust
    /// Wire the macro service into this command service exactly once, after the
    /// macro service has been constructed. Idempotent-safe: a second call is a
    /// no-op and is logged, never a panic. Returns `Ok(())` on the first set,
    /// `Err(())` if already set — callers may ignore the result in production
    /// wiring where set-once is guaranteed by construction order.
    pub fn set_macro_service(
        &self,
        macro_service: std::sync::Arc<dyn MacroService>,
    ) -> Result<(), ()> {
        self.macro_service.set(macro_service).map_err(|_| {
            tracing::warn!(
                event = "command_service_macro_double_set",
                "set_macro_service called after macro service already wired; ignoring"
            );
        })
    }
```

Update `list_commands` (`command/mod.rs:186–189`). Replace:
```rust
            if let (Some(macro_svc), Some(server_id)) = (
                self.services.macro_service.as_deref(),
                context.and_then(|c| c.workspace_id),
            ) {
```
with:
```rust
            if let (Some(macro_svc), Some(server_id)) = (
                self.macro_service.get().map(std::sync::Arc::as_ref),
                context.and_then(|c| c.workspace_id),
            ) {
```
(`OnceLock::get` returns `Option<&Arc<dyn MacroService>>`; `Arc::as_ref` gives
`&dyn MacroService`, matching the existing `synthesize_command_definitions` call.)

#### 2b. `rim_infra_services/src/provider.rs:194–269`

Collapse two-phase construction into one instance. Concretely:

1. Delete the `command_service_phase1` binding and the phase-2 rebuild (lines 218–224 and
   253–269), and delete the two large explanatory comments (194–203, 253–258).
2. Keep the `command_services_args` closure but remove the `macro_service: None` line from it
   (that field no longer exists).
3. Build the command service **once** as a concrete `Arc` so the inherent setter is callable:
```rust
    let command_service_concrete = std::sync::Arc::new(
        crate::command::DefaultCommandService::with_confirmation_store(
            repos.for_command(),
            command_services_args(),
            Arc::clone(&confirmation_store),
        ),
    );
    let command_service: Arc<dyn CommandService> = Arc::clone(&command_service_concrete)
        as Arc<dyn CommandService>;
```
4. `keybind_service` and `pipeline_service` take `Arc::clone(&command_service)` (same
   instance). `macro_service` takes `Arc::clone(&command_service)` in its
   `MacroServices.command_service`.
5. After `macro_service` is constructed, wire it in:
```rust
    // Break the command↔macro init cycle: the command service is constructed
    // first (macro cell empty), then the macro service is built from it, then
    // the macro service is set back into the single command-service instance.
    // keybind/pipeline hold the same Arc, so they observe macros too — no
    // comment-only invariant.
    let _ = command_service_concrete.set_macro_service(Arc::clone(&macro_service));
```
   Place this immediately after the `macro_service` binding and before `channel_lens_service`.
   Order requirement: `keybind_service` may be constructed before or after the `set` call —
   it holds the shared `Arc`, so the set is visible regardless. Keep the existing construction
   order otherwise to minimize diff.
6. The provider struct field `command_service` stores the `Arc<dyn CommandService>` as today.

The `confirmation_store` shared-instance concern (`provider.rs:187–192`) is naturally
satisfied — there is now one command service, so there is nothing to keep in sync.

#### 2c. Test call-site updates (mechanical, compiler-guarded)

`CommandServices` no longer has a `macro_service` field. Delete the `macro_service: None,`
line from each of these `CommandServices { ... }` literals:

- `rim_infra_services/src/command/executor/pin.rs:103`
- `rim_infra_services/src/command/executor/reaction.rs:132`
- `rim_infra_services/src/command/executor/state.rs:294`
- `rim_infra_services/src/command/executor/admin.rs:112`
- `rim_infra_services/src/command/executor/chat.rs` — lines 376, 666, 711, 754, 819, 858, 900,
  953, 1076
- `rim_infra_services/src/command/executor/conversation.rs:100`
- `rim_infra_services/src/command/executor/chat_messages.rs:244`
- `rim_infra_services/src/command/executor/member.rs:108`
- `rim_infra_services/src/command_tests.rs:101` and `:1003` and `:1505` (the `None` sites)

The one **injection** site, `command_tests.rs` `service_with_macro` (currently
`macro_service: macro_svc` at line 1143): remove that field from the literal, and after
constructing the service, wire the macro via the setter. Change the function to:
```rust
    fn service_with_macro(
        cap: rim_services::capability::service::MockCapabilityService,
        macro_svc: Option<Arc<dyn rim_services::macro_def::service::MacroService>>,
    ) -> DefaultCommandService {
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));

        let svc = DefaultCommandService::new(
            CommandRepos { /* unchanged */ },
            CommandServices { /* unchanged, minus macro_service */ },
        );
        if let Some(macro_svc) = macro_svc {
            let _ = svc.set_macro_service(macro_svc);
        }
        svc
    }
```
(`set_macro_service` takes `&self`; calling it on the owned `svc` before returning is fine.)

The compiler will flag any missed site — treat a clean `cargo test --workspace` compile as the
completeness check.

### Tests to add (hulk writes these)

Add to `rim_infra_services/src/command_tests.rs` (or the existing command test module):

1. **`set_macro_service_wires_ai_discovery`** — construct a `DefaultCommandService` with a mock
   `MacroService` whose `synthesize_command_definitions` returns one synthetic
   `macro.<id>` `CommandDefinitionDto` (ai_callable=true). Call `set_macro_service`. Call
   `list_commands(Surface::Ai)` with a context carrying a `workspace_id`. Assert the synthesized
   macro command appears in the result. (This is the "single source of truth" delegation path.)

2. **`macro_visible_through_shared_arc_clone`** — the **regression test for the invariant**.
   Wrap the command service in `Arc<DefaultCommandService>`, then `Arc::clone` it into a second
   binding that stands in for what keybind/pipeline hold. Call `set_macro_service` on the
   **original** Arc. Then call `list_commands(Surface::Ai)` on the **clone** and assert the
   synthesized macro command is present. This proves the late set is observed through the shared
   instance — the exact property the old two-instance design lacked. Add a comment naming the
   fixed defect so a future refactor that reintroduces a second instance fails here.

3. **`set_macro_service_second_call_is_noop`** — call `set_macro_service` twice; assert the
   first returns `Ok(())` and the second returns `Err(())` and does not change the wired service
   (assert `list_commands(Surface::Ai)` still reflects the first macro service).

Use the existing `full_caps()` capability mock and the existing `MockMacroService` pattern from
`macro_def.rs` tests as references. The mock's `synthesize_command_definitions` must return a
`Vec<CommandDefinitionDto>` with `ai_callable: true` so it survives the AI-surface filter.

### Docs
- Doc comment on `set_macro_service` (above).
- Doc comment on the new `macro_service` cell field (above).
- Update the `list_commands` inline comment if it references the old `self.services.macro_service`
  seed; keep the "single source of truth" note.
- Update `provider.rs`: replace the two-phase construction comment block with the short
  cycle-breaking note in 2b step 5.

### Rollback note
Pure in-process wiring change; no persistent state, no schema. Reverting is a straight code
revert. No data migration implications.

---

## Item 3 — Deduplicate wiring

### 3a. `build_schema` / `export_sdl` shared wiring (`rim_api/src/lib.rs`)

**Problem.** `build_schema` (lines 47–131) and `export_sdl` (136–220) duplicate ~40 lines of
identical loader construction and `.data(...)` wiring. They differ in exactly two ways:
- `build_schema` ends with `.limit_depth(10).limit_complexity(200)` before `.finish()`;
  `export_sdl` has no limits (so introspection succeeds).
- Registration data: `build_schema` injects the caller's `RegistrationContext` (3 `Arc`s);
  `export_sdl` injects hardcoded defaults (`RegistrationMode::InviteOnly`,
  `PreviewInviteRateLimiterToken`, `SignupRateLimiterToken`).

**Finding worth flagging (do NOT act on it in this item):** `export_sdl` has **no caller**. The
`/api/sdl` endpoint handler (`rim_server/src/server.rs:125`) serves `schema.sdl()` from the
already-built `AppSchema`, not `export_sdl`. `export_sdl` is effectively dead code. I am
**keeping** it in this spec (extract-and-reuse, not delete) to stay within the "no behavior
change" constraint and avoid removing a `pub` fn without sign-off. See "Follow-up for nick-fury"
at the end.

**Required change.** Extract a private helper in `rim_api/src/lib.rs` that does the common
loader + `.data()` wiring and returns the `SchemaBuilder`; have both functions call it, then add
their divergent tail.

```rust
/// Wire the loaders and all shared `.data(...)` context onto a fresh schema
/// builder. Both [`build_schema`] and [`export_sdl`] call this; they differ only
/// in registration-context data and depth/complexity limits, which each applies
/// after this returns. Kept private — the two public entry points are the API.
fn wire_common_schema_data(
    builder: async_graphql::SchemaBuilder<Query, Mutation, Subscription>,
    service_provider: Arc<dyn AppServiceProvider>,
    broadcaster: InMemoryBroadcaster,
    presence_manager: Arc<dyn rim_services::presence::PresenceService>,
    typing_tracker: Arc<dyn rim_services::typing::TypingService>,
    observers: Arc<rim_services::events::Observers>,
) -> async_graphql::SchemaBuilder<Query, Mutation, Subscription> {
    use crate::loaders::{LinkPreviewLoader, MentionLoader, PinLoader, ReactionLoader, UserLoader};
    use async_graphql::dataloader::DataLoader;

    let user_loader = DataLoader::new(
        UserLoader::new(service_provider.user_service()),
        tokio::spawn,
    );
    let reaction_loader = DataLoader::new(
        ReactionLoader::new(service_provider.message_reaction_service()),
        tokio::spawn,
    );
    let mention_loader = DataLoader::new(
        MentionLoader::new(service_provider.mention_service()),
        tokio::spawn,
    );
    let pin_loader = DataLoader::new(PinLoader::new(service_provider.pin_service()), tokio::spawn);
    let link_preview_loader = DataLoader::new(
        LinkPreviewLoader::new(service_provider.link_preview_service()),
        tokio::spawn,
    );

    builder
        .data(std::sync::Arc::new(crate::auth_intent::AuthIntentCtx::default()))
        .data(crate::auth_context::AuthActorCtx::default())
        .data(broadcaster)
        .data(Arc::clone(&service_provider))
        .data(service_provider.action_service())
        // ... every existing `.data(service_provider.*())` call, in the same order ...
        .data(service_provider.pipeline_service())
        .data(presence_manager)
        .data(typing_tracker)
        .data(observers)
        .data(user_loader)
        .data(reaction_loader)
        .data(mention_loader)
        .data(pin_loader)
        .data(link_preview_loader)
}
```

Then:
```rust
pub fn build_schema(/* unchanged signature */) -> AppSchema {
    wire_common_schema_data(
        Schema::build(Query, Mutation, Subscription),
        service_provider,
        broadcaster,
        presence_manager,
        typing_tracker,
        observers,
    )
    .data(registration.registration_mode)
    .data(registration.preview_rate_limiter)
    .data(registration.signup_rate_limiter)
    .limit_depth(10)
    .limit_complexity(200)
    .finish()
}

pub fn export_sdl(/* unchanged signature */) -> String {
    let schema = wire_common_schema_data(
        Schema::build(Query, Mutation, Subscription),
        service_provider,
        broadcaster,
        presence_manager,
        typing_tracker,
        observers,
    )
    .data(Arc::new(rim_services::auth::dto::RegistrationMode::InviteOnly))
    .data(Arc::new(rim_services::auth::dto::PreviewInviteRateLimiterToken))
    .data(Arc::new(rim_services::auth::dto::SignupRateLimiterToken))
    .finish();
    schema.sdl()
}
```

**Critical correctness requirement:** the `.data(...)` call **set and order** inside
`wire_common_schema_data` must exactly reproduce the current `build_schema` body (lines 79–123)
minus the registration/limits tail. Reproduce the full list of 30 `service_provider.*()` data
calls verbatim (do not rely on the elision in the snippet above). The public function signatures
must not change (they have external callers: `rim_server/src/app.rs:287`, `rim_api/src/tests.rs`,
`rim_api/tests/support/mod.rs`).

**Tests.** No new public behavior, but add/confirm a guard test in `rim_api/src/tests.rs`:
- **`sdl_export_is_nonempty_and_has_core_types`** — call `export_sdl(...)` with the test
  service provider and assert the returned SDL string contains a few known type markers (e.g.
  `type Query`, `type Mutation`). This gives `export_sdl` its first test and locks the extraction.
- The existing `build_schema`-based e2e/unit tests already cover the `build_schema` path;
  confirm they still pass unchanged (they exercise depth/complexity limits and registration).

**Docs.** Doc comment on `wire_common_schema_data` (above). Keep the existing `export_sdl` doc
comment. Note in the `export_sdl` doc that it shares wiring with `build_schema` via the helper.

### 3b. `app.rs` uses the canonical `AppRepos::with_handle`

**Problem.** `rim_server/src/app.rs:48–143` hand-builds all 31 repos, duplicating
`AppRepos::with_handle` (`rim_infra_services/src/repos.rs:392–506`). Adding a repo means editing
both; drift is silent (`fable-findings.md` A-M8).

**Findings that make this safe:**
- `with_handle(&DbHandle)` constructs the identical 31-repo set from one handle, and is already
  the production UoW path (used in `message.rs`, `message_reaction.rs`, `server.rs`, `auth.rs`,
  `incident.rs`, and `rim_infra_services/tests/signup_txn_regression.rs`). Well-exercised.
- In `app.rs`, the standalone `Arc<dyn …>` repo bindings created before the struct literal
  (`user_repo`, `server_repo`, `server_member_repo`, `session_repo`, `invite_repo`,
  `bootstrap_token_repo`, `instance_invite_repo`, lines 48–61) are **only** used to populate the
  `AppRepos` literal — they are not referenced after line 143. Confirmed by reading the whole
  function. So they can be deleted along with the literal.
- `app.rs` builds repos from `db` (the `DbHandle` from `db.into()` at line 46). `with_handle`
  takes `&DbHandle`. Same underlying connection; `raw_db` (the raw `DatabaseConnection` for UoW)
  is unaffected and still passed to `DefaultAppServiceProvider::new`.

**Required change.** Replace lines 48–143 (the seven standalone repo bindings **and** the entire
`let repos = AppRepos { ... };` literal) with:
```rust
    let repos = AppRepos::with_handle(&db);
```
Keep everything else: `raw_db`, the `seed_builtin_capabilities(repos.capability_registry_repo...)`
call, `DefaultAppServiceProvider::new(&repos, raw_db, ...)`, etc. all continue to work because
`repos` has the same shape.

**Import cleanup.** After the change, the explicit `sea_orm_impl::{...}` imports in `app.rs:19–25`
and the `rim_infra_db::sea_orm_impl` import (line 18) become unused. Remove the now-unused imports
(clippy `-D warnings` will flag them). Keep `DbHandle` (still used at line 46) and
`rim_infra_services::{AppRepos, DefaultAppServiceProvider}` (line 26). Run
`cargo clippy -p rim_server --all-targets -- -D warnings` to catch every unused import precisely.

**Tests.** This is a composition-root wiring change with no unit-test seam (building the runtime
needs a live DB). Coverage comes from:
- `cargo build --workspace` + clippy (proves it compiles and imports are clean).
- The existing `rim_api` e2e tests and `rim_infra_services` integration tests that boot repos via
  `with_handle` already exercise the canonical constructor.
- If a DB is available, `cargo test -p rim_api --features e2e-tests -- --test-threads=1` is the
  end-to-end proof the wired app still serves requests. Note in the handoff whether a DB was
  available; if not, state that this path was verified by compile + clippy only.

**Docs.** No new function. Optionally add a one-line comment at the `with_handle` call site noting
it is the canonical constructor (mirrors the FL1 composition-root note in CLAUDE.md). Do not
duplicate the doc that already lives on `with_handle`.

**Rollback note.** Pure wiring; revert is a code revert. No schema/data impact.

---

## Item 4 — Add the missing `parent_audit_id` column (HIGH schema bug)

### Problem
`command_audit_log.parent_audit_id` is declared in the entity
(`entity/src/command_audit_log.rs:30`, `Option<Uuid>`), **written** by the repository
(`rim_infra_db/src/sea_orm_impl/audit.rs:45`), and **set** by macro execution
(`rim_infra_services/src/macro_def.rs:408`, where children carry `parent_audit_id = Some(parent)`
and the parent row is written after the loop). But **no migration ever creates the column** —
`rg parent_audit_id migration/src/` returns zero hits. The three migrations that touch the table
(`m20260425_000001_create_command_audit_log`, `m20260425_000002_audit_log_add_check_outcome_columns`,
`m20260427_000001_audit_outcome_widen_check`) never add it.

**Impact:** every audit `INSERT` against a real Postgres fails with `column "parent_audit_id"
does not exist`. Because SeaORM's `ActiveModel` insert lists every column, this breaks the
**entire** audit write path — the top-level `execute_command` audit row and, most visibly, the
macro parent/child rows. It escaped the ~350 backend tests because command/macro unit tests wire
`audit_log_service: None` (mocked away) and no DB integration test exercises an audit insert. That
missing test is part of this item.

**Related, already-correct:** the `outcome` CHECK constraint already lists all 7
`AuditOutcome::as_str()` values (fixed in `m20260427_000001`), so once the column exists the macro
outcome values (`step_error`, `macro_partial_failure`, `rate_limited`) insert cleanly. No CHECK
work needed here. No entity change needed either — the entity already declares the field
correctly; only the migration is missing.

### Design decision: plain nullable column + partial index, **no foreign key**

`parent_audit_id` conceptually references `command_audit_log.id` (self-reference). Do **not** add a
self-referencing FK. Two concrete reasons from the code:

1. **Write order.** In `macro_def.rs`, child rows are inserted inside the step loop (lines
   478–517) **before** the parent row is written after the loop (lines 543–569). These are
   separate inserts (separate awaits, not one transaction), so a self-FK — even
   `DEFERRABLE INITIALLY DEFERRED` — would reject the child inserts because the parent row does
   not yet exist.
2. **Retention cleanup.** `AuditLogRepository::delete_older_than` (`audit.rs:143`) deletes rows by
   age. A self-FK with `ON DELETE RESTRICT` would block deleting a parent while children survive;
   `CASCADE`/`SET NULL` add surprising delete semantics to an append-only forensic log.

An audit log is an append-only forensic record; referential enforcement via FK is the wrong tool.
Use a **plain nullable `uuid` column** plus a **partial index** (`WHERE parent_audit_id IS NOT
NULL`) — most rows are top-level with a NULL parent, so the partial index stays small while
supporting the child→parent join that `macro_def.rs` describes ("consumers joining on
parent_audit_id").

### Migration (use `sea migrate generate`, never hand-create the file)

Per CLAUDE.md, generate the skeleton:
```sh
sea migrate generate add_command_audit_log_parent_audit_id
```
This produces `migration/src/m20260708_HHMMSS_add_command_audit_log_parent_audit_id.rs` (today's
UTC timestamp; sorts after the current latest `m20260428_194718`). Then register it in
`migration/src/lib.rs`: add the `mod ...;` line (end of the mod list) and the
`Box::new(m20260708_..._add_command_audit_log_parent_audit_id::Migration)` line at the **end** of
the `migrations()` vec — timestamp order. (If your `sea-orm-cli` auto-registers it, verify both
entries landed and are last.)

**`up`** — mirror the `add_column` builder style of `m20260425_000002`, then add the partial index
via raw SQL (this codebase already uses `execute_unprepared` for index/constraint SQL that the
builder can't express, e.g. the CHECK constraints):
```rust
manager
    .alter_table(
        Table::alter()
            .table(CommandAuditLog::Table)
            .add_column(ColumnDef::new(CommandAuditLog::ParentAuditId).uuid().null())
            .to_owned(),
    )
    .await?;

manager
    .get_connection()
    .execute_unprepared(
        "CREATE INDEX IF NOT EXISTS idx_command_audit_log_parent_audit_id \
         ON command_audit_log (parent_audit_id) \
         WHERE parent_audit_id IS NOT NULL",
    )
    .await?;
Ok(())
```

**`down`** — drop index then column:
```rust
manager
    .get_connection()
    .execute_unprepared("DROP INDEX IF EXISTS idx_command_audit_log_parent_audit_id")
    .await?;
manager
    .alter_table(
        Table::alter()
            .table(CommandAuditLog::Table)
            .drop_column(CommandAuditLog::ParentAuditId)
            .to_owned(),
    )
    .await?;
Ok(())
```

Define the local iden enum in the new migration file (matching the `m20260425_000002` pattern):
```rust
#[derive(DeriveIden)]
enum CommandAuditLog {
    Table,
    ParentAuditId,
}
```

Equivalent raw SQL (for the reviewer / rollback record):
```sql
-- up
ALTER TABLE command_audit_log ADD COLUMN parent_audit_id uuid NULL;
CREATE INDEX idx_command_audit_log_parent_audit_id
  ON command_audit_log (parent_audit_id) WHERE parent_audit_id IS NOT NULL;
-- down
DROP INDEX IF EXISTS idx_command_audit_log_parent_audit_id;
ALTER TABLE command_audit_log DROP COLUMN parent_audit_id;
```

### Rollback note
Forward-only in production. The `down` path drops the column and index and is provided for
dev/test (`sea migrate down`) only; running it in production discards macro parent/child linkage
data. The column is nullable with no default and no FK, so `up` is a fast, non-rewriting
`ADD COLUMN` (Postgres 11+ adds a nullable column without a table rewrite) — safe on a populated
table. No backfill required: existing rows correctly get `NULL` (they predate parent linkage).

### Entity
**No change.** `entity/src/command_audit_log.rs:30` already declares
`pub parent_audit_id: Option<Uuid>` with the correct inferred type (`uuid null`). Do not
regenerate the entity.

### Tests hulk must add (this is the test that would have caught the bug)

Add a DB integration test under `rim_infra_db` gated by `--features db-tests` (run with
`cargo test -p rim_infra_db --features db-tests -- --test-threads=1`; it PANICs without
`DATABASE_URL` by design). Place it with the existing audit repo integration tests, or add an
audit test module if none exists — check `rim_infra_db/tests/` and the `tests/support` harness.

1. **`audit_insert_roundtrips_parent_audit_id`** — build an `AuditLogEntryRecord` with
   `parent_audit_id: Some(Uuid::now_v7())` and insert via `SeaOrmAuditLogRepository::insert`.
   Assert `Ok(())`. Then `list(...)` and assert the row comes back with the same
   `parent_audit_id`. Pre-fix, the insert fails with `column "parent_audit_id" does not exist` —
   this test is the regression guard.
2. **`audit_parent_child_linkage`** — mirror the macro path: insert a **child** row with
   `parent_audit_id: Some(P)` **first**, then insert the **parent** row with `id = P` and
   `parent_audit_id: None`, then insert a top-level row with `parent_audit_id: None`. Assert all
   three inserts succeed (proves child-before-parent ordering works with a plain column and no FK)
   and that `list` returns the child pointing at `P`. Use a macro outcome value
   (`AuditOutcome::StepError.as_str()` = `"step_error"`) on the child to also exercise the
   already-widened `outcome` CHECK.
   - `command_audit_log` has **no** FKs on `actor_user_id`/`server_id` (the create migration
     defines none, and the FK-policy migration predates the table), so the test may use arbitrary
     `Uuid`s without seeding users/servers. Keep it self-contained.

Also verify the test-support **TRUNCATE list** (`rim_infra_db/tests/support/mod.rs`, ~lines
98–133) includes `command_audit_log`; if it does not, add it so these tests clean up between runs.
(Independently flagged in `fable-notes.md` finding 5 — a one-line add here is fine; the broader
"query `information_schema`" refactor is out of scope for this spec.)

### Environment (from nick-fury)
Postgres 17 running locally; `DATABASE_URL` → `testdb`; migrations currently applied up to
`m20260428_194718`; Rust toolchain bumped to **1.96** tonight (a dependency MSRV forced it) — build
and test against 1.96. After writing the migration, apply it with `sea migrate up` and confirm
`sea migrate status` shows the new migration applied, then run the db-tests.

### Acceptance
- `sea migrate up` applies cleanly; `sea migrate status` lists the new migration.
- `sea migrate down` cleanly reverts it (column + index gone), then `up` again — proves both paths.
- `cargo test -p rim_infra_db --features db-tests -- --test-threads=1` passes, including the two
  new tests.
- No entity edit, no change to items 1–3.

---

## Summary of files hulk will touch

- `rim_server/Cargo.toml`, `rim_api/Cargo.toml`, `rim_infra_services/Cargo.toml` (item 1)
- `rim_infra_services/src/command/mod.rs` (item 2: field, setter, `list_commands`)
- `rim_infra_services/src/provider.rs` (item 2: single-instance wiring)
- `rim_infra_services/src/command/executor/{pin,reaction,state,admin,chat,conversation,chat_messages,member}.rs`
  and `rim_infra_services/src/command_tests.rs` (item 2: delete `macro_service: None`; convert
  the one injection site; add 3 tests)
- `rim_api/src/lib.rs` (item 3a: extract `wire_common_schema_data`)
- `rim_api/src/tests.rs` (item 3a: add SDL guard test)
- `rim_server/src/app.rs` (item 3b: use `with_handle`, drop dead imports)
- `migration/src/m20260708_*_add_command_audit_log_parent_audit_id.rs` (item 4: new migration,
  via `sea migrate generate`) and `migration/src/lib.rs` (register it, last in order)
- `rim_infra_db/tests/...` (item 4: two `db-tests` integration tests) and
  `rim_infra_db/tests/support/mod.rs` (item 4: ensure `command_audit_log` is in the TRUNCATE list)
- **No** entity change for item 4 — `entity/src/command_audit_log.rs` already declares the column.

## Follow-up for nick-fury (not in this spec's scope)
`export_sdl` in `rim_api/src/lib.rs` has no caller — `/api/sdl` serves the live `schema.sdl()`
(`server.rs:125`). After this dedup it is a thin, tested wrapper, but it is still dead. Recommend a
separate decision on deleting it (and its hardcoded registration defaults) once someone confirms
no out-of-repo tooling imports it. Not deleted here to honor the no-behavior-change constraint.
