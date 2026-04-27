# AI Agent Reframe — Architecture Design

**Author:** Nick Fury (Software Architect)
**Date:** 2026-04-22
**Audience:** Tony (CTO), Black Widow (security review), Ant-Man (UX), Kuba (owner — morning approval)
**Inputs:** Falcon's audit `docs/design/ai-agent-audit-2026-04-22.md` (ground truth), `CLAUDE.md` (architectural rules), `ROADMAP.md` lines 445-468 (priority order).
**Status:** DRAFT — pending Black Widow security pass (Task #11) and Kuba sign-off.

> *"I recognize the council has made a decision. But given that it's a stupid decision, I've elected to ignore it." — keep two parallel registries, get two parallel bug surfaces. We unify.*

---

## 1. Goal

The AI Agent stops carrying its own tool registry. It becomes a **thin LLM driver** on top of the existing `CommandService`.

Concretely:

- One registry: `CommandService::list_commands(surface = AI)` is the **only** source of truth for what the agent can do.
- One execution path: agent dispatches through `commandDispatcher.execute(...)` exactly like the palette does — local handlers stay local, plugin commands route to plugins, backend commands go through `ExecuteCommandCmd` with `confirmed_dangerous`.
- One permission model: a real `CapabilityService::check(actor, capability)` gates every execution, agent-initiated or not.
- One confirmation surface: dangerous commands trigger the **same modal** whether they originate from the palette, a keybind, a macro, or the agent.
- One mental model for users and plugin authors: ship a command, get palette + keybind + context-menu + AI exposure for free (opt-in via `ai_callable` flag).

The reframe is not a new runtime. It is a **deletion** — `agent/tools/builtins/*` goes away — plus three small additions on the backend (DTO fields, AI surface variant, real capability check) and one shared confirmation modal on the frontend.

---

## 2. Current vs Target Architecture

### 2.1 Current (from Falcon's audit)

```
┌──────────────────────────────────────────────────────────────────┐
│                          rim-ui (Tauri)                          │
│                                                                  │
│  ┌──────────────────────┐         ┌──────────────────────────┐   │
│  │  Agent Overlay       │         │  Command Palette         │   │
│  │  (Ctrl+Shift+A)      │         │  (Ctrl+P / Ctrl+Shift+P) │   │
│  └──────────┬───────────┘         └────────────┬─────────────┘   │
│             │                                   │                 │
│             ▼                                   ▼                 │
│  ┌──────────────────────┐         ┌──────────────────────────┐   │
│  │ agentEngine.ts       │         │ commandDispatcher.ts     │   │
│  │ getAllToolDefinitions│         │  - localHandlers (zustand)│  │
│  │ window.confirm()     │         │  - plugin invoke         │   │
│  │ MAX_TOOL_ROUNDS=10   │         │  - executeBackend (gql)  │   │
│  └──────────┬───────────┘         └────────────┬─────────────┘   │
│             │                                   │                 │
│             ▼                                   │                 │
│  ┌──────────────────────┐                      │                 │
│  │ tools/registry.ts    │                      │                 │
│  │ Map<name,            │                      │                 │
│  │     RegisteredTool>  │                      │                 │
│  │ 13 hand-written      │                      │                 │
│  │ TS handlers          │                      │                 │
│  └──────────┬───────────┘                      │                 │
│             │ (direct GraphQL, bypass dispatch) │                 │
└─────────────┼──────────────────────────────────┼─────────────────┘
              │                                   │
              ▼                                   ▼
  ┌──────────────────────────────────────────────────────────────┐
  │                   rim-server GraphQL /api                    │
  │                                                              │
  │  sendMessage  pinMessage  createChannel  ...                 │
  │  executeCommand(ExecuteCommandCmd)                           │
  │     └─> CommandService                                       │
  │           ├─> builtin_executor (chat.* / workspace.* / ...)  │
  │           ├─> plugin_executor (PLUGIN source_kind)           │
  │           └─> user_commands (recursive, MAX_DEPTH=8)         │
  │                                                              │
  │  CapabilityService::list_capabilities  (placeholder, G2)     │
  └──────────────────────────────────────────────────────────────┘

Problems (G1–G8 from audit):
  G1 duplicate tool source       G2 no real capability check
  G3 two confirmation paths      G4 no NL metadata on commands
  G5 no AI surface tag           G6 UserCommand vs MacroDef both store steps_json
  G7 no audit trail              G8 local commands invisible to agent
```

### 2.2 Target

```
┌──────────────────────────────────────────────────────────────────┐
│                          rim-ui (Tauri)                          │
│                                                                  │
│  ┌──────────────────────┐         ┌──────────────────────────┐   │
│  │  Agent Overlay       │         │  Command Palette         │   │
│  └──────────┬───────────┘         └────────────┬─────────────┘   │
│             │                                   │                 │
│             ▼                                   ▼                 │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │              commandDispatcher.execute()                    │ │
│  │  (single execution path: local | plugin | backend)          │ │
│  │     ↑                                                       │ │
│  │     │ both surfaces use SAME dispatcher                     │ │
│  └─────┴───────────────────────────────────────────────────────┘ │
│             ▲                                   ▲                 │
│             │                                   │                 │
│  ┌──────────┴───────────┐         ┌────────────┴─────────────┐   │
│  │ agentEngine.ts (slim)│         │ useCommands(PALETTE)     │   │
│  │  useCommands(AI) →   │         │ useCommands(CONTEXT_MENU)│   │
│  │  AiToolDefinition[]  │         │ useCommands(KEYBIND)     │   │
│  │  (projection)        │         │                          │   │
│  └──────────┬───────────┘         └──────────────────────────┘   │
│             │                                                     │
│             │  on dangerous command → uses ↓                      │
│             ▼                                                     │
│  ┌─────────────────────────────────────────────────────────────┐ │
│  │      ConfirmCommandModal (shared across all surfaces)       │ │
│  │  Shows title, description, resolved args, dangerous badge,  │ │
│  │  capability list. "Confirm" / "Deny" / "Confirm + remember" │ │
│  └─────────────────────────────────────────────────────────────┘ │
└──────────────────────────────────────────────────────────────────┘
              │
              ▼
  ┌──────────────────────────────────────────────────────────────┐
  │                   rim-server GraphQL /api                    │
  │                                                              │
  │  executeCommand(ExecuteCommandCmd { ..., initiator: AGENT }) │
  │     └─> CommandService                                       │
  │           ├─> CapabilityService::check (REAL, post-G2 fix)   │
  │           ├─> dangerous gate → ConfirmationRequired          │
  │           ├─> builtin / plugin / user_commands dispatch      │
  │           └─> AuditLogService::record(actor, cmd, args, ...) │
  │                                                              │
  │  CommandDefinitionDto + new fields:                          │
  │     ai_callable, nl_description, intent_samples, sensitivity │
  │                                                              │
  │  CommandSurface { Palette, ContextMenu, KeybindPicker, Ai }  │
  └──────────────────────────────────────────────────────────────┘
```

The agent becomes a *consumer* of the same registry the palette uses. Tool execution flows through the same dispatcher, the same capability check, and the same dangerous-action modal.

---

## 3. Concrete Unification Design

### 3.1 Schema extensions on `CommandDefinitionDto`

Three additions to `rim_services/src/command/dto.rs:68-86`. All optional, backward compatible — existing commands without these fields simply don't appear on the AI surface.

```rust
pub struct CommandDefinitionDto {
    // ... existing fields unchanged ...

    /// Verbose, LLM-friendly description. Falls back to `description` if None.
    /// Should describe *intent* and *side effects* in natural language.
    pub nl_description: Option<String>,

    /// Sample user phrases that should route to this command.
    /// Used both for intent ranking (Phase 4+) and for prompt-time
    /// few-shot examples in the agent's tool description.
    pub intent_samples: Vec<String>,

    /// Whether this command is exposed on the AI surface.
    /// Defaults to false — opt-in. Plugin/user-command authors set this
    /// explicitly. Built-ins curated case by case.
    pub ai_callable: bool,

    /// Sensitivity classification, orthogonal to `dangerous`.
    /// `dangerous` = irreversible side effects (always confirms).
    /// `sensitivity` = whether the agent should auto-execute or request
    /// confirmation even for non-dangerous read actions
    /// (e.g., search_messages might be Sensitive in a private channel).
    pub sensitivity: CommandSensitivityDto,
}

pub enum CommandSensitivityDto {
    /// Read-only, public, safe to auto-execute. Most chat.scroll_*, list_*.
    Safe,
    /// Reads private data or makes minor non-destructive writes.
    /// Agent may execute without confirmation but result is shown verbatim
    /// (so the user sees what was read). E.g., search_messages.
    Sensitive,
    /// Mutates state. Always confirms in the agent flow even if not
    /// `dangerous`. E.g., send_message (non-dangerous, but user must approve
    /// any message the agent posts in their name).
    Mutating,
    /// `dangerous` is implicitly `Mutating`; this variant is for the
    /// rare "destructive-read" case (export_all_messages → triggers DLP).
    Restricted,
}
```

**Why two flags (`dangerous` + `sensitivity`) and not just one:** `dangerous` is an objective property of the command itself (deleting a message is dangerous regardless of caller). `sensitivity` is a *channel-aware* policy for AI initiation specifically — sending a message is not dangerous in the abstract, but a user must approve every message the agent puts under their name. The palette path already shows the user typing, so it doesn't need `Mutating` confirmation. The agent path does.

### 3.2 AI surface tag

Add a fourth variant to `CommandSurfaceDto` (rim_services/src/command/dto.rs:21-25):

```rust
pub enum CommandSurfaceDto {
    Palette,
    ContextMenu,
    KeybindPicker,
    Ai,   // NEW
}
```

`CommandService::list_commands(ListCommandsQuery { surface: Ai, ... })` returns only commands with `ai_callable = true` AND that pass the actor's capability check (so the agent never sees commands the user can't run anyway).

Frontend `CommandSurface` mirror: add `"AI"` to `rim-ui/src/features/commands/types.ts`. After the schema change, **re-export SDL and run `npm run codegen`** per `rim-ui/CLAUDE.md`. The generator emits `CommandSurface.Ai` matching backend casing.

### 3.3 Capability check contract — closing G2

`CapabilityService` (rim_services/src/capability/service.rs:10-12) gets a new method:

```rust
#[async_trait]
pub trait CapabilityService: Send + Sync {
    async fn list_capabilities(&self, actor: Actor) -> ServiceResult<Vec<CapabilityDto>>;

    /// Returns Ok(()) if actor holds the capability, Err(PermissionDenied) otherwise.
    /// Used by CommandService::execute_command before any dispatch.
    async fn check(&self, actor: &Actor, capability: &str) -> ServiceResult<()>;

    /// Bulk variant for filtering AI surface — avoids N+1 when listing 100s of cmds.
    async fn check_many(
        &self,
        actor: &Actor,
        capabilities: &[String],
    ) -> ServiceResult<HashMap<String, bool>>;
}
```

**Wiring point (single call site):** `rim_infra_services/src/command.rs::execute_command` — at the top of the function, before any executor dispatch:

```text
for cap in &command_def.required_capabilities {
    capability_service.check(&cmd.actor, cap).await?;
}
```

This matches the layered architecture rule from `rim-server/CLAUDE.md` (services orchestrate; the check lives in `rim_services`/`rim_infra_services`, not in the executor or the GraphQL resolver). Same call covers palette, agent, macro, and keybind initiators — exactly the goal.

`MockCapabilityService` already exists via `automock`. Stub backend (Phase 1): "actor holds all capabilities by default" — keeps existing behavior identical until real ACLs ship. The shape lands now; the policy tightens later (out of scope for this reframe but unblocks Black Widow's enforcement work).

### 3.4 Tool-schema generator — `CommandDefinitionDto` → `AiToolDefinition`

Pure, frontend-side, deterministic. Lives at `rim-ui/src/features/agent/runtime/toolSchema.ts` (new file).

Mapping rules:

| `CommandArgumentTypeDto` | JSON Schema `type` | JSON Schema `format` (custom) |
|---|---|---|
| String, SelectionText | `"string"` | — |
| Integer | `"integer"` | — |
| Float | `"number"` | — |
| Boolean | `"boolean"` | — |
| Enum | `"string"` | — (use `enum` array from `allowed_values`) |
| UserId | `"string"` | `"rim-user-id"` |
| ConversationId | `"string"` | `"rim-conversation-id"` |
| MessageId | `"string"` | `"rim-message-id"` |
| ThreadId | `"string"` | `"rim-thread-id"` |
| WorkspaceId | `"string"` | `"rim-workspace-id"` |

Generator output:

```ts
function commandToTool(def: CommandDefinition): AiToolDefinition {
  return {
    name: def.id,                             // "chat.post_message"
    description: buildToolDescription(def),   // nl_description + intent_samples + danger note
    input_schema: {
      type: "object",
      properties: Object.fromEntries(
        def.arguments.map(a => [a.name, argToJsonSchema(a)])
      ),
      required: def.arguments.filter(a => a.required).map(a => a.name),
      additionalProperties: false,
    },
  };
}
```

`buildToolDescription` concatenates: `nl_description ?? description`, plus a one-line "Examples: 'phrase 1'; 'phrase 2'" footer derived from `intent_samples` (capped at 3 to control token usage). If `dangerous` or `sensitivity = Mutating`, append "REQUIRES USER CONFIRMATION." so the LLM knows to surface intent before calling.

### 3.5 Dangerous flow — shared modal

UX is Ant-Man's call (Task #10 in flight); architectural constraints from this side:

1. **Single component:** `rim-ui/src/shared/ui/ConfirmCommandModal.tsx` — used by palette, context-menu, keybind-picker, AND agent. Props: `{ command: CommandDefinition, resolvedArgs: Record<string, unknown>, initiator: "palette" | "agent" | "keybind" | "macro", onConfirm, onDeny }`.
2. **Resolved args, not raw:** modal renders args after `CommandContext` interpolation (so `{{selected_message.text}}` shows the actual message body the user is about to act on, not the placeholder). This is the audit-trail anchor — what the user sees is exactly what executes.
3. **One source of truth for the dangerous gate:** the *backend* `ExecuteCommandCmd { confirmed_dangerous: bool }` flag stays the authority. Frontend modal returns `confirmed_dangerous: true`; if any layer skips the modal, the backend rejects with `ServiceError::ConfirmationRequired`. **Frontend cannot bypass.**
4. **"Remember for this session" is per-command-id, not blanket:** allows the user to grant a single recurring action without writing a blank check (Black Widow's likely concern — call out for security review).

### 3.6 Args validation strategy

LLM-generated args land at three validation gates, in order:

1. **JSON Schema validation (frontend, pre-dispatch):** `ajv` validates `args` against the generated `input_schema` before `commandDispatcher.execute` is called. Schema mismatch → tool result `"Error: invalid args (schema): missing required field 'channel_id'"` fed back to the LLM, which retries within `MAX_TOOL_ROUNDS`. This is the **fast feedback loop** — the agent learns and self-corrects without a server roundtrip.

2. **Typed-ID well-formedness (frontend, in dispatcher):** for `UserId` / `ConversationId` / `MessageId`, verify the value is a UUIDv7-shaped string. Catches LLM hallucinations like `"user_alice"` early. Cheap, deterministic.

3. **Authoritative validation (backend, in `CommandService`):** the existing `builtin_executor` + service layer continues to be the source of truth. Schema-passing args may still violate domain rules (e.g., conversation_id refers to a workspace the actor isn't a member of). Backend rejects with `ServiceError`, dispatcher surfaces the error to the agent transcript, agent retries or asks the user.

**No new validation logic on the backend** — existing `*Cmd` validations already do this. The reframe just guarantees the agent path hits them, instead of bypassing them via direct GraphQL.

### 3.7 Pipeline composition

The agent gets pipelining for free. Two complementary mechanisms:

- **Native LLM tool-use loop:** the LLM already chains tool calls within `MAX_TOOL_ROUNDS = 10` (rim-ui/src/features/agent/runtime/agentEngine.ts:10). Round 1: `search_messages` → result. Round 2: based on result, call `pin_message`. This is already pipelining — just dynamic and LLM-driven.

- **Explicit pipe via existing command pipe parser:** `rim_infra_services/src/command.rs` already parses `cmd1 | cmd2` syntax with `MAX_RECURSION_DEPTH = 8`. We expose a single AI-callable command `pipeline.run` whose argument is `{ steps: [{ command_id, args }, ...] }`. The LLM can choose to bundle multi-step intents into one call when the pipeline is known up front (cheaper than 3 round-trips).

**Recommendation: ship native loop only in Phase 3.** Add `pipeline.run` in Phase 4 once we have telemetry on actual multi-step usage patterns. Premature exposure burns tokens describing a feature the LLM may not need.

### 3.8 G6 verdict — UserCommand vs MacroDef

**Falcon flagged this. My call: formalize the split, do NOT consolidate. Reasoning:**

| | `UserCommand` (CommandService) | `MacroDef` (MacroService) |
|---|---|---|
| Identity | `command_key` (`"user.daily_standup"`) | `name` (`"daily standup"`) |
| Surface | Palette + keybind + context menu + AI | NL routing layer (intent → macro by name) |
| Composition rules | Recursive into other commands (depth 8) | No nesting (`MAX_STEPS = 20`, no macro-in-macro) |
| Ownership | User-level, per-server unique key | Per-user, per-server, NL-friendly |
| Use case | "I want a keybind for X" | "I want to say 'standup' to RIM and have it post" |

**They are not duplicates. They are two intent surfaces for the same underlying primitive (`steps_json` execution).** The ROADMAP's P2 — "Skill Macros + intent routing" — is *exactly* the MacroDef productization. UserCommand is the palette/keybind productization.

What I'd change to make the split crisp:

1. **Both surface as `CommandDefinitionDto`** at the AI layer, but differently:
   - `UserCommand`: `source_kind = User`, `ai_callable = false` by default (user opts in via UI toggle).
   - `MacroDef`: `source_kind = User`, `ai_callable = true` always (because the *whole point* of a Skill Macro is NL invocation). Exposed via a **new** `MacroService` adapter that synthesizes a `CommandDefinitionDto` per macro using `MacroDef.name` → `id`, `MacroDef.description` → `nl_description`.

2. **Macros never appear in the palette.** Their surface is "AI + autocomplete in composer" (the latter already shipped per ROADMAP line 405-407). Keeps the palette uncluttered.

3. **Future consolidation if the split fails to differentiate:** if 80% of users end up creating both a UserCommand and a MacroDef for the same logic, we have evidence to merge. Until then, the split serves real workflows. **Don't unify what users distinguish.**

This decision is **load-bearing** for Phase 3 of the migration plan below.

---

## 4. Migration Path (Zero-Downtime)

Each phase ships green, behind no flag. Each phase is independently revertable. No flag day.

### Phase 1 — Backend metadata extensions (S, ~3-4 days)

**Goal:** `CommandDefinitionDto` carries the new fields; nothing consumes them yet.

- Add `nl_description`, `intent_samples`, `ai_callable`, `sensitivity` to `CommandDefinitionDto`. All default-safe (None / empty / false / Safe).
- Add `Ai` variant to `CommandSurfaceDto`.
- `list_commands(surface = Ai)` returns empty array initially (no built-in has `ai_callable = true` yet).
- `CapabilityService::check` and `check_many` shipped as no-op-allow stubs (returns Ok always; Black Widow's later work tightens). Guarded by `#[deprecated(note = "stub — replace before exposing AI surface to plugins")]` so we cannot forget.
- Wire `CapabilityService::check` into `execute_command` per §3.3.
- SDL re-exported, frontend codegen run, types updated. Frontend builds green with new optional fields ignored.

**Backward compat:** existing palette/keybind/context-menu surfaces unchanged. Old GraphQL clients receive new fields as nullable/empty — no breakage.

**Definition of done:** `cargo test --workspace` green; `npm run build` green; `/api/sdl` reflects new schema; coverage gate (95% line) holds.

### Phase 2 — Tool-schema generator + agent dual-mode (S, ~3 days)

**Goal:** the agent can build tool defs from the AI surface, but it still defaults to the legacy hardcoded registry. Toggle is a feature flag *for development only*, removed at end of Phase 4.

- Build `rim-ui/src/features/agent/runtime/toolSchema.ts` per §3.4.
- Wire `useCommands(surface = "AI")` into `agentEngine.ts`.
- New code path: `getAllToolDefinitions()` becomes `getAgentTools()` which merges (a) legacy hardcoded tools, (b) generated tools from the AI surface. Dedupe by name — generated wins on conflict.
- Initially the AI surface is empty so behavior is identical to today.
- Add `pipeline.run` skeleton (commented out, not yet routed).

**Backward compat:** the agent overlay's behavior is byte-identical until commands start opting into `ai_callable`.

**Definition of done:** unit tests for generator (every `CommandArgumentTypeDto` variant maps correctly); integration test with a fixture command shows up as `AiToolDefinition` in the agent's tool list.

### Phase 3 — Migrate one safe read-only tool (S, ~1-2 days)

**Goal:** prove the end-to-end loop. Pick the safest tool: `list_channels` (read-only, no PII risk beyond what the user already sees).

- Mark `chat.list_channels` (or equivalent built-in command) with `ai_callable = true`, `sensitivity = Safe`, populate `nl_description` and 3 `intent_samples`.
- Delete `rim-ui/src/features/agent/tools/builtins/listChannels.ts`. Agent now invokes the command via the dispatcher.
- Add an end-to-end test: agent prompt "what channels do I have?" → tool call → result rendered.
- Compare round-trip vs legacy implementation; target: equal or better latency, fewer LOC.

**Backward compat:** the legacy hardcoded `list_channels` is gone; the agent now uses the registry. Same user-visible result.

**Definition of done:** e2e test passes; `agent/tools/builtins/listChannels.ts` deleted; LOC reduction visible in PR diff.

### Phase 4 — Migrate remaining tools + shared confirmation modal (M, ~5-7 days)

**Goal:** all 13 hardcoded tools migrated. `agent/tools/builtins/` directory deleted entirely. Confirmation modal shared across surfaces.

Order (safest → most sensitive):

1. **Read tools first:** `get_recent_messages`, `search_messages`, `list_members`, `get_current_context`. All `Safe` or `Sensitive`.
2. **Navigation tools:** `navigate_to_channel`, `open_thread`, `switch_workspace`, `open_settings`. These are local commands — exercise the dispatcher's `localHandlers` path from the agent. **This closes G8.** All `Safe`.
3. **Mutating writes:** `send_message`, `pin_message`, `add_reaction`, `create_channel`. All `Mutating` (some `dangerous`). Ship the shared `ConfirmCommandModal` here — concurrent UI work coordinated with Ant-Man (Task #10).
4. **Delete `rim-ui/src/features/agent/tools/builtins/` directory** and `tools/registry.ts`. `agent/tools/` now contains only `types.ts` (still needed for the AiToolDefinition wire format) and a thin module re-exporting the generator.

**Backward compat at each step:** because dedupe-on-name "generated wins on conflict" logic shipped in Phase 2, each migrated tool seamlessly switches over. Worst case: a single-tool revert reverts that single tool's command entry.

**Definition of done:** `agent/tools/builtins/` is gone; no `window.confirm` calls remain in `agent/`; e2e tests cover one command per category (read, navigate, write, dangerous-write); `ConfirmCommandModal` snapshot tests for each `initiator` variant.

### Phase 5 — Hardening: audit trail + Skill Macros AI exposure + plugin surface (M, ~5-7 days)

**Goal:** close the remaining audit, intent-routing, and plugin gaps. This is where the reframe pays its second dividend.

- **Audit trail (closes G7):** every `execute_command` call writes a structured record `{ actor_id, command_id, args, context, initiator, dangerous, confirmed, result_status, ts }`. New service `AuditLogService` in `rim_services`. Adds `initiator: CommandInitiator { Palette, Agent, Macro, Keybind, ContextMenu, Plugin, Cli }` to `ExecuteCommandCmd`. Backend storage is a new `command_audit_log` table (SeaORM migration via `sea migrate generate`).
- **Skill Macros AI exposure:** `MacroService` ships a `synthesize_command_definitions` method that returns each `MacroDef` as a `CommandDefinitionDto { ai_callable: true, source_kind: User, sensitivity: Mutating }`. `CommandService::list_commands(Ai)` merges in the macro projection. Closes the P2 roadmap item plumbing.
- **Plugin surface:** plugin manifest gets new optional fields (`nl_description`, `intent_samples`, `ai_callable`). Plugin SDK docs updated. **Now plugin authors get AI exposure for free, just by setting one flag.** This is the strategic win the reframe was for.
- **Token-budget filter:** the AI surface query takes an optional `recently_used_first: bool` and `limit: u32`. Default agent behavior asks for the top 50 by recency + capability; the LLM uses an introspection meta-tool `command.discover(query: string)` to fetch additional commands on demand. (See §5 for context.)

**Definition of done:** audit log query exists, returns last N entries; one user macro callable from the agent in an e2e test; plugin example in `docs/plugin_model.md` shows the new fields; introspection meta-tool documented.

### Phase 6 — Cleanup (S, ~1-2 days)

- Remove the Phase 2 dev-only feature flag.
- Remove the `#[deprecated]` stub on `CapabilityService::check` (replaced by Black Widow's real implementation in the parallel security workstream).
- Update `docs/command_model.md` and `docs/agent_model.md` (the latter is new) with the unified picture.
- ROADMAP P1 → "Shipped".

---

## 5. Risks & Open Questions

### R1 — Token budget for many tools (Falcon E.5 #6, ROADMAP-acknowledged)

**The risk:** plugin marketplace + user commands + macros could put 200+ tools in front of the LLM. At ~150 tokens per tool description, that's 30k tokens of system prompt overhead before the user even types — burning latency and money.

**Mitigations, in priority order:**

1. **Capability-permitted filter.** Already in the design — the AI surface only returns commands the actor *can* execute. Cuts noise.
2. **Recency + frequency ranking.** `list_commands(Ai)` returns the top N most-recently-used by this actor + the always-callable starter set (search, navigate, send_message). Cap default at 50.
3. **Two-tier discovery via meta-tool.** Always expose one tool `command.discover(intent_phrase: string)` that returns a list of relevant command IDs based on `intent_samples` matching. The LLM can call this when it doesn't see what it needs in the initial set, then call the discovered command in the next round. **Trades one tool round for ~25k tokens of context — almost always worth it past 50 commands.**
4. **Deferred (Phase 7+):** embedding-based pre-filter on the server side, returning the top-K most-relevant commands for the user's prompt. This is the Slack-AI / Notion-AI approach. Worth it past 200 commands, not before.

**Open question:** at what command count do we flip from "send all" to "send 50 + meta-tool"? Recommend instrumentation in Phase 4 to measure actual prompt-token usage; switch heuristic when median user system-prompt > 20k tokens.

### R2 — `intent_samples` ranking (Falcon E.5 #6)

**The risk:** literal keyword match on `intent_samples` is fragile (user says "post a note" not "send a message"). Embeddings or LLM routing are powerful but add latency / cost / a vector-store dependency.

**Recommendation:** start with **literal-substring + token-overlap fuzzy match** in `command.discover`. It's free and good enough for the 5-50 command regime. Revisit when (a) plugin marketplace has 50+ AI-callable commands, or (b) telemetry shows discover() returning irrelevant results frequently. Embeddings is **not** in scope for this reframe — separate decision, separate spike.

### R3 — Backward compatibility for existing AI users

Per Falcon: ~zero today. The reframe is functionally a no-op for existing users at Phase 4 completion (same 13 tools, same prompts, same overlay). After Phase 5 they gain access to *more* commands without lifting a finger. **No migration burden on users.**

### R4 — Capability stub creates a security trust gap

**The risk:** Phase 1 ships `CapabilityService::check` as no-op-allow. If we expose the AI surface to plugins (Phase 5) before Black Widow tightens the policy, an unprivileged user could trigger a privileged plugin command via the agent.

**Mitigation:**
- Phase 5 plugin exposure **blocks on** Black Widow's real capability implementation. ADD this dependency to Task #11 explicitly.
- Until then, `ai_callable = true` is curated case-by-case for built-ins only; no plugin path to opt in until the gate is real.

### R5 — `commandDispatcher` becomes a single point of failure

The dispatcher is already the palette's execution path; making it the agent's path too concentrates blast radius. **Mitigation:** existing dispatcher test coverage is at the same level as the rest of the frontend; bring it to 100% as part of Phase 3 (small, scoped). Add a "dispatch tracer" debug toggle in the agent overlay — when enabled, logs each step of the dispatch decision tree to the transcript. Cheap, useful for both debugging and Black Widow's audit requirements.

### R6 — LLM hallucinated args bypassing UUID format check

Phase 3.6 step 2 catches well-formed-but-wrong UUIDs at the dispatcher. Phase 3.6 step 3 catches them at the backend. **But:** an LLM could pass `"00000000-0000-0000-0000-000000000000"` for a UserId, which is well-formed. Backend already rejects (no such user), but the error message returned to the agent should not leak whether the ID exists for a *different* actor than the current user. Falcon should flag this — likely already a backend convention but worth confirming during security review.

### R7 — Two implementations of "user-defined logic" (G6) — see §3.8

Decision made: **formalize the split**, do not consolidate. Risk: if usage shows both surfaces converging in user behavior, refactor cost in 6 months. Acceptable.

### Open Questions for the Council

1. **`pipeline.run` vs native loop in Phase 3 — ship both or one?** Recommendation: native loop only initially; pipeline command in Phase 5 contingent on telemetry.
2. **Audit log retention?** Compliance question, not architectural. Defer to Black Widow + Kuba.
3. **Per-command "always confirm" override?** User preference: "even if the agent thinks send_message is Mutating-with-confirm, I want EVERYTHING confirmed for this one command." Out of scope for v1, easy to add later (per-actor command_preferences table).
4. **Anthropic dangerous-direct-browser-access header (`agent/providers/anthropic.ts:46`).** Architectural smell — frontend hits LLM directly with user's API key. Black Widow's territory; flagged in Falcon's audit A.3. Reframe does not change this; long-term should consider a server-side proxy. Out of scope for this design.

---

## 6. Definition of Done (whole reframe)

The reframe is "shipped" when ALL of the following are true:

**Code:**
- `rim-ui/src/features/agent/tools/builtins/` does not exist.
- `rim-ui/src/features/agent/tools/registry.ts` does not exist.
- No `window.confirm()` call remains in `rim-ui/src/features/agent/`.
- `CapabilityService::check` is wired into `execute_command` in `rim_infra_services/src/command.rs` and called on every dispatch.
- `CommandSurfaceDto::Ai` exists and `list_commands(surface = Ai)` returns a non-empty set in any populated workspace.
- `ConfirmCommandModal` is the only confirmation UI for commands; rendered for both palette and agent dangerous flows.
- A plugin command with `ai_callable: true` is reachable from the agent in an e2e test.
- A user macro is reachable from the agent via NL phrase in an e2e test.

**Tests:**
- All existing tests green (350 backend + 692 frontend, per ROADMAP line 436-437).
- New: tool-schema generator unit tests, dispatcher-path-from-agent integration tests, capability-denied integration tests, dangerous-confirmation snapshot tests.
- Backend coverage gate (95% line, per CLAUDE.md) holds.

**Observable behaviors:**
- User opens agent overlay, asks "what channels do I have?" → response includes channel list. (Same behavior as today; proves the migration was invisible.)
- User asks "send 'lunch?' to #random" → ConfirmCommandModal appears with resolved args; on confirm, message posts; audit log entry written.
- User installs a plugin with an `ai_callable` command → command appears in palette AND agent surface without code change.
- User creates a Skill Macro named "standup" → asking the agent "do my standup" routes to the macro.

**Docs:**
- `docs/agent_model.md` (new) describes the unified architecture for plugin authors.
- `docs/command_model.md` updated with `ai_callable`, `nl_description`, `intent_samples`, `sensitivity`.
- `docs/plugin_model.md` includes one example AI-callable plugin command.
- `ROADMAP.md` P1 → "Shipped".

**Hawkeye review:** every PR per phase reviewed per `CLAUDE.md` mandate; no BLOCK findings outstanding.

---

## 7. Estimated Effort

Phase-by-phase, T-shirt sizes with reasoning. Total: **M-L overall (~3-4 weeks for one engineer, ~2 weeks with two engineers parallelizing frontend/backend).**

| Phase | Size | Calendar (1 eng) | Why |
|---|---|---|---|
| 1 — Backend metadata + capability shape | S | 3-4 days | DTO additions, single executor wire-in, SDL re-export, codegen. Mechanical, well-understood. Coverage gate adds ~half a day. |
| 2 — Generator + agent dual-mode | S | 3 days | Pure frontend, generator is deterministic, dual-mode merge is ~50 LOC. |
| 3 — One tool migrated end-to-end | S | 1-2 days | Most cost is the e2e test infra, not the migration itself. Pays for itself by validating the pipeline. |
| 4 — Remaining 12 tools + ConfirmCommandModal | M | 5-7 days | The bulk of the work. Each tool migration is ~30 min, but ConfirmCommandModal is real UX (coordinated with Ant-Man, Task #10), and dangerous-write flows need careful testing. |
| 5 — Audit log + Skill Macros AI + plugin surface | M | 5-7 days | New SeaORM migration, new service trait + impl, MacroService adapter, plugin SDK docs. Couples to Task #10 (Skill Macros UX) and Task #11 (real CapabilityService) — schedule serialization required. |
| 6 — Cleanup | S | 1-2 days | Flag removal, doc finalization. |
| **Total** | **M-L** | **~3-4 weeks 1-eng** | Realistic with parallelism: ~2 weeks (backend + frontend split). |

**Critical path dependencies:**
- Phase 5 audit log can parallel Phase 4. Don't serialize.
- Phase 5 plugin AI exposure **blocks on** Black Widow's real capability implementation (Task #11 outcome). Schedule accordingly.
- Phase 4 ConfirmCommandModal blocks on Ant-Man's UX spec (Task #10). Spec must be ready by end of Phase 3.

**What's NOT in this estimate:**
- Embedding-based intent routing (R2) — separate spike, separate decision.
- Server-side LLM proxy (replacing direct-browser Anthropic calls) — separate architectural decision driven by security review.
- Real `CapabilityService` policy implementation — Black Widow's deliverable.

---

## Closing

> *"I still believe in heroes. And I believe in single sources of truth."*

The reframe doesn't add a runtime — it removes one. The smallest viable change that gives us:
- One registry (Command Registry).
- One execution path (commandDispatcher).
- One permission model (CapabilityService::check).
- One confirmation surface (ConfirmCommandModal).
- One audit trail (AuditLogService).

Plugin authors ship a command; they get palette + keybind + context menu + AI for free. Users learn one mental model: "if it's in the palette, the agent can do it (when I let it)." That's the unification.

Ship it phased. Each phase green. No flag day. No regrets.

— Fury
