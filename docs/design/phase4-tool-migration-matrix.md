# Phase 4 Tool Migration Matrix

Audit of the 12 hardcoded TS tools in `rim-ui/src/features/agent/tools/builtins/`
remaining after `chat.list_channels` migrated in Phase 3 (P3-A2/B3). Drives
parallel backend migration tasks P4-A4..P4-A13.

**Source:** `rim-ui/src/features/agent/tools/builtins/` (commit at audit time).
**Backend reference:** `rim_infra_services/src/command/registry.rs::builtin_commands_raw`.

---

## Audit Notes (read before the matrix)

- **Sensitivity enum naming.** The dispatch brief referenced
  `Safe | Notable | Standard | Mutating | Dangerous` (5 tiers, "Ant-Man r2.1").
  The actual backend enum at
  `rim_services/src/command/sensitivity.rs:73` is
  `Safe | Sensitive | Mutating | Restricted` (4 tiers — Fury v2 §3.1, "unchanged
  from v1"). This matrix uses the real backend names. If the 5-tier model is
  intended, that's a Phase 4 enum-rename task with a re-audit; flag for Tony.
- **Existing-backend-but-non-AI rows.** Six FE tools have a backend command
  with the same intent already registered (`chat.post_message`,
  `chat.create_channel`, `chat.open_thread`, `workspace.open_settings`,
  `workspace.navigate_to_channel`, `workspace.switch_workspace`). For these the
  migration is a metadata flip + nl_description/intent_samples — NOT a new
  command. The naming column flags whether the existing command's args match
  the FE tool or need extension.
- **Three FE-only tools** (`get_current_context`, `navigate_to_channel`,
  `open_thread`, `switch_workspace`, `open_settings`) execute against
  Zustand stores, not HTTP. They map cleanly to backend commands with
  `execution_target: DesktopLocal` or to a new commandContext bridge that
  reads on the backend side from `CommandContextInput`. The matrix flags this
  per-row under "Migration shape".
- **`get_current_context` is special.** It reads runtime FE state and has no
  backend analogue. Two viable paths: (a) keep as a hardcoded builtin forever
  (cheapest, fine for an open-ended audit logger to track), or (b) synthesize a
  backend command that just echoes back the request's `commandContext`. Matrix
  recommends (a) — flagged.
- **Six new builtins required.** `list_members`, `search_messages`,
  `get_recent_messages`, `pin_message`, `add_reaction` have no backend
  command at all today. They need new entries in `registry.rs` plus
  executor wiring in `builtin_executor.rs` analogous to the
  `chat.list_channels` template from P3-A2.
- **Capability gates.** All write tools require `chat.write` (already seeded
  in Phase 1's `builtin_capabilities()`). Read tools today have no required
  cap. The matrix proposes `chat.read` for read tools — that's a new cap key
  needing seeding in Phase 4 if not already present (verify via
  `rim_infra_services/src/capability.rs::builtin_capabilities`).
- **`workspace_id` argument naming.** All current FE tools use
  `workspace_id` in their input_schema. Backend convention is `server_id`
  (P3 already proved this — `chat.list_channels` declares `server_id`,
  `chat.post_message` likewise). Per-row "Args" column shows the **backend
  arg name** (what the migrated command will declare). The agent's input
  schema follows.

---

## Matrix

| # | FE tool name | FE handler | Backend op invoked today | Existing backend command | Args (post-migration) | Response shape (post-migration) | Class | Sensitivity | required_capabilities | Migration shape |
|---|---|---|---|---|---|---|---|---|---|---|
| 1 | `list_members` | `listMembers.ts` | `serverMembers(serverId)` query | **none** | `server_id: WorkspaceId (req)` | `{ members: [{ username, role, joined }] }` | read | `Safe` | `["chat.read"]` | NEW backend command `chat.list_members`. Mirror `chat.list_channels` template. |
| 2 | `search_messages` | `searchMessages.ts` | `searchMessages(serverId, conversationId?, query, limit, includeDeleted)` query | **none** | `server_id: WorkspaceId (req)`, `query: String (req)`, `conversation_id: ConversationId (opt)`, `limit: Integer (opt, default=10, max=50)` | `{ results: [{ id, channel, author, text, sentAt }] }` | read | `Safe` | `["chat.read"]` | NEW `chat.search_messages`. Backend resolver should also DataLoader-resolve usernames so the FE-side `memberResolver` workaround can be deleted. |
| 3 | `get_recent_messages` | `getRecentMessages.ts` | `messages(serverId, conversationId, limit, before?)` query, paginated | **none** | `server_id: WorkspaceId (req)`, `conversation_id: ConversationId (req)`, `limit: Integer (opt, default=30, max=100)` | `{ messages: [{ author, text, sentAt }] }` (deleted filtered) | read | `Safe` | `["chat.read"]` | NEW `chat.get_recent_messages`. Move pagination loop server-side; backend should respect `limit` even if internal page cap is 50. Also resolve usernames via DataLoader. |
| 4 | `get_current_context` | `getCurrentContext.ts` | NONE — reads `useShellStore` + `useWorkspaceStore` | **none** | (none) | `{ workspaceId, conversationId, isThreadOpen, rootMessageId }` | read | `Safe` | `[]` | **STAY HARDCODED.** No backend value-add — backend would just echo `commandContext` it received. Recommend keeping as builtin and documenting the carve-out in `loader.ts`. |
| 5 | `navigate_to_channel` | `navigateActions.ts` | NONE — sets Zustand `selectedConversationId` + main surface | `workspace.navigate_to_channel` (DesktopLocal, `arguments: vec![]`) | `conversation_id: ConversationId (req)` ← NEW arg | `{ success, navigatedTo }` | navigation | `Safe` | `[]` | EXTEND existing command: add `conversation_id` arg. `execution_target` stays `DesktopLocal`. Frontend dispatcher (P4-FE) reads payload + dispatches to Zustand. Set `ai_callable: true`. |
| 6 | `open_thread` | `navigateActions.ts` | NONE — calls `useShellStore.openThreadForMessage` | `chat.open_thread` (DesktopLocal, `arguments: vec![]`, `required_context: ["selected_message"]`) | `root_message_id: MessageId (req)` ← NEW arg | `{ success, threadOpened }` | navigation | `Safe` | `[]` | EXTEND existing command: add `root_message_id` arg. Decouple from `required_context` for AI surface (or keep context for palette path; AI surface passes explicit). `execution_target` stays `DesktopLocal`. Set `ai_callable: true`. |
| 7 | `switch_workspace` | `navigateActions.ts` | NONE — sets Zustand `workspaceId` | `workspace.switch_workspace` (DesktopLocal, `arguments: vec![]`) | `workspace_id: WorkspaceId (req)` ← NEW arg | `{ success, switchedTo }` | navigation | `Safe` | `[]` | EXTEND existing command: add `workspace_id` arg. `execution_target` stays `DesktopLocal`. Set `ai_callable: true`. |
| 8 | `open_settings` | `navigateActions.ts` | NONE — sets main surface to `{ kind: "settings" }` | `workspace.open_settings` (DesktopLocal, `arguments: vec![]`) | (none) | `{ success, opened: "settings" }` | navigation | `Safe` | `[]` | METADATA FLIP only on existing command. Set `ai_callable: true`, add `nl_description` + intent_samples. No new args. |
| 9 | `send_message` | `writeActions.ts` | `sendMessage(input)` mutation | `chat.post_message` (Backend, full args match: `server_id`, `conversation_id`, `text`) | (already complete) | (already complete; backend mutation returns `{ id, sentAt }`) | mutating | `Mutating` | `["chat.write"]` (already set) | METADATA FLIP only. Set `ai_callable: true`, add `nl_description` + intent_samples. Sensitivity `Safe → Mutating`. `dangerous: false` (per Phase-4 escalation rules in §3.5; same-turn tool_result triggers confirmation, not the tier alone). Verify `builtin_executor.rs` handles `chat.post_message` end-to-end and returns the right payload shape. |
| 10 | `pin_message` | `writeActions.ts` | `pinMessage(input)` mutation | **none** | `server_id: WorkspaceId (req)`, `conversation_id: ConversationId (req)`, `message_id: MessageId (req)` | `{ success, pinned: <message_id> }` | mutating | `Mutating` | `["chat.write"]` | NEW `chat.pin_message`. Add executor wiring that calls `pin_service`. |
| 11 | `create_channel` | `writeActions.ts` | `createConversation({ ..., conversationType: "Channel" })` mutation | `chat.create_channel` (Backend; args: `server_id`, `name` — but FE also passes optional `description`) | `server_id: WorkspaceId (req)`, `name: String (req)`, `description: String (opt)` ← extend with optional `description` | `{ success, channelId, name }` | mutating | `Mutating` | `["chat.write"]` (already set) | METADATA FLIP + arg extension. Add optional `description` arg to existing command. Set `ai_callable: true`. Sensitivity `Safe → Mutating`. |
| 12 | `add_reaction` | `writeActions.ts` | `toggleReaction(input)` mutation | **none** | `server_id: WorkspaceId (req)`, `conversation_id: ConversationId (req)`, `message_id: MessageId (req)`, `emoji: String (req)` | `{ success, reacted: <message_id>, emoji }` | mutating | `Mutating` | `["chat.write"]` | NEW `chat.add_reaction`. Note: FE `toggleReaction` is bidirectional (add or remove); naming this `add_reaction` may mislead the LLM into thinking it always adds. Recommend either renaming to `chat.toggle_reaction` and updating nl_description, or splitting into add/remove. Defer naming call to backend lead. |

---

## NL Descriptions + Intent Samples

Reusable across `nl_description` (one-liner) and `intent_samples` (3–4 each).
All copy is sanitized — no embedded `<system>` / `<tool_use>` / "ignore previous"
patterns per Phase 1 §3.1 M3/M4.

### 1. `chat.list_members` (NEW)
- nl: "List all members of a workspace with their usernames and roles."
- intent_samples:
  - "who's in this workspace"
  - "list everyone in the team"
  - "show all members"
  - "who can I message here"

### 2. `chat.search_messages` (NEW)
- nl: "Search workspace messages by text content. Use to find specific past messages or topics."
- intent_samples:
  - "find messages about the launch"
  - "search for 'auth migration'"
  - "did anyone mention the bug from Tuesday"
  - "find what jane said about deploys"

### 3. `chat.get_recent_messages` (NEW)
- nl: "Read the most recent messages in a channel. Use for summaries, catching up, or quoting back what someone said."
- intent_samples:
  - "summarize what's been said in #engineering today"
  - "what did the team discuss this morning"
  - "catch me up on the design channel"
  - "read the last 50 messages in here"

### 4. `agent.get_current_context` (KEEP HARDCODED)
- nl: "Get the user's current view: which workspace and conversation they are looking at, and whether a thread is open."
- intent_samples:
  - "what am I looking at"
  - "where am I"
  - "what's my current channel"
- (No need for backend — keep TS handler.)

### 5. `workspace.navigate_to_channel` (METADATA + ARG)
- nl: "Switch the user's view to a specific channel. Use when the user asks to go to or open a channel."
- intent_samples:
  - "open #general"
  - "switch to the design channel"
  - "go to engineering"
  - "take me to random"

### 6. `chat.open_thread` (METADATA + ARG)
- nl: "Open the thread panel for a specific message. Use when the user asks to see or expand a thread."
- intent_samples:
  - "open the thread for that message"
  - "show me the thread on tony's announcement"
  - "expand this thread"

### 7. `workspace.switch_workspace` (METADATA + ARG)
- nl: "Switch the user to a different workspace. Use when the user asks to change workspaces."
- intent_samples:
  - "switch to the demo workspace"
  - "open my other workspace"
  - "go to the customer workspace"

### 8. `workspace.open_settings` (METADATA ONLY)
- nl: "Open the application settings page."
- intent_samples:
  - "open settings"
  - "show me preferences"
  - "go to settings"

### 9. `chat.post_message` (METADATA — FORMERLY `send_message`)
- nl: "Send a message in a channel or conversation on the user's behalf. Only use when the user explicitly asks you to post or send."
- intent_samples:
  - "send 'hey, running 5 late' to #ops"
  - "post in #general that the deploy is done"
  - "reply to jane saying yes"
  - "tell #design the mockups are merged"

### 10. `chat.pin_message` (NEW)
- nl: "Pin a message in a channel so it appears in the channel's pinned list."
- intent_samples:
  - "pin that message"
  - "pin the announcement"
  - "save this message to pinned"

### 11. `chat.create_channel` (METADATA + OPTIONAL ARG)
- nl: "Create a new channel in the workspace. Use when the user asks to create a new channel or start a new topic."
- intent_samples:
  - "make a channel for the migration project"
  - "create #incident-2026-04-24"
  - "start a new channel called design-reviews"

### 12. `chat.add_reaction` (NEW; or `chat.toggle_reaction` — see flag in matrix)
- nl: "Add an emoji reaction to a message."
- intent_samples:
  - "react to that with thumbs up"
  - "add a 🎉 to jane's message"
  - "react :+1: to the announcement"

---

## Migration Categorization Summary

| Category | Count | Tools |
|---|---|---|
| Metadata flip only (existing backend, args match) | 2 | `chat.post_message` (#9), `workspace.open_settings` (#8) |
| Metadata flip + arg extension on existing | 4 | `chat.create_channel` (#11), `workspace.navigate_to_channel` (#5), `chat.open_thread` (#6), `workspace.switch_workspace` (#7) |
| New backend command | 5 | `chat.list_members` (#1), `chat.search_messages` (#2), `chat.get_recent_messages` (#3), `chat.pin_message` (#10), `chat.add_reaction` (#12) |
| Stay hardcoded (no backend value) | 1 | `agent.get_current_context` (#4) |
| **Total** | **12** | (matches scope; `chat.list_channels` already shipped P3) |

---

## Open Questions for Tony / Fury

1. **Sensitivity enum mismatch.** Brief used 5-tier `Safe/Notable/Standard/Mutating/Dangerous`; backend has 4-tier `Safe/Sensitive/Mutating/Restricted`. Confirm which model Phase 4 ships against. If 5-tier is intended, Phase 4 needs an enum migration first.
2. **`chat.read` capability.** None of the read tools require any cap today. Spec proposes adding `chat.read` (Phase 1 strict default-deny would block all reads otherwise once `required_capabilities` is non-empty). Verify it's already seeded in `builtin_capabilities()` or add as part of Phase 4.
3. **`add_reaction` vs `toggle_reaction` naming.** Backend mutation is `toggleReaction`. Calling the agent tool `add_reaction` could mislead the LLM. Strange's call.
4. **`get_current_context` carve-out.** Confirm it's OK to keep one hardcoded builtin for context reads. The audit-trail value of routing it through backend is near-zero (no auditable side effect; payload is purely client state).
5. **`workspace.navigate_to_channel` execution_target.** Currently `DesktopLocal`. For agent calls from web (no Tauri), the dispatcher needs a fallback. Confirm whether AI surface always implies desktop or whether web-agent needs a separate command path.
6. **DataLoader for `search_messages` + `get_recent_messages` author resolution.** FE today has a `memberResolver` workaround. Backend should expose `author { ... }` via DataLoader so the FE workaround dies on migration. Verify `MessageGql` already has nested `author` resolver (from `rim_api/src/lib.rs:38` it does). Just confirm Phase 4 backend executor selects it.

---

## Recommended Dispatch Shape (P4-A4..P4-A13)

Based on the above, propose 10 sub-tasks (not 12 — collapse the trivial metadata flips):

- **P4-A4** — Metadata flips on existing 2 (#8, #9). One PR.
- **P4-A5** — Metadata flip + arg extension on `workspace.navigate_to_channel` (#5).
- **P4-A6** — Metadata flip + arg extension on `chat.open_thread` (#6).
- **P4-A7** — Metadata flip + arg extension on `workspace.switch_workspace` (#7).
- **P4-A8** — Metadata flip + arg extension on `chat.create_channel` (#11).
- **P4-A9** — NEW `chat.list_members` (#1) + executor wiring.
- **P4-A10** — NEW `chat.search_messages` (#2) + executor + DataLoader confirm.
- **P4-A11** — NEW `chat.get_recent_messages` (#3) + executor + DataLoader confirm.
- **P4-A12** — NEW `chat.pin_message` (#10) + executor.
- **P4-A13** — NEW `chat.add_reaction` (#12) + executor + naming decision.

**P4-FE deletion task** (separate from above): once N–1 backends ship, delete the corresponding TS builtins; keep `getCurrentContext.ts` per #4 carve-out. Remove `memberResolver.ts` once #2 + #3 land with backend-resolved authors.

— Spider-Man, P4-A2 audit, 2026-04-24
