# Command-Context Workspace Fallback (Option B)

**Date:** 2026-04-30
**Author:** Tony (backend architect)
**Implementer:** Hulk (programmer agent)
**Reviewer:** Hawkeye (mandatory, before report-back)
**Approved by:** Fury (Jakub) — Option B blessed

---

## 1. Background and Decision

The command executor layer has two inconsistent contracts for sourcing
`WorkspaceId` (a.k.a. `ServerId` in `rim_core`):

- **Context-based group** (already correct) — the registry declares
  `required_context: ["workspace", ...]`, and the executor reads
  `context.workspace_id`. Examples: `chat.reply_to_selected_message`,
  `chat.delete_message`, `chat.edit_message`. The palette wires
  `commandContext.workspaceId` straight to this field, so it works.

- **Args-based group** (broken on the palette path) — the registry declares
  `server_id` as a required argument, and the executor calls
  `parse_uuid_arg(&args, "server_id")?`. The AI surface fills the arg via
  the LLM tool schema, so it works there. The palette sends
  `commandContext.workspaceId` but no `args.server_id`, so it fails with
  `Validation("missing string arg \`server_id\`")`. This is finding #5 from
  the diagnosis.

**Option B — selected.** Keep the registry surface unchanged (so the AI tool
schema continues to require `server_id`, which the LLM populates), but make
the executor *fall back* to `context.workspace_id` when the arg is absent.
Args-wins-when-present preserves AI behaviour; context fallback unblocks the
palette.

**Hard out-of-scope:**

- No registry edits. `server_id` stays `required: true`. The AI tool-schema
  generator continues to surface it.
- No frontend edits. Palette toast / UX work is a separate Spider-Man task.
- No changes to the context-based group (already correct).
- No Option C (registry redesign of `WorkspaceId` sourcing). Long-term.
- **No migration**, schema change, or seaorm work. None required for this
  task. Confirmed: this spec ships zero `sea migrate generate` output.

---

## 2. Hard Rules (repeated so hulk doesn't miss them)

1. **NEVER** run `git commit`, `git push`, or `git merge`. Period.
2. Every new function MUST have:
   - At least one unit test exercising it.
   - A `///` doc comment describing contract, precedence, errors.
3. Migrations only via `sea migrate generate <name>`. **This spec needs zero
   migrations.** If you find yourself writing one, stop and re-read.
4. New modules default to `pub(crate) mod`. Only promote to `pub` with a
   documented external consumer named in the comment above the declaration.
5. Stay strictly in scope. The list of files to touch is enumerated below;
   anything outside that list is a separate task.
6. Hand back to **Hawkeye** for review before reporting done. No exceptions.
7. Test gate report format: `"N/N tests pass + build green"`. Only after
   running all five gate commands listed in §9.

---

## 3. The Helper

**Location:** `rim_infra_services/src/command/executor/mod.rs` — add directly
to the existing `Shared arg-parsing helpers` section (just after
`parse_uuid_arg`). A separate module is not warranted: this is one short
function, and `mod.rs` is where every executor already imports its shared
helpers from.

**Visibility:** `pub(super)` — same as the rest of the helpers in this file.
Only the sibling executor modules need it. Do not promote to `pub(crate)` or
`pub`.

### 3.1 Signature and contract

```rust
/// Resolve a `WorkspaceId`-typed argument with context fallback.
///
/// Precedence:
///   1. `args.<arg_name>` — used when the key is present and the value is a
///      non-empty string that parses as a valid UUID. The empty string and
///      whitespace-only strings are treated as **absent** (consistent with
///      `require_string_arg`).
///   2. `context.workspace_id` — used when the arg is absent OR empty/blank.
///      The `Option<&CommandContextDto>` is dereferenced with the
///      `.and_then(|c| c.workspace_id)` shape (i.e. either a missing context
///      or a context with `workspace_id: None` falls through to the error).
///   3. If neither path yields a value, returns
///      `ServiceError::Validation("missing workspace id: provide \
///       `<arg_name>` arg or `context.workspace_id`")`. The message MUST
///      mention BOTH paths so the developer debugging it knows what to
///      check.
///
/// A present-but-malformed arg is a hard error (returns
/// `Validation("invalid uuid arg `<arg_name>`")`). The function does NOT
/// silently fall through to context when the arg parses badly — that would
/// hide LLM tool-call bugs.
///
/// Generic over the target ID type so the same helper works for any
/// `WorkspaceId`-typed arg name. Today every call passes
/// `ServerId` and `"server_id"`, but the signature accepts any
/// `T: From<uuid::Uuid>` and any string key so future commands using a
/// different arg name (e.g. `workspace_id`) get free coverage.
///
/// # Examples
///
/// ```ignore
/// // Palette path — args is empty, context carries workspace.
/// let server_id = resolve_workspace_id::<ServerId>(&args, context.as_ref(), "server_id")?;
///
/// // AI path — args carries server_id, context is None.
/// // Same call site; arg wins.
/// let server_id = resolve_workspace_id::<ServerId>(&args, context.as_ref(), "server_id")?;
/// ```
pub(super) fn resolve_workspace_id<T>(
    args: &serde_json::Map<String, serde_json::Value>,
    context: Option<&CommandContextDto>,
    arg_name: &str,
) -> ServiceResult<T>
where
    T: From<uuid::Uuid>,
{
    // 1. Try the arg.
    if let Some(raw) = args.get(arg_name).and_then(|v| v.as_str()) {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            // Present and non-blank: parse strictly. A bad UUID here is a
            // hard error; we don't fall through to context because that
            // would mask LLM tool-call bugs.
            let uuid = uuid::Uuid::parse_str(trimmed).map_err(|_| {
                ServiceError::Validation(format!("invalid uuid arg `{arg_name}`"))
            })?;
            return Ok(T::from(uuid));
        }
    }

    // 2. Fall back to context.
    if let Some(ws) = context.and_then(|c| c.workspace_id) {
        // ServerId already wraps a UUID; bounce through Uuid to stay generic.
        let raw_uuid: uuid::Uuid = ws.into();
        return Ok(T::from(raw_uuid));
    }

    // 3. Neither path. Mention both in the error.
    Err(ServiceError::Validation(format!(
        "missing workspace id: provide `{arg_name}` arg or `context.workspace_id`"
    )))
}
```

> **Note on the `.into()` bounce in step 2.** `ServerId` is a newtype around
> `Uuid` (via `model_id!`), so `Uuid::from(server_id)` exists and round-trips
> losslessly. We bounce through `Uuid` to keep the helper generic over any
> `T: From<Uuid>` (the same bound `parse_uuid_arg` already uses). If you find
> a cleaner way to express this without breaking genericity, propose it in
> the PR; do not invent a new trait bound.

### 3.2 Imports needed in `mod.rs`

`CommandContextDto` is already imported at the top of `mod.rs` (currently
used only in the function signature of `execute_builtin_command`). No new
imports required.

---

## 4. Per-Executor Change Pattern

The pattern is mechanical. For every executor in the list below:

### 4.1 Worked example: `chat::list_channels`

**Before** (`rim_infra_services/src/command/executor/chat.rs`):

```rust
pub(super) async fn list_channels(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
) -> ServiceResult<CommandResultDto> {
    let server_id = parse_uuid_arg::<rim_core::server::model::ServerId>(&args, "server_id")?;
    // ... rest unchanged
}
```

**After:**

```rust
pub(super) async fn list_channels(
    services: &CommandServices,
    actor: Actor,
    args: serde_json::Map<String, Value>,
    context: Option<CommandContextDto>,
) -> ServiceResult<CommandResultDto> {
    let server_id = resolve_workspace_id::<rim_core::server::model::ServerId>(
        &args,
        context.as_ref(),
        "server_id",
    )?;
    // ... rest unchanged
}
```

**Plus** the dispatch site in `executor/mod.rs::execute_builtin_command` for
this command needs to pass `context` through:

```rust
"chat.list_channels" => chat::list_channels(services, actor, args, context).await,
```

`context` is already in scope at the dispatch site (it's a parameter of
`execute_builtin_command`). For executors that take `context` AND return
without consuming it later, you can pass `context` by value once at the call
site (the dispatch arm calls each executor exactly once). No `Clone` needed.

> **Caveat for dispatch ordering.** Inside
> `execute_builtin_command`, `context` is currently `Option<CommandContextDto>`
> by value. Each `match` arm consumes it independently — so the existing
> arms that already take `context` (`reply_to_selected_message`,
> `delete_message`, `edit_message`) consume it on their branches, and the
> new arms will too. Rust's borrow checker handles this fine because only
> one match arm runs per dispatch.

### 4.2 Imports

In each executor file, add `CommandContextDto` to the existing
`use rim_services::command::dto::{CommandResultDto, ...};` line, and replace
the `super::parse_uuid_arg` import with `super::resolve_workspace_id`.
Where the file calls `parse_uuid_arg` for **non-workspace** IDs
(`conversation_id`, `message_id`, `member_ids`), keep the import — those
calls don't change.

`chat_messages.rs` already imports `CommandContextDto`; just add the helper
import and use it for `post_message` (the other three functions in that file
are already context-based and don't change).

---

## 5. Scope: Full Executor Inventory

Audited via `rg "parse_uuid_arg::<.*ServerId>" rim_infra_services/src/command/executor`.
Every site below takes the workspace via args today and MUST migrate to the
helper. **17 call sites across 8 files.**

| # | File | Line | Function | Dispatch arm in `mod.rs` |
|---|------|------|----------|---------------------------|
| 1 | `chat.rs` | 37 | `list_channels` | `"chat.list_channels"` |
| 2 | `chat.rs` | 81 | `create_channel` | `"chat.create_channel"` |
| 3 | `chat.rs` | 131 | `mark_as_read` | `"chat.mark_as_read"` |
| 4 | `chat.rs` | 160 | `mark_all_as_read` | `"chat.mark_all_as_read"` |
| 5 | `chat.rs` | 191 | `search_messages` | `"chat.search_messages"` |
| 6 | `chat.rs` | 256 | `get_recent_messages` | `"chat.get_recent_messages"` |
| 7 | `chat_messages.rs` | 16 | `post_message` | `"chat.post_message"` |
| 8 | `member.rs` | 22 | `list_members` | `"chat.list_members"` |
| 9 | `admin.rs` | 15 | `invite_member` | `"admin.invite_member"` |
| 10 | `conversation.rs` | 17 | `create_group` | `"chat.create_group"` |
| 11 | `pin.rs` | 16 | `pin_message` | `"chat.pin_message"` |
| 12 | `reaction.rs` | 24 | `toggle_reaction` | `"chat.toggle_reaction"` |
| 13 | `state.rs` | 15 | `get` | `"state.get"` |
| 14 | `state.rs` | 57 | `set` | `"state.set"` |
| 15 | `state.rs` | 100 | `delete` | `"state.delete"` |
| 16 | `state.rs` | 138 | `list` | `"state.list"` |
| 17 | `state.rs` | 182 | `incr` | `"state.incr"` |

**Note on Fury's enumeration.** Fury named 7 chat-namespace commands.
Re-grep adds `chat.create_group` (`conversation.rs`), `chat.pin_message`,
`chat.toggle_reaction`, `admin.invite_member`, and the **5 state.\*
commands**. All 17 are in scope. Do not skip the state.* family — they
have the same break and the same fix.

**Not in scope** (already correct, do NOT touch):
- `chat.reply_to_selected_message` — context-based, correct.
- `chat.delete_message` — context-based, correct.
- `chat.edit_message` — context-based, correct.
- `desktop_local::*` — handled out-of-band by the FE, no workspace id.

---

## 6. Audit-Log Behaviour

**No new audit fields. No changes to audit emission.** The spec confirms:

- The audit row's `server_id` field is **already** sourced from
  `context.workspace_id` in `command/mod.rs::execute_command` (see the
  snapshot at line 226-231). So the palette path was already producing
  correct audit `server_id` values *before* this change — only the
  executor body was broken. Confirmed: nothing to update in the audit
  pipeline.

- After this change, the resolved `ServerId` that reaches the executor's
  body and downstream service calls (e.g. `ListChannelsQuery::server_id`,
  `MarkAsReadCmd::workspace_id`) is whatever `resolve_workspace_id`
  returns. Downstream service-layer audit/observability sees the correct
  workspace either way. No semantic change for the AI path
  (args wins → same value as today). For the palette path, the executor
  now sees `context.workspace_id` and forwards it; previously it errored
  before any service call.

- The `command_executed` tracing line and the `audit_log_service.write_entry`
  call both use `server_id_for_audit` from context. They keep working
  unchanged.

Anything that *would* introduce a new audit field is out of scope for this
task.

---

## 7. Tests Required

For each executor changed, hulk MUST add the following four unit tests
(mockall, in-process). Co-locate next to the existing executor tests; if a
file has no existing test module yet, add one as a `#[cfg(test)] mod tests`
inside the same source file (consistent with the existing pattern in
`mod.rs`'s peers).

### 7.1 Per-executor tests (×17 executors)

For each executor `<exec>` in the table at §5:

1. **`<exec>_args_present_uses_arg_value`** — call with
   `args: { "server_id": "<uuid_a>", ...other_required_args }` and
   `context: None`. Assert downstream service mock is called with
   `server_id == uuid_a`. **Regression guard for the AI surface.**
2. **`<exec>_args_absent_uses_context_value`** — call with
   `args: { ...other_required_args (NO server_id) }` and
   `context: Some(CommandContextDto { workspace_id: Some(uuid_b), ... })`.
   Assert downstream service mock is called with `server_id == uuid_b`.
   **Palette regression guard — this is the bug we're fixing.**
3. **`<exec>_args_present_and_context_present_args_wins`** — both supplied,
   different UUIDs. Assert downstream sees the **arg** value. Locks the
   precedence rule against future drift.
4. **`<exec>_both_absent_returns_validation`** — `args: {}`,
   `context: None` (or `context.workspace_id: None`). Assert returns
   `ServiceError::Validation` whose message contains both `"server_id"` and
   `"context.workspace_id"`. The AND-message contract is what makes a
   future debugger's life bearable.

For executors that have OTHER required args (e.g. `chat.post_message` needs
`conversation_id` and `text`), the tests still need to populate those — the
"args absent" case means **only** the workspace arg is absent. Don't water
down the other required fields.

> **Volume note.** 17 executors × 4 tests = 68 new unit tests. That's the
> right number; the correctness payload here is "every executor migrated
> with the same precedence guarantees". Don't shortcut to a parameterised
> table-test — keeping them as named tests gives clearer failure
> attribution when one executor regresses.

### 7.2 Helper-in-isolation tests (in `executor/mod.rs::tests` module)

Add a fresh `#[cfg(test)] mod tests` inside `mod.rs` if one isn't there
yet (the file currently has none). Tests:

1. **`resolve_workspace_id_arg_only_returns_arg_uuid`** — args has the key,
   context is `None`. Returns the arg UUID.
2. **`resolve_workspace_id_context_only_returns_context_uuid`** — args is
   empty, context has `workspace_id: Some(_)`. Returns the context UUID.
3. **`resolve_workspace_id_arg_wins_when_both_present`** — both supplied
   with different UUIDs. Returns the arg UUID.
4. **`resolve_workspace_id_arg_present_but_empty_falls_back_to_context`** —
   `args: { "server_id": "" }`, context has workspace. Returns the context
   UUID. Locks the explicit "blank arg = absent" choice (§3.1 rule).
5. **`resolve_workspace_id_arg_present_but_whitespace_falls_back_to_context`**
   — same as above with `"   "`. Same expectation.
6. **`resolve_workspace_id_neither_present_returns_validation_with_both_paths_in_msg`**
   — args empty, context `None`. Asserts:
   - `matches!(err, ServiceError::Validation(_))`,
   - the message contains the literal `"server_id"`,
   - the message contains the literal `"context.workspace_id"`.
7. **`resolve_workspace_id_malformed_arg_returns_validation`** —
   `args: { "server_id": "not-a-uuid" }`, context has workspace. Asserts
   the result is `ServiceError::Validation` with `"invalid uuid"` in the
   message. **Crucially: does NOT silently fall through to context.** This
   test locks §3.1 rule about not masking LLM tool-call bugs.
8. **`resolve_workspace_id_context_with_no_workspace_id_falls_through_to_error`**
   — `context: Some(CommandContextDto { workspace_id: None, ... })`, args
   empty. Returns `Validation` (not a panic, not the context's nil-UUID).
9. **`resolve_workspace_id_context_none_with_args_works`** — sanity: even
   with `context = None`, an arg-only call returns the arg UUID.

### 7.3 Integration tests (palette path, live DB)

Add to a new file `rim_infra_services/tests/command_palette_workspace_fallback_test.rs`,
gated behind feature `infra-db-tests` (consistent with
`infra_db_tests_guard.rs`). Build on the shape of
`chat_list_channels_agent_test.rs` (same `build_service` pattern, same
mocking style — but with a real DB-backed `ConversationService` /
`ServerMemberService`). Two tests:

1. **`palette_chat_list_channels_uses_context_workspace_id_when_args_omit_server_id`**
   — `args: None` (or `args: Some(json!({}))`), `context:
   Some(CommandContextDto { workspace_id: Some(real_workspace_id), ... })`,
   `initiator: CommandInitiator::Palette`. Real workspace seeded with at
   least one channel. Assert `result.status == Success`, payload
   `channels` array is non-empty.
2. **`palette_chat_list_members_uses_context_workspace_id_when_args_omit_server_id`**
   — same shape against `chat.list_members`. Asserts
   `result.status == Success`, payload `members` array non-empty.

> **Do NOT delete `chat_list_channels_agent_test.rs`** — it locks the
> agent path. The new test locks the palette path. Both contracts must
> hold.

### 7.4 Documentation tests

Each new function (`resolve_workspace_id`) gets a `///` doc comment per
§3.1. The doc comment includes a `# Examples` section using `ignore` (we
can't easily build a `CommandContextDto` in a doctest without exposing the
constructor). The example block in §3.1 above is what you commit verbatim.

---

## 8. Implementation Order (sequential — do not parallelise)

1. **Helper + helper tests.** Add `resolve_workspace_id` to
   `executor/mod.rs`. Add the 9 helper-in-isolation tests (§7.2). Run
   `cargo test -p rim_infra_services resolve_workspace_id`. All 9 must
   pass before moving on. **Do not edit any executor yet.**

2. **Migrate one executor as a beachhead.** Pick `chat::list_channels`
   (most existing tests, easiest to verify nothing regresses). Edit:
   - `chat.rs::list_channels` signature + body per §4.1.
   - `mod.rs` dispatch arm to pass `context`.
   - Add the 4 unit tests for `list_channels` per §7.1.
   - Run `cargo test -p rim_infra_services`. All existing tests
     (including `chat_list_channels_agent_test.rs`'s 3 legs) MUST still
     pass.

3. **Migrate the remaining 16 executors in one pass.** Same pattern. Add
   all per-executor unit tests as you go. Run `cargo test -p
   rim_infra_services` after each file to catch issues early.

4. **Add the 2 palette-path integration tests** per §7.3.

5. **Run the full gate** per §9. Only when all five commands are green
   do you report back.

---

## 9. Gate Commands (run all five, in order)

```sh
cargo build --workspace
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
```

Then, with `DATABASE_URL` exported (Fury confirmed:
`postgres://postgres:postgres@localhost:5432/testdb`):

```sh
export DATABASE_URL="postgres://postgres:postgres@localhost:5432/testdb"
cargo test -p rim_infra_services --features infra-db-tests -- --test-threads=1
```

All five must be green. Report format:
`"<N>/<N> tests pass + build green"` where `N` is the total count after
this work. (Total = previous total + 68 per-executor unit tests + 9
helper unit tests + 2 integration tests. If any pre-existing test fails,
stop and report — do not "fix" it without checking with Tony.)

---

## 10. Hand-back

After all gates pass:

1. Hand the change set to **Hawkeye** for code review. Do not skip this
   step. The CLAUDE.md mandates Hawkeye review for every code change.
2. Once Hawkeye signs off (or after addressing BLOCK findings), report
   back to Tony with:
   - Test gate report in the format above.
   - List of files touched (absolute paths).
   - Hawkeye's verdict.
3. Tony reviews and reports up to Fury.

**No git operations at any step.** That's Fury's call.

---

## 11. Quick Reference: Where Things Live

- Helper goes in `rim_infra_services/src/command/executor/mod.rs`.
- Per-executor changes touch the 8 files listed in §5 plus
  `executor/mod.rs` (dispatch arms).
- Helper unit tests: `executor/mod.rs` `#[cfg(test)] mod tests`.
- Per-executor unit tests: same file as the executor, in its
  `#[cfg(test)] mod tests` block.
- Integration tests:
  `rim_infra_services/tests/command_palette_workspace_fallback_test.rs`.
- Existing agent-path test stays at
  `rim_infra_services/tests/chat_list_channels_agent_test.rs`.

---

## 12. Anti-checklist (things this spec is NOT asking for)

- Not asking for a registry edit.
- Not asking for a frontend/palette toast change.
- Not asking for a migration.
- Not asking for changes to `parse_uuid_arg` itself (it stays — non-workspace
  IDs still use it).
- Not asking for changes to context-based executors.
- Not asking for new audit fields.
- Not asking for a `pub` API.
- Not asking for git operations.

If you find yourself doing any of the above, stop and re-read the spec.
