# AI Agent + Command Registry Audit (Recon Report)

**Author:** Falcon (Sam Wilson, recon)
**Date:** 2026-04-22
**Audience:** Nick Fury (Software Architect), Tony Stark (CTO), Kuba (owner)
**Purpose:** Foundation for the AI Agent reframe design (Task #9). Establishes the *current shape* of the AI Agent and Command Registry so unification can be designed against ground truth, not assumptions.

> "On your left." — Sam
> Bar: every claim cites `path:line`. Read it before designing. *Then* fly.

---

## Executive Summary

The AI Agent and the Command Registry are **two parallel systems that hit the same backend** through different doors. Today they share zero metadata, zero validation, and zero capability surface.

**Three load-bearing facts that drive the reframe:**

1. **The AI Agent lives entirely in the frontend.** No `rim_*_agent` Rust crate exists. The agent runtime is `rim-ui/src/features/agent/` — direct browser → Anthropic/OpenAI API calls, with tools registered in a frontend `Map<string, RegisteredTool>`. *(Implication: there is no server-side authority over what the agent can do.)*
2. **Tools are hardcoded TypeScript, not derived from the Command Registry.** All 13 agent tools (`send_message`, `pin_message`, `create_channel`, etc.) are hand-written handlers in `src/features/agent/tools/builtins/`. They re-implement what `CommandRegistry` already exposes — same GraphQL mutation, two paths.
3. **`CommandDefinitionDto` is already 80% of an AI tool schema.** Typed args, descriptions, `dangerous` flag, `required_capabilities`, `required_context` — all there. What's missing is *intent metadata* (`nl_description`, `intent_samples`, `ai_callable`) and a serverside capability check (`CapabilityService` is a placeholder).

**Bottom line for the reframe:** The work is *not* "build a new AI runtime." It's "make the existing Command Registry the single source of truth, expose it via an `ai_callable` projection, move the agent to consume that projection, and harden the dangerous-action confirmation path." Everything below justifies that claim.

---

## Part A — Current AI Agent Inventory

### A.1 Location & shape

The AI Agent is **frontend-only**. Glob for `agent*.rs` in `rim-server/` returns zero matches. All agent code lives at:

- `rim-ui/src/features/agent/` — runtime, providers, tools, state, components

### A.2 Runtime loop

`rim-ui/src/features/agent/runtime/agentEngine.ts:88-269` — `runAgent()`:

- Tool-call loop, **max 10 rounds** (`MAX_TOOL_ROUNDS`).
- Context window trimming for long conversations.
- Tools are fetched per-call via `getAllToolDefinitions()` from the agent's own registry — **not** from the backend command registry.
- `requiresConfirmation` triggers a synchronous `window.confirm()` (no rich UX, no batched approval, no audit trail).
- Errors swallowed into the assistant transcript ("Tool execution failed: …").

### A.3 Provider layer (direct browser → LLM)

- `rim-ui/src/features/agent/providers/anthropic.ts` — POSTs directly to `api.anthropic.com` from the browser using `anthropic-dangerous-direct-browser-access: true` header. API key sourced from Stronghold.
- `rim-ui/src/features/agent/providers/types.ts` — wire format is `AiToolDefinition { name, description, input_schema }`. **JSON Schema for input_schema** — directly compatible with what `CommandArgumentDto` could serialize to.
- `rim-ui/src/features/agent/runtime/keyStore.ts` — Stronghold per-installation password from OS keychain. Keys never localStorage.

### A.4 Tool registry (the current source of truth for "what the agent can do")

- `rim-ui/src/features/agent/tools/registry.ts` — in-memory `Map<string, RegisteredTool>` populated at startup. Singleton.
- `rim-ui/src/features/agent/tools/types.ts` — `RegisteredTool { definition, handler, sideEffect?, requiresConfirmation? }`. Handler signature: `(input: Record<string, unknown>) => Promise<string>` — returns a string for the LLM transcript, no structured result.
- `rim-ui/src/features/agent/tools/builtins/index.ts` registers **13 hardcoded tools**:
  - **Read (5):** `list_channels`, `list_members`, `search_messages`, `get_recent_messages`, `get_current_context`
  - **Action/navigation (4):** `navigate_to_channel`, `open_thread`, `switch_workspace`, `open_settings`
  - **Write (4):** `send_message`, `pin_message`, `create_channel`, `add_reaction`

### A.5 Critical: tools bypass the Command Registry

- `rim-ui/src/features/agent/tools/builtins/writeActions.ts:37-72` — agent's `send_message` tool calls `SEND_MESSAGE_MUTATION` GraphQL **directly**.
- The Command Registry's `chat.post_message` (rim_infra_services/src/command/registry.rs) ends up calling the same `SendMessageCmd` on the backend — **same destination, two distinct caller paths, no shared validation, no shared confirmation, no shared audit.**
- `rim-ui/src/features/agent/tools/builtins/listChannels.ts` — read tool, also direct GraphQL.

### A.6 State & UX

- `rim-ui/src/features/agent/state/useAgentStore.ts` — Zustand persist. Settings: `{ providerId, modelId, systemPrompt }`. Caps: max 20 conversations in memory, 5 persisted.
- `rim-ui/src/features/agent/components/AgentOverlay.tsx` — Ctrl+Shift+A invokes via `useAgentStore.open()`. Confirmation today is **`window.confirm()` only**.

---

## Part B — Current Command Registry Inventory

### B.1 Domain model

- `rim-server/rim_core/src/command/model.rs` — `UserCommand { command_key, title, description, steps_json }`. `is_user_command_key()` checks for `"user."` prefix. (Built-ins use namespaces like `chat.*`, `workspace.*`, `state.*`.)

### B.2 The unified DTO (the linchpin for the reframe)

`rim-server/rim_services/src/command/dto.rs:68-86`:

```rust
pub struct CommandDefinitionDto {
    pub id: String,
    pub title: String,
    pub description: String,
    pub category: String,
    pub source_kind: CommandSourceKindDto,
    pub execution_target: CommandExecutionTargetDto,
    pub arguments: Vec<CommandArgumentDto>,
    pub required_context: Vec<String>,
    pub required_capabilities: Vec<String>,
    pub visible_in_palette: bool,
    pub visible_in_context_menus: bool,
    pub keybind_capable: bool,
    pub dangerous: bool,
    pub user_command_id: Option<UserCommandId>,
}
```

**This already carries 80% of the metadata an AI tool schema needs.** Missing: `nl_description` (LLM-friendly verbose phrasing), `intent_samples` (NL phrases that route to this command), `ai_callable` (boolean projection flag).

### B.3 Typed arguments — direct JSON Schema mapping

`rim-server/rim_services/src/command/dto.rs:43-53` — `CommandArgumentDto` has typed argument kinds: `String`, `Integer`, `UserId`, `ConversationId`, `MessageId`, etc. These map cleanly to JSON Schema `type` + custom `format` for the agent's `input_schema`.

### B.4 Service contract

- `rim-server/rim_services/src/command/service.rs` — `CommandService` trait: `list_commands`, `get_command`, `execute_command`, `create_user_command`, `update_user_command`, `delete_user_command`.
- `rim-server/rim_services/src/command/request.rs:19-28` — `ExecuteCommandCmd { actor, command_id, args, context, confirmed_dangerous, is_pipe }`. **`confirmed_dangerous` flag already exists** — reframe can ride on it.

### B.5 Service implementation

`rim-server/rim_infra_services/src/command.rs` (1069 lines):

- Modules: `builtin_executor`, `pipe_parser`, `registry`, `user_commands`.
- `MAX_RECURSION_DEPTH = 8` for piped commands.
- Tests at lines ~800-1069 confirm user commands compose backend commands only (no nesting macros into macros via this path — that's the macro service's territory).
- **Capability check gap:** Comment in `execute_piped` says *"required_capabilities are NOT validated here because CapabilityService only exposes list_capabilities."* — confirmed by `rim-server/rim_services/src/capability.rs` which is a 2-line module declaration only.

### B.6 Built-in command count

`rim-server/rim_infra_services/src/command/registry.rs` (1122 lines) — **~37 built-in commands** enumerated:

- **chat.*** (~14): `post_message`, `create_group`, `reply_to_selected_message`, `delete_message`, `copy_message_text`, `open_thread`, `edit_message`, `create_channel`, `scroll_*`, `mark_as_read`, `mark_all_as_read`
- **workspace.*** (~18): `open_command_palette`, `cycle_recent_activity`, `open_settings`, `open_search`, `search_in_channel`, `open_marketplace`, `open_agent`, `toggle_sidebar`, `focus_*`, `navigate_*`, `switch_workspace`, `previous/next_channel`, `next_unread_channel`, `close_panel`, `reopen_last_thread`, `reload_commands`
- **state.*** (5): `get`, `set`, `delete`, `list`, `incr`
- **admin.*** (1): `invite_member`

### B.7 Built-in dispatcher

`rim-server/rim_infra_services/src/command/builtin_executor.rs:67-108` — `execute_builtin_command()` is a `match command_id { ... }` dispatcher. `chat.post_message` arm calls `services.message_service.send_message(SendMessageCmd { ... })` — **the same destination the agent's `send_message` tool reaches via direct GraphQL.**

---

## Part C — Macros & Pipelines

Two parallel "user-defined logic" systems exist on the backend.

### C.1 UserCommand (CommandService domain)

- `rim-server/rim_core/src/command/model.rs` — `UserCommand` persists a `steps_json` blob, namespace `"user.*"`.
- Created via `CommandService::create_user_command`.
- Composable into the same `execute_command` flow as built-ins.

### C.2 MacroDef (separate MacroService)

- `rim-server/rim_core/src/macro_def/model.rs` — `MacroDef { id, server_id, user_id, name, description: Option<String>, steps_json }`.
- Has an NL-friendly **`name`** and **`description`** — already shape-compatible with "Skill Macros" intent routing.
- `rim-server/rim_services/src/macro_def/request.rs:48-54` — `GetMacroByNameQuery { actor, server_id, name }` already exists. **Name → macro lookup is a free pre-built primitive for intent routing.**
- `rim-server/rim_infra_services/src/macro_def.rs` — `DefaultMacroService`. `MAX_STEPS = 20`, `MAX_MACROS_PER_USER = 50`. `validate_steps` rejects nested macros (no macro-in-macro recursion).

### C.3 Open question for Nick Fury

UserCommand vs MacroDef — both store `steps_json`, both compose. **Two implementations of the same idea.** The reframe should decide whether to consolidate or formalize the split (e.g., MacroDef = NL-routed, UserCommand = palette/keybind-routed). Current state is duplicative.

---

## Part D — Frontend Command Client (current wiring)

- `rim-ui/src/features/commands/api/useCommands.ts` — `useCommands(surface, commandContext)` query (TanStack Query); `useExecuteCommandMutation()` for backend execution.
- `rim-ui/src/features/commands/types.ts` — `CommandDefinition` mirrors the backend DTO. `CommandSurface = "PALETTE" | "CONTEXT_MENU" | "KEYBIND_PICKER"`.
- `rim-ui/src/features/commands/runtime/commandDispatcher.ts:87-275` — three-way dispatch:
  1. **`localHandlers` Record** — desktop-local commands (most `workspace.*`) dispatched directly to Zustand stores. Takes precedence even over backend marker.
  2. **Plugin commands** — Tauri `invoke("plugin_execute_command", ...)`.
  3. **Backend commands** — `executeBackend()` mutation against the `CommandService`.

**Implication for reframe:** The agent ideally goes through `commandDispatcher` (so local commands work without a server roundtrip) — *not* directly through the backend mutation. This means `CommandDispatcher` becomes the agent's tool execution path too, and the agent's tool registry becomes a *projection* of `useCommands(surface = "AI")` into AiToolDefinitions.

---

## Part E — Gap Analysis & Reframe Recommendations

### E.1 Gaps (ground truth)

| # | Gap | Location of evidence |
|---|---|---|
| G1 | **Tool source duplication.** Agent's tool registry (frontend Map) re-implements ~half the Command Registry. | `agent/tools/builtins/index.ts` vs `command/registry.rs` |
| G2 | **No server-side capability enforcement.** `CapabilityService` is a placeholder. | `rim_services/src/capability.rs` (2 lines) |
| G3 | **Two confirmation paths.** Agent uses `window.confirm()`; commands have `dangerous` flag + `confirmed_dangerous` but no shared modal. | `agentEngine.ts:88-269` vs `ExecuteCommandCmd` |
| G4 | **No NL metadata on commands.** `CommandDefinitionDto` has `description` but nothing tuned for LLM intent matching. | `rim_services/src/command/dto.rs:68-86` |
| G5 | **No `ai_callable` projection.** All commands either flow to palette/context-menu/keybind surfaces. There's no surface tag for "agent-callable." | `CommandSurface` enum in frontend, `visible_in_palette/visible_in_context_menus` in DTO |
| G6 | **Two user-logic systems.** UserCommand and MacroDef both store `steps_json`. | `rim_core/command/model.rs` + `rim_core/macro_def/model.rs` |
| G7 | **Agent has no audit trail.** Tool calls return strings to LLM transcript. No structured result, no log of what was executed against the workspace. | `RegisteredTool.handler` signature in `agent/tools/types.ts` |
| G8 | **Local commands invisible to agent.** Frontend `localHandlers` (e.g., `workspace.toggle_sidebar`) never reach the backend command registry, so the agent has no way to know they exist. | `commandDispatcher.ts:87-275` |

### E.2 Minimum viable unification (the small delta)

Three additions and one consolidation:

1. **Extend `CommandDefinitionDto`** with: `nl_description: Option<String>`, `intent_samples: Vec<String>`, `ai_callable: bool`.
2. **Add an `AI` variant** to `CommandSurface` so `useCommands(surface = "AI")` returns the agent-callable projection, mapped to `AiToolDefinition[]`.
3. **Implement `CapabilityService::check(actor, capability)`** as the gate for `execute_command` (closes G2; precondition for trusting agent-driven execution).
4. **Replace agent's `getAllToolDefinitions()`** with a query against the new `AI` surface; replace the per-tool handlers with a single `commandDispatcher.execute(commandId, args, context)` shim. Hardcoded builtins go away. (Closes G1, G7, G8.)

**Macros stay separate but get NL-promoted:** `MacroDef.name` + `description` make it a natural intent-routing layer. A user-created MacroDef can be exposed as an AI-callable command via the same projection (`ai_callable: true`).

### E.3 Confirmation UX (critical for the reframe)

Replace `window.confirm()` with a typed modal driven by `CommandDefinitionDto.dangerous`. Same modal serves agent-initiated and palette-initiated dangerous actions — single review surface, single audit point. (Closes G3.)

### E.4 Happy-path sketch — `youtrack.create_ticket`

End-to-end of how a user-defined integration command flows once unified:

```
1. PLUGIN INSTALL (one-time)
   YouTrack plugin installs → backend registers a command via CommandRegistry:
     {
       id: "youtrack.create_ticket",
       title: "Create YouTrack ticket",
       description: "Create a ticket in the linked YouTrack project",
       nl_description: "Create a YouTrack ticket from the current message or selection,
                        with title, description, assignee, and priority.",
       intent_samples: [
         "make a youtrack ticket for this",
         "file a bug for the issue alice mentioned",
         "create a ticket: ..."
       ],
       category: "integration",
       source_kind: PLUGIN,
       execution_target: PLUGIN_RPC,
       arguments: [
         { name: "title",       kind: String,   required: true,  context: SELECTED_MESSAGE_TEXT },
         { name: "description", kind: String,   required: false, context: SELECTED_MESSAGE_BODY },
         { name: "assignee",    kind: UserId,   required: false },
         { name: "priority",    kind: String,   required: false, default: "Normal" },
       ],
       required_context: ["selected_message"],
       required_capabilities: ["plugin.youtrack.write"],
       visible_in_palette: true,
       visible_in_context_menus: true,
       keybind_capable: true,
       dangerous: true,           // creating external tickets is irreversible-ish
       ai_callable: true,
       user_command_id: None,
     }

2. USER UTTERANCE (in agent overlay)
   "create a youtrack ticket for the bug @alice mentioned"

3. AGENT TURN
   Agent loads tools via useCommands(surface="AI") → receives ~N tool defs
   including youtrack.create_ticket as AiToolDefinition with JSON Schema
   derived from CommandArgumentDto[].

4. LLM CHOOSES TOOL
   Returns: tool_use { name: "youtrack.create_ticket",
                       input: { title: "<from selected msg>",
                                description: "...",
                                assignee: "<alice's UserId>",
                                priority: "High" } }

5. AGENT DISPATCHES VIA COMMAND DISPATCHER
   commandDispatcher.execute("youtrack.create_ticket", input, context)
   → context auto-fills selected_message from useSelectedMessage()
   → dispatcher sees execution_target=PLUGIN_RPC → routes to backend
     ExecuteCommandCmd { actor, command_id, args, context,
                         confirmed_dangerous: false }

6. BACKEND: CAPABILITY CHECK (new, post-G2 fix)
   CapabilityService::check(actor, "plugin.youtrack.write")
   → if missing: ServiceError::PermissionDenied → user sees inline error in agent

7. BACKEND: DANGEROUS GATE
   command.dangerous && !confirmed_dangerous
   → returns ServiceResult::ConfirmationRequired { command, args, context }

8. FRONTEND: CONFIRMATION MODAL (shared with palette path)
   Modal renders title, description, resolved args, dangerous badge.
   User clicks Confirm → re-dispatches with confirmed_dangerous: true.

9. BACKEND EXECUTES
   builtin_executor sees source_kind=PLUGIN → forwards to PluginExecutor
   over JSON-RPC → YouTrack plugin creates ticket → returns
   CommandResultDto { ok: true, output_json: { ticket_id, url } }

10. AGENT TRANSCRIPT
    Agent receives structured CommandResultDto, formats:
    "Created YouTrack ticket FOO-123: https://youtrack/.../FOO-123"
    Audit trail entry created (closes G7).
```

**Same flow for built-ins, plugins, user macros, and (with the small unification) the agent.** That's the design target.

### E.5 Suggested order of operations for Nick Fury's design

1. Decide UserCommand vs MacroDef consolidation (or formalize the split). [G6]
2. Specify the `CommandDefinitionDto` extensions and `AI` surface. [G4, G5]
3. Specify `CapabilityService::check` contract. [G2]
4. Specify shared confirmation modal contract (frontend). [G3]
5. Specify agent → `commandDispatcher` migration plan (delete `agent/tools/builtins/`). [G1, G7, G8]
6. Specify `intent_samples` ranking strategy (literal match vs embedding-based vs LLM-routed) — this is the one part where there's real design choice, not just plumbing.

---

## Appendix: Files surveyed (full list, for traceability)

**rim-server:**
- `rim_core/src/command/model.rs`
- `rim_core/src/macro_def/model.rs`
- `rim_services/src/command/dto.rs`
- `rim_services/src/command/service.rs`
- `rim_services/src/command/request.rs`
- `rim_services/src/macro_def/request.rs`
- `rim_services/src/capability.rs`
- `rim_infra_services/src/command.rs` (1069 lines)
- `rim_infra_services/src/command/registry.rs` (1122 lines)
- `rim_infra_services/src/command/builtin_executor.rs`
- `rim_infra_services/src/macro_def.rs`
- `ROADMAP.md`, `docs/vision.md`, `docs/competitive-analysis.md`,
  `docs/command_model.md`, `docs/plugin_model.md`, `.claude/status.md`

**rim-ui:**
- `src/features/agent/runtime/agentEngine.ts`
- `src/features/agent/runtime/keyStore.ts`
- `src/features/agent/providers/anthropic.ts`
- `src/features/agent/providers/types.ts`
- `src/features/agent/tools/registry.ts`
- `src/features/agent/tools/types.ts`
- `src/features/agent/tools/builtins/index.ts`
- `src/features/agent/tools/builtins/listChannels.ts`
- `src/features/agent/tools/builtins/writeActions.ts`
- `src/features/agent/state/useAgentStore.ts`
- `src/features/agent/components/AgentOverlay.tsx`
- `src/features/commands/types.ts`
- `src/features/commands/api/useCommands.ts`
- `src/features/commands/api/useUserCommands.ts`
- `src/features/commands/runtime/commandDispatcher.ts`
- `.claude/status.md`

---

*"You can't design the unification without knowing the actual current shape." Now you do. — Falcon*
