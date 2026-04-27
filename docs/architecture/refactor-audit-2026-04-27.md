# Backend Refactor Audit — 2026-04-27

**Auditor:** Nick Fury (software-architect)
**Repo:** `rim-server`
**Method:** Static read-only inspection. Every finding has `path:line`.
**Trigger:** Kuba flagged "macros for allowing many arguments… why don't we create some struct for it?" — broadened to general code-quality + arch-drift sweep.

---

## TL;DR

- **10 findings**: 4 HIGH, 4 MEDIUM, 2 LOW.
- Kuba's specific complaint (long arg lists) is real and concentrated in three places:
  1. `update_profile` repo trait (4× nested `Option<Option<String>>` + id = 5 args).
  2. `Observer.on_*` event emitters (5–7 positional args, 9 such methods, identical shape).
  3. GraphQL pagination resolvers (`messages` / `thread_messages` / `list_incidents`, 7–8 params).

### Top 3 priorities

1. **F2** — Collapse 9 observer fan-out methods behind a single `EventEnvelope` struct + `emit(envelope)` API. Eliminates ~80 LOC of arg-list noise; future event fields don't touch every call site.
2. **F4** — Replace `map_repo_error` (97 occurrences) with a `From<RepoError> for ServiceError` impl. Mechanical win; deletes 97+ `.map_err(...)` call sites; makes `?` propagation natural.
3. **F1** — Convert `update_profile(id, Option<Option<String>>×4)` to `update_profile(id, UpdateProfilePatch)`. Kuba's exact complaint, smallest blast radius of the long-arg fixes.

---

## Findings

### F1: `update_profile` repo trait — 5 args with 4× nested `Option<Option<String>>` — HIGH

**Location:** `rim_core/src/user/repository.rs:21-28`
**Smell:** Nested `Option<Option<String>>` ("absent" vs "set to null" vs "set to value") is a recognized Rust idiom, but **four** of them in positional order makes every call site read like noise; every new optional field is a breaking signature change. Repository trait — touched by every `UserRepository` impl + mock.
**Proposed refactor:** Introduce `UpdateProfilePatch` in `rim_core::user::model`:

```rust
pub struct UpdateProfilePatch {
    pub display_name: Option<Option<String>>,
    pub avatar_url:   Option<Option<String>>,
    pub status_text:  Option<Option<String>>,
    pub bio:          Option<Option<String>>,
}
```

Trait becomes `update_profile(&self, id: UserId, patch: UpdateProfilePatch) -> RepoResult<User>`. Future fields (`pronouns`, `timezone`) extend the struct — no signature break.
**Effort:** S (1–2 hrs).
**Risk:** Internal trait + one SeaORM impl + mock. No public API impact (frontend hits GraphQL, which already wraps these in an input object). Test coverage: `user_unit_tests.rs`.
**Ticket:** P7-RFAC-01.

---

### F2: Observer fan-out methods — 9 functions × 5–7 positional args, identical shape — HIGH

**Location:** `rim_services/src/events/observers.rs:32-349` (whole file).
**Smell:** Nine `on_*` methods (`on_message_sent`, `on_message_updated`, `on_message_deleted`, `on_reactions_changed`, `on_thread_created`, `on_message_pinned`, `on_message_unpinned`, `on_typing_started`, `on_member_added_to_conversation`, `on_member_removed_from_conversation`, …) all do the same thing: take 4–7 positional args, build a `WorkspaceEvent` variant, call `notify`. Every call site (e.g. `rim_infra_services/src/message.rs:468-474`, `pin.rs`, `read_state.rs`) is a 5–7-line tuple-call. Adding a field (e.g. `correlation_id` for AI agent traceability — coming in Phase 1 reframe) requires touching every call site.
**Proposed refactor:** `WorkspaceEvent` already carries the data. Drop the 9 redundant `on_*` methods, keep one entry point:

```rust
impl<O: EventObserver> O {
    fn emit(&self, event: WorkspaceEvent) { self.notify(event); }
}

// Call site:
self.observers.message.emit(WorkspaceEvent::NewMessage {
    workspace_id: cmd.server_id,
    conversation_id: dto.conversation_id,
    …
});
```

Or expose `WorkspaceEvent::new_message(...)` constructors and keep `notify(event)` only. Either way, the per-method positional-arg trampolines disappear.
**Effort:** M (4–6 hrs across ~25 call sites).
**Risk:** Internal-only; `WorkspaceEvent` is already the wire format. Test coverage at sites is good.
**Ticket:** P7-RFAC-02.

---

### F3: GraphQL pagination resolvers — 7–8 params each — HIGH

**Locations:**
- `rim_api/src/query.rs:381-390` (`messages`: 7 params after `&self`/ctx)
- `rim_api/src/query.rs:627-637` (`thread_messages`: 8 params)
- `rim_api/src/query.rs:1071-1079` (`list_incidents`: 8 params)
- Several similar in `query.rs` (the pagination triplet `dir/cursor/limit` repeats everywhere).

**Smell:** Resolver signatures with `(ctx, server_id, conversation_id, [thread_id], dir, cursor, limit, include_deleted)` are unreadable; the `(dir, cursor, limit)` group is duplicated across ~10 resolvers.
**Proposed refactor:** Two-step:

1. **`PageArgs` InputObject** in `rim_api/src/pagination.rs`:
   ```rust
   #[derive(InputObject)]
   pub struct PageArgs<C: InputType> {
       pub dir: PageDir,
       pub cursor: Option<C>,
       pub limit: i32,
   }
   ```
   async-graphql allows generic InputObjects via `concrete(...)` attributes. If too painful, write 3–4 concrete variants (`MessagePageArgs`, `IncidentPageArgs`, …) and accept the small duplication for clarity.

2. **`MessageScope` InputObject** for `(server_id, conversation_id, [thread_id])` — appears in `messages`, `thread_messages`, `send_message`, `edit_message`, `delete_message`, `pin_message`, `unpin_message`, `toggle_reaction`. Rule of three+ — strong signal.

**Effort:** M (1 day for `PageArgs`, ~half day for `MessageScope` if you do schema migration in one PR).
**Risk:** **Breaking GraphQL schema change** — frontend codegen will regenerate types; coordinate with Tony before shipping. If SDL diff isn't acceptable now, scope down to backend-only refactors and live with the resolver arg count.
**Ticket:** P7-RFAC-03 (HIGH RISK: SDL change).

---

### F4: `map_repo_error` boilerplate — 97 sites — HIGH

**Location:** `rim_services/src/lib.rs:122-138` (the function); 97 call sites across 12 files in `rim_infra_services/src/`. Same for `map_service_error` in `rim_api` (61 sites, 1 file).
**Smell:** Every repo call ends in `.await.map_err(rim_services::map_repo_error)?`. Exactly what `From` impls + `?` exist for.
**Proposed refactor:**

```rust
// rim_services/src/lib.rs
impl From<RepoError> for ServiceError {
    fn from(err: RepoError) -> Self { /* current map_repo_error body */ }
}
```

After: every `.map_err(rim_services::map_repo_error)?` becomes just `?`. Keep the free function as a deprecated shim during migration if you want incremental rollout.

For `rim_api`: `async_graphql::Error` is foreign — orphan rule blocks `From<ServiceError> for Error`. Either newtype-wrap it in `GqlError` (deletes 61 sites too), or accept the 61 remaining `map_service_error` sites and only fix the 97 in services.

**Effort:** S–M (2–4 hrs, mostly mechanical).
**Risk:** Low — pure refactor, no behavior change. Compile-clean rebuild proves equivalence; clippy flags stragglers.
**Ticket:** P7-RFAC-04.

---

### F5: `command.rs` is 3309 lines, multiple responsibilities — MEDIUM

**Location:** `rim_infra_services/src/command.rs` (3309 LOC).
**Smell:** Single file holds: `CommandServices` struct, `DefaultCommandService`, builtin dispatch, user-command recursion (`execute_user_command_recursive`), pipe execution (`execute_piped`), capability gating, confirmation gate, rate-limit gate, AI surface filter, and ~1800 LOC of inline tests. Adding a new gate (e.g. agent budget enforcement coming in Phase 1 reframe) means scrolling 3300 lines.
**Proposed refactor:** Submodule scaffolding already exists (`builtin_executor`, `pipe_parser`, `rate_limiter`, `registry`, `user_commands`). Extract:
- `gates.rs` — `dispatch_required_capabilities`, `dispatch_requires_confirmation`, the `execute_command_inner` gate ladder (lines 764–923).
- `surface.rs` — `filter_ai_surface`, `command_matches_surface`.
- `dispatch.rs` — `resolve_command_definition`, `execute_resolved_command_recursive`, `execute_builtin`.
- Keep `DefaultCommandService` shell + trait impl in main file (~300 LOC).
- Move tests to a sibling `command_tests.rs` per repo's existing `*_unit_tests.rs` convention.
**Effort:** M (1 day, mostly cut/paste + visibility adjustments).
**Risk:** Low — internal organization. Watch `pub(crate)` adjustments on helpers that move modules.
**Ticket:** P7-RFAC-05.

---

### F6: Triple-guard pattern (`ensure_server_member` → `ensure_conversation_in_server` → `ensure_conversation_access`) duplicated 21× — MEDIUM

**Locations:** `rim_infra_services/src/{message,message_attachment,pin,conversation,thread,read_state,message_reaction}.rs` — 21 occurrences across 7 files.
**Smell:** Every conversation-scoped operation runs the same 3 guards in the same order. Different param shapes (some take `cmd.actor.user_id`, some take a passed-in `actor`), but the core 3-call dance is identical.
**Proposed refactor:** Helper in `rim_infra_services/src/util.rs`:

```rust
pub async fn ensure_conversation_member_access(
    repos: &ConversationAccessRepos, // tiny struct of 3 Arcs
    actor: UserId,
    server_id: ServerId,
    conversation_id: ConversationId,
) -> ServiceResult<Conversation> { /* the 3 calls, returning the loaded Conversation */ }
```

Returning the loaded `Conversation` skips re-fetching for `conversation_type` lookups (which several sites do today after the guard). Win in clarity and DB calls.
**Effort:** M (4–6 hrs, 21 sites).
**Risk:** Low. Each unit-test file already covers these paths. Preserve error-variant order from the 3 guards (NotFound vs PermissionDenied — security-audit-relevant; flag Widow on PR).
**Ticket:** P7-RFAC-06.

---

### F7: Layer leak — `rim_api` reaches into `rim_infra_services` — MEDIUM

**Locations:**
- `rim_api/src/mutation.rs:2100` — `rim_services::link_preview::extract_urls(text)` called directly from a resolver. *(Note 2026-04-28: Original audit cited `rim_infra_services::link_preview::extract_urls` at `:2099`; the F7 layer-leak fix landed and moved the free fn to `rim_services::link_preview`. Line-number drift is also captured here.)*
- `rim_api/src/mutation.rs:1128, 1183` — `Arc<rim_infra_services::typing::TypingTracker>` and `PresenceManager` pulled from GQL context.
- `rim_api/src/subscription.rs:8`, `rim_api/src/lib.rs:34, 35, 118, 119`, `rim_api/src/query.rs:821` — same `PresenceManager` / `TypingTracker` reach-through.
- `rim_api/Cargo.toml:10` — `rim_infra_services = { path = "../rim_infra_services" }` is a non-dev dep.

**Smell:** Per CLAUDE.md, dependency arrow is `rim_api → rim_services → rim_infra_services`. Today `rim_api` directly depends on `rim_infra_services` for three concrete types. Architectural shortcut bypassing the trait boundary; locks `rim_api` to a specific implementation.
**Proposed refactor:**
1. Define `PresenceService` and `TypingService` traits in `rim_services::presence` (already exists — extend it) and `rim_services::typing`. Move `PresenceManager`/`TypingTracker` to be `Default*Service` impls behind the traits.
2. Add `extract_urls` to `LinkPreviewService` trait. Resolver calls `link_svc.extract_urls(text)` instead of importing the free fn.
3. Drop `rim_infra_services` from `rim_api/Cargo.toml`'s `[dependencies]` (keep in `[dev-dependencies]` for tests).
**Effort:** M (1 day).
**Risk:** Low — pure plumbing. No behavior change.
**Ticket:** P7-RFAC-07.

---

### F8: `unwrap()` in production binary code — MEDIUM

**Locations:**
- `rim_server/src/config.rs:89` — `RIMConfig::load().unwrap()`
- `rim_server/src/main.rs:28` — `start_server(&config).await.unwrap()`
- `rim_auth/src/session_token.rs:8` — `fill(&mut bytes).expect("getrandom failed")`
- `rim_auth/src/session_token.rs:46, 104` — `strip_prefix("rim_sess_").unwrap()` (verify cfg(test) adjacency).

**Smell:** CLAUDE.md §Conventions explicitly forbids `unwrap`/`expect` in library code. The two binary-edge unwraps (`config.rs:89`, `main.rs:28`) swallow error context. The `getrandom` expect panics-on-OS-failure with no audit-channel notice.
**Proposed refactor:**
- `main.rs` returns `Result<(), Box<dyn Error>>`. Print + exit-1 on error rather than panic; clean shutdown for the logging worker guard (today dropped during panic unwind — silent loss of last audit lines).
- `config.rs` returns `Result<RIMConfig, ConfigError>`, propagate to `main`.
- `session_token.rs:8` — `getrandom` failure is genuinely fatal; log via `tracing::error!(target = "audit", …)` before unwinding so it's visible in audit forensics.
**Effort:** S (1–2 hrs).
**Risk:** Low. No behavior change happy-path; observability improvement on startup failure.
**Ticket:** P7-RFAC-08.

---

### F9: Test fixture duplication across `*_unit_tests.rs` — LOW

**Location:** Every `rim_infra_services/src/*_unit_tests.rs` repeats `let actor = Actor::new(UserId::new());` and similar `MockServerMemberRepository` setup. ~200+ occurrences across 11 files.
**Smell:** Test fixtures duplicated across crates. Classic candidate for a `rim_test_support` crate.
**Proposed refactor:** New `rim_test_support` crate (dev-dep only) exposing:
- `fixtures::actor()` → `Actor` (builder: `actor().with_user_id(...).build()`).
- `fixtures::server_member_mock()` → preconfigured `MockServerMemberRepository`.
- `fixtures::observers_noop()` → `Observers` with no-op broadcaster.
- Move `rim_infra_db/tests/support/mod.rs` shared helpers here too.
**Effort:** L (1–2 days; value comes from migration over many test files).
**Risk:** Tests-only.
**Ticket:** P7-RFAC-09. **Defer** until F1–F8 done; pure quality-of-life.

---

### F10: Public surface area in `rim_infra_services` is too wide — LOW

**Location:** `rim_infra_services/src/lib.rs:1-38` — every module is `pub`.
**Smell:** Only the trait + `DefaultAppServiceProvider` should be the public contract. Pre-F7, `rim_api/src/mutation.rs:2099` exploited this to call `rim_infra_services::link_preview::extract_urls` directly. F7 has since landed: the call now reads `rim_services::link_preview::extract_urls` at `:2100`. Closing F7 enables tightening here.
**Proposed refactor (after F7 lands):**
- Most modules → `pub(crate)`.
- Keep public: `provider`, `repos`, and `presence`/`typing` only if F7 trait migration leaves them as the public access point.
- `Default*Service` only needs `pub` for tests + provider.
**Effort:** S (2 hrs after F7).
**Risk:** Low. Compiler enforces correctness.
**Ticket:** P7-RFAC-10. **Dependency: F7 must land first.**

---

## Refactor Sequencing (recommended order)

1. **F4** — pure mechanical, no risk, deletes 97+ noisy `.map_err` sites. Makes every subsequent refactor easier to read in diff.
2. **F1** — small, isolated, knocks out Kuba's specific complaint. Confidence builder.
3. **F8** — small, easy. Get rid of binary-edge `unwrap`s while in `main.rs`.
4. **F2** — observer collapse. Touches ~25 call sites. Do this before F6 because the observer call sites are inside the same service files where F6 lives.
5. **F6** — triple-guard helper. F2's smaller call sites are now easier to read.
6. **F7** — close layer leak. Unblocks F10.
7. **F5** — split `command.rs`. Independent of the above; can be parallelized.
8. **F10** — visibility tightening. Trivial after F7.
9. **F3** — GraphQL `PageArgs` / `MessageScope`. **DO LAST** because of SDL break risk; bundle with a frontend codegen sync milestone.
10. **F9** — `rim_test_support` extraction. Defer; quality-of-life, not a defect.

---

## Out of scope (intentionally not flagged)

- **`sensitivity.rs`, `ConfirmationProof`, AI agent reframe surface** — Phase 1 already owns this territory.
- **CSRF / session cookie / Argon2 wiring** — Black Widow's domain.
- **GraphQL UX shape (field naming, nullability)** — Ant-Man's department.
- **`txn_op!` macro in `rim_infra_db/src/db_handle.rs:69`** — 3-arg helper, single use site, well-named. Kuba's flag was about argument count; this one has 3.
- **`model_id!` macro** — single-arg, clean. Kuba's complaint must have targeted the observer fan-out or `update_profile` repo trait.
- **`AppRepos` struct (29 fields, `repos.rs:34-64`)** — looks bad on paper but is a wiring registry, not business logic. Splitting it doesn't help; it's a service locator.
- **`types.rs` size (2645 LOC)** — all GraphQL `SimpleObject`/`InputObject` definitions + `From` impls. Splitting by domain (e.g. `types/message.rs`, `types/command.rs`) is plausible but low value vs. F5.
- **`pipeline.rs` (1004 LOC), `keybind.rs` (1195 LOC), `message.rs` (1258 LOC)** — at the edge of "too big" but each has a coherent single responsibility. Watch them; don't split yet.
