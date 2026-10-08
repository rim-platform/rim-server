# Command Model

This document defines the Phase 0 command model for RIM.

The command system is one of the core differentiators of the product. Commands are the backbone for:

- built-in workflows
- user-defined workflows
- keybinds
- plugin-provided actions
- context actions
- automations

## Purpose

A command is a named, invokable action with:

- a stable id
- declared arguments
- declared context requirements
- declared capabilities
- a defined execution target
- a structured result

Commands should be the common execution unit across the product.

This means:

- the command palette invokes commands
- keybinds invoke commands
- context actions invoke commands
- plugins register commands
- automations may invoke commands
- user-defined workflows are commands too

## Current Backend State

The current backend/contracts implementation already provides:

- built-in command definitions
- command listing and single-command lookup
- backend command execution
- persisted user-defined commands/macros
- user-defined command create/update/delete flows
- command context-aware filtering for palette/context-menu/keybind use cases

Current v1 constraint:

- persisted user-defined commands may only compose backend-executable commands
- desktop-local command composition belongs to a later client/runtime slice

## Product Requirements

The command system must be:

- easy to understand for end users
- easy to extend for plugin authors
- easy to wire into keybinds and context actions
- safe by default
- explicit about dangerous local behavior

## Command Identity

Every command must have a stable, namespaced id.

Examples:

- `chat.post_message`
- `chat.open_thread`
- `workspace.open_panel`
- `integration.youtrack.create_issue`
- `plugin.k8s.restart_pod`
- `user.clone_repo_and_open`

Namespacing rules:

- built-in commands should use product namespaces like `chat.*`, `workspace.*`, `call.*`
- integration commands should use `integration.<name>.*`
- plugin commands should use `plugin.<plugin_id>.*`
- user-defined commands should use `user.*`

## Command Kinds

RIM should support the following command sources.

### Built-in Commands

- Shipped by RIM.
- Stable and documented.
- Usually available in the command palette.

Channel builtins (Track 1.1, `docs/specs/track-1-1-private-channels.md`). Apart from the read-only
`chat.list_channels` (`Safe`, `chat.read`), all are `Backend`, `Mutating`, `chat.write`, palette-
and keybind-visible. `conversation_id`
falls back to the focused conversation (`context.conversation_id`). Authorization (membership,
private-channel visibility, Owner/Moderator) lives in `ConversationService`, so the agent can
never do more than the user it acts for.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `chat.list_channels` | `server_id` | yes | no | Read; each channel is `{id, title, conversation_type, is_private, is_archived, is_member}` (`is_member` is the caller's; member ids are never returned). Default filter: every visible, unarchived channel |
| `chat.create_channel` | `server_id`, `name`, `description?`, `is_private?` (default false), `member_ids?` | yes | no | Members are added right after creation; privacy cannot change later |
| `chat.join_channel` | `server_id`, `conversation_id` | yes | no | Public, unarchived channels only; idempotent |
| `chat.leave_conversation` | `server_id`, `conversation_id` | yes | no | Channels and groups, not DMs |
| `chat.invite_to_conversation` | `server_id`, `conversation_id`, `user_ids` | yes | no | Caller must be a member |
| `chat.archive_channel` | `server_id`, `conversation_id` | yes | no | Owner/Moderator |
| `chat.unarchive_channel` | `server_id`, `conversation_id` | yes | no | Owner/Moderator |
| `chat.remove_conversation_member` | `server_id`, `conversation_id`, `user_id` | no | yes | Owner/Moderator, or yourself; confirmation required |

Moderation builtins (Track 1.2, `docs/specs/track-1-2-roles-and-moderation.md`). All are
`Backend` and `Mutating`; the `admin.*` ones require `admin.manage_members`, the `chat.*` ones
`chat.write`. They call `ModerationService`, which enforces the roles (Owner/Moderator, who may
target whom) and writes the moderation audit row (`docs/messaging.md`, Roles And Moderation), so
a command execution is audited twice: the generic command row plus the moderation row.
`chat.delete_message` (context menu, selected message) goes through `MessageService` as before,
which now lets an Owner/Moderator delete other people's messages.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `admin.kick` | `server_id`, `user_id`, `reason?` | no | yes | Owner/Moderator; confirmation required |
| `admin.ban` | `server_id`, `user_id`, `reason?`, `expires_at?` (RFC 3339) | no | yes | Owner/Moderator; confirmation required |
| `admin.unban` | `server_id`, `user_id` | no | no | Owner/Moderator |
| `admin.timeout` | `server_id`, `user_id`, `minutes` (1..=43200), `reason?` | yes | no | Owner/Moderator |
| `admin.set_role` | `server_id`, `user_id`, `role` (`moderator`/`member`/`guest`) | no | yes | Owner: any but owner; Moderator: member/guest; confirmation required |
| `chat.set_slow_mode` | `server_id`, `conversation_id`, `seconds` (0..=21600) | yes | no | Owner/Moderator; channels only; `conversation_id` falls back to the focused conversation |
| `chat.report_message` | `reason` | yes | no | Context menu; requires `workspace`, `conversation` and `selected_message` context |

Notification builtins (Track 1.3, `docs/specs/track-1-3-notification-rules.md`). All are
`Backend`, `Mutating`, palette- and keybind-visible, AI-callable, in category `Notifications`,
and need **no capability**: like `chat.mark_as_read` they change only the caller's own state.
They call `NotificationService`, which requires workspace membership (and, for the level, read
access to the conversation). Being `Mutating`, they are refused while the caller is timed out
(T1); the GraphQL mutations are not.

| Id | Args | AI-callable | Dangerous | Notes |
|---|---|---|---|---|
| `notifications.set_level` | `server_id`, `conversation_id`, `level` (`all`/`mentions`/`none`/`default`) | yes | no | `default` clears the override; `conversation_id` falls back to the focused conversation. Payload `{conversation_id, level, effective_level}` |
| `notifications.dnd` | `server_id`, `mode` (`on`/`off`/`until`), `until?` (RFC 3339), `minutes?` (1..=10080) | yes | no | The manual do-not-disturb pause (separate from the daily window); `until` mode needs `until` or `minutes`. Payload `{paused, until}` |
| `notifications.mark_all_read` | `server_id` | yes | no | Marks every notification of the workspace read (not the conversations' unread counts). Payload `{marked}` |

General builtins made AI-callable in Track 4.1. None declares a new capability. The navigation
commands are `DesktopLocal`: the server refuses to execute them, so the desktop agent runs them
through the client's local handlers, and only when they are `Safe` and not dangerous (anything
stricter goes to `executeCommand`, which refuses it). The five writes were raised from `Safe` to
`Mutating` (ai-reframe review §8.3), which has three effects beyond the agent's confirmation
gate: a timed-out member can no longer run them through the command path (T1; the GraphQL
`markAsRead` mutation is not affected), server-side pipelines (initiator `Plugin`) can no longer
run them because nobody is there to confirm, and they count towards the agent's per-turn
mutating-call cap. Palette and keybind still run them in one step.

| Id | Args | Sensitivity | Agent confirmation | Notes |
|---|---|---|---|---|
| `workspace.open_search`, `workspace.search_in_channel`, `workspace.open_marketplace`, `workspace.toggle_sidebar`, `workspace.previous_channel`, `workspace.next_channel`, `workspace.next_unread_channel`, `workspace.close_panel`, `workspace.reopen_last_thread`, `chat.scroll_to_bottom`, `chat.scroll_to_top` | none | `Safe` | no | `DesktopLocal`, run by the client |
| `state.get` | `server_id`, `key`, `scope?` | `Safe` | no | Read |
| `state.list` | `server_id`, `prefix?`, `scope?`, `limit?` | `Safe` | no | Read |
| `state.set` | `server_id`, `key`, `value`, `scope?`, `ttl?` | `Mutating` | yes | |
| `state.incr` | `server_id`, `key`, `delta?`, `scope?` | `Mutating` | yes | |
| `state.delete` | `server_id`, `key`, `scope?` | `Mutating` | yes | |
| `chat.mark_as_read` | `server_id`, `conversation_id` | `Mutating` | yes | |
| `chat.create_group` | `server_id`, `member_ids`, `title?` | `Mutating` | yes | `member_ids` is a JSON array or one comma-separated string |

Data the agent sends to its model provider grows with this set: `state.get` and `state.list` return
stored values, including the caller's own `scope: user` entries, and nothing redacts secrets kept
in workspace state. Kept off the AI surface on purpose: `chat.edit_message`,
`chat.delete_message`, `chat.reply_to_selected_message` and `chat.copy_message_text` (they act on
the user's selected message, which the agent cannot supply), `admin.invite_member` (its result is
an invite token that would pass through the model provider), and `chat.mark_all_as_read`
(duplicates `notifications.mark_all_read`).

List arguments such as `user_ids` and `member_ids` accept a JSON array of ids or one
comma-separated string, because the agent's tool schema types every argument as a scalar.

AI-callable builtins: 45 (the 21 of Track 1.1, plus `admin.timeout`, `chat.set_slow_mode` and
`chat.report_message` from Track 1.2, plus the three `notifications.*` builtins of Track 1.3, plus
the eighteen general builtins of Track 4.1), pinned by
`collect_ai_callable_inventory_matches_reviewed_set` (`registry/mod.rs`) and
`list_commands_ai_surface_returns_forty_five_commands_with_caps` (`command_tests.rs`). The
capability union the AI surface checks is unchanged (`admin.manage_members`, `chat.read`,
`chat.write`), since neither the notification nor the Track 4.1 builtins declare any.

### Plugin-Provided Commands

- Registered by plugins.
- May be local-runtime or plugin-runtime specific.
- Must declare required capabilities.

### Integration-Provided Commands

- Registered by integrations such as bots, webhooks, or external tool connectors.
- Usually remote or backend-oriented.

### User-Defined Commands

- Created by the user.
- May wrap built-in, plugin, or integration commands.
- May compose multiple commands into a single workflow.

Current v1 backend constraint:

- persisted user-defined commands may only compose backend-executable commands
- desktop-local command composition belongs to a later client/runtime slice

## Macros

Macros are not a separate root command type.

Decision:

- a macro is a user-defined command composed from other commands

This keeps the model simpler:

- one command abstraction
- one invocation system
- one keybind target model

## Command Contract

Each command should expose the following metadata.

### Required Fields

- `id`
- `title`
- `description`
- `source_kind`
- `execution_target`
- `arguments`
- `required_context`
- `required_capabilities`

### Recommended Fields

- `visible_in_palette`
- `visible_in_context_menus`
- `visible_in_keybind_picker`
- `dangerous`
- `supports_preview`
- `supports_dry_run`
- `feature` — the optional workspace module the command belongs to (see below), or none

### `feature` (workspace feature gate, Track 0.4)

`CommandDefinitionDto.feature: Option<WorkspaceFeature>` (GraphQL `Command.feature:
WorkspaceFeature`, nullable) names the optional per-workspace module a command depends on.
Today the only feature is `incidents` (GraphQL `INCIDENTS`), set on `incident.ack`,
`incident.escalate`, `oncall.who` and `oncall.take_shift`. Every other builtin, every
user-defined command and every macro has `feature = None` and is always available.

- `list_commands` (and therefore `command_discover`, the AI surface and the keybind picker)
  drops a command whose feature is off for `context.workspace_id`, with a single feature lookup
  for the whole list. Without a workspace context, feature-gated commands are hidden: they
  cannot run without one.
- `execute_command` rejects a gated command with `ServiceError::FeatureDisabled` (GraphQL code
  `FEATURE_DISABLED`, extension `feature: "incidents"`) when the feature is off for the
  context's workspace or there is no workspace context. The gate (F1) runs after the capability
  and confirmation gates and before the rate limiter, inside the audit wrapper, so the
  rejection is audited and never consumes a rate-limit slot.
- Nested dispatch hits the same gate: a user-command step or a pipe stage that reaches a gated
  builtin fails with `FeatureDisabled`; a macro step goes through `execute_command` and fails
  the same way.

### Timeout gate (T1, Track 1.2)

Right after F1, `execute_command` refuses any command whose `sensitivity` is `Mutating` or
`Restricted` with `ServiceError::TimedOut { until }` (GraphQL `TIMED_OUT`, extension `until`)
when the actor is timed out in the context's workspace (`ModerationService::active_timeout`).
`Safe` and `Sensitive` commands still run. The gate also runs on nested dispatch (user-command
steps, pipe stages), before the rate limiter, inside the audit wrapper. Without a workspace
context it has nothing to check; the services behind the command apply their own write gates
anyway. Gate order: C1 capability → H1 confirmation → F1 feature → T1 timeout → R1 rate limit.

Feature state is managed per workspace by its Owner (`setWorkspaceFeature`); see
`docs/specs/track-0-4-optional-incident-module.md`.

## Arguments

Arguments should be typed from day one.

Initial recommended argument kinds:

- `string`
- `integer`
- `float`
- `boolean`
- `enum`
- `user_id`
- `conversation_id`
- `message_id`
- `thread_id`
- `workspace_id`
- `selection_text`

Each argument definition should include:

- `name`
- `title`
- `description`
- `type`
- `required`
- `default_value` if applicable
- `allowed_values` for enums

Arguments should be usable by:

- command palette forms
- plugin-provided UIs
- macros/user-defined command composition
- API surfaces later

## Context Model For Commands

Commands should explicitly declare required context.

Examples of context sources:

- selected text
- selected message
- selected thread
- current conversation
- current workspace
- active call/session
- current user/principal

This is important for:

- discoverability
- context menu filtering
- keybind eligibility
- plugin action registration

If required context is missing, the command must not be silently invoked.

## Invocation Sources

Commands should be invokable from multiple surfaces.

### Command Palette

- Main discovery surface.
- Should show only commands valid for the current context unless the user explicitly asks for all.

### Keybinds

- Keybinds should point to commands, not to custom one-off handlers.
- A keybind may provide fixed arguments or derive arguments from current context.

### Context Actions

- Text selection, message menus, thread actions, channel actions, etc.
- A context action should usually map to a command invocation.

### Plugin UI

- Buttons, panels, modals, or toolbars can invoke commands.

### Automations

- Event-driven or scheduled workflows may invoke commands later.

## Execution Targets

Every command should declare where it runs.

Initial execution targets:

- `backend`
- `desktop_local`
- `plugin_runtime`
- `integration_runtime`

Why this matters:

- security and capabilities differ per target
- availability differs by platform
- result handling and latency expectations differ

## Command Results

Every command should return a structured result envelope.

Decision:

- commands should not be modeled as only success/failure
- even simple commands should return a consistent result shape

Minimum result envelope should include:

- `status`
- `message` or summary
- optional structured payload
- optional follow-up actions

This supports:

- command palette feedback
- automation chaining
- plugin UI responses
- audit/event recording later

## Dangerous Local Commands

User-defined commands may run local OS processes on desktop.

Decision:

- allowed, but only with an explicit dangerous capability

Additional rule:

- local process execution should show a confirmation dialog by default
- the user may disable the confirmation later
- the safe default is confirmation enabled

This applies especially to commands such as:

- running shell commands
- launching local tools
- opening paths or files
- interacting with local system automation

The confirmation UX should clearly show:

- what command will run
- what runtime/capability is involved
- whether the command came from:
  - built-in logic
  - a plugin
  - an integration
  - a user-defined command

## Security Rules

Commands must inherit the security model of their provider.

Examples:

- built-in commands use built-in permissions
- plugin commands use plugin-declared capabilities
- integration commands use integration capabilities and tokens
- user-defined commands cannot exceed the capabilities of the commands they compose plus any local
  execution permissions explicitly granted by the user

Security rules:

- no implicit privilege escalation
- no hidden local process execution
- no hidden remote side effects
- dangerous commands must be marked clearly

## Availability Rules

A command may be:

- globally available
- available only in certain contexts
- available only on certain platforms
- available only when a required capability is granted
- available only when the workspace has the command's optional module (`feature`) turned on

Examples:

- `chat.post_message` may require conversation context or explicit arguments
- `plugin.k8s.restart_pod` may require desktop runtime and plugin capability approval
- `integration.youtrack.create_issue` may require integration installation and auth

## Example Command Shapes

### Built-in Command

- `id`: `chat.post_message`
- target: `backend`
- context: current conversation optional
- capabilities: `chat.write`

### Integration Command

- `id`: `integration.youtrack.create_issue`
- target: `integration_runtime`
- context: selected text or selected message optional
- capabilities: `action.invoke`, integration-specific external access

### Plugin Command

- `id`: `plugin.k8s.restart_pod`
- target: `plugin_runtime`
- context: selected pod or active panel context
- capabilities: plugin-declared runtime permissions

### User-Defined Command

- `id`: `user.clone_repo_and_open`
- target: `desktop_local`
- composed from:
  - `workspace.prompt_input`
  - local process execution
  - `workspace.open_folder`
- dangerous capability required because it runs a local process

## Non-Goals For Phase 0

This spec does not yet define:

- a full scripting language
- complex piping/dataflow semantics
- distributed workflow execution
- full audit schema
- full command-result typed payload protocol
- final UI for editing commands

Those can be layered on later once the base model is stable.

## Summary Of Decisions

- Commands are the common execution unit across palette, keybinds, context actions, plugins, and
  automations.
- Command ids are namespaced.
- User-defined commands may compose other commands.
- Macros are user-defined composed commands, not a separate root type.
- Commands use typed arguments.
- Commands declare required context.
- Commands declare execution target.
- Commands return structured result envelopes.
- User-defined local process execution is allowed on desktop only behind an explicit dangerous
  capability.
- Dangerous local commands should show a confirmation dialog by default, with an option to disable
  it later.
