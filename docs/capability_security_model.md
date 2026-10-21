# Capability And Security Model

This page records the design direction for capabilities. Its example manifest and capability
categories name capabilities the plugin host refuses, such as `local.process`, `local.filesystem`,
`http.outbound`, `action.invoke` and `event.emit`. The names a manifest may declare are in
[`specs/plugin-protocol-v1.md`, section 12](specs/plugin-protocol-v1.md#12-capabilities) and
rim-ui's [plugin manifest reference](../../rim-ui/docs/plugin-manifest.md).

This document defines the capability and security model for RIM.

RIM is a chat app on steroids, but plugins, commands, integrations, and local behaviors must not
turn into a hidden privilege surface. Capabilities are the core language for controlling what
extensions and extension-provided commands are allowed to do.

## Purpose

A capability is an explicit permission that allows a plugin, integration, command provider, or
future app principal to access a sensitive product surface or perform a sensitive action.

Capabilities are intended to make extension behavior:

- explicit
- reviewable
- grantable
- revocable
- auditable later

## Core Rule

Capabilities are explicit, not implicit.

Nothing should gain sensitive power just because it exists in the system. Sensitive access must be:

- declared
- granted
- visible

## Who Uses Capabilities

Capabilities should primarily apply to extension-like actors and providers, including:

- plugins
- integrations
- app principals
- bot principals
- webhook principals

Users still have product permissions and membership rules, but extension capabilities are a separate
security layer.

## Capability Grant Model

Recommended model:

- the provider declares requested capabilities
- the user or admin grants them explicitly
- the granted state is persisted
- the granted state can be revoked later

Example:

- a plugin declares:
  - `ui.panel`
  - `chat.read`
  - `local.process`
- the user sees an install/enable prompt
- the user grants or denies capabilities
- the final granted state becomes the effective permission set

## Capability Categories

The initial model should use a small set of broad, understandable capabilities.

### Communication

- `chat.read`
- `chat.write`

### Workflow

- `action.invoke`
- `event.emit`

### UI

- `ui.panel`

Later candidates may include:

- `ui.context_action`
- `ui.composer_extension`

### Local System

- `local.process`
- `local.filesystem`

Later candidates may include:

- `local.notifications`
- `local.window_control`

### Network / External Access

- `http.outbound`

### Settings

There is no `settings.*` capability. A plugin's settings are its own data, like its `rim.log`
lines and its `rim.storage` store: the host gives a plugin only the settings its manifest
declares, picks the plugin from the connection rather than from the message, and only the user
writes them, so there is nothing for a grant to guard. A capability would add a prompt line that
protects nothing, and such a line teaches people to approve prompt lines without reading them.
The contract is
[`specs/plugin-protocol-v1.md`, section 15](specs/plugin-protocol-v1.md#15-settings).

## Decisions

- `http.outbound` starts as one broad capability.
- `chat.read` and `chat.write` stay broad for now.
- later tightening is expected once the platform hardens.

## Dangerous Capabilities

Some capabilities should be considered dangerous from the beginning.

Initial dangerous capabilities:

- `local.process`
- `local.filesystem`
- broad `http.outbound`

Dangerous capability rules:

- they must be visibly marked
- they should get stronger install/enable prompts
- dangerous actions may still require per-action confirmation later
- they should be easy to revoke

## Command Capability Inheritance

Commands should not invent permissions for themselves.

Recommended rule:

- commands inherit capabilities from their provider/source

This means:

- built-in commands use built-in trusted behavior
- plugin commands use plugin-granted capabilities only
- integration commands use integration-granted capabilities only
- user-defined commands/macros cannot exceed the capabilities of the commands they compose plus any
  explicit local permissions granted by the user

This rule is required to avoid privilege escalation.

## User-Defined Commands And Macros

User-defined commands should remain powerful, but they must not bypass capability checks.

Rules:

- user-defined commands may compose other commands
- user-defined commands may run local processes on desktop only if the dangerous local capability
  model allows it
- user-defined commands cannot gain more power than the capabilities of the commands or dangerous
  local permissions they use

## Dangerous Local Execution

User-defined and plugin-provided local process execution is allowed in the product direction already
agreed earlier.

Rules:

- local process execution requires explicit dangerous capability
- execution should show confirmation by default
- the user may later disable confirmation explicitly
- the UI should show the origin of the action:
  - built-in
  - plugin
  - integration
  - user-defined command

## Partial Capability Grant

Recommended direction:

- partial grants are desirable if the implementation can support them cleanly
- first version may fall back to full-set approval if needed

This keeps the early implementation realistic without giving up a better long-term direction.

## Admin Restriction Direction

Recommended direction:

- workspace admins should eventually be able to restrict dangerous capabilities regardless of user
  preference

This is especially important for:

- `local.process`
- `local.filesystem`
- broad external network access

Admin policy enforcement over capabilities does not exist yet; designs should leave room for it.

## Command Execution Gate Order

`execute_command` checks, in order: C1 capability (every required capability, workspace
membership included) → H1 confirmation (dangerous commands) → T1 timeout (a
timed-out actor cannot run `Mutating`/`Restricted` commands in that workspace) → R1 rate limit.
Every rejection is audited. T1 also runs on nested dispatch (user-command steps, pipe stages).
Workspace roles (Owner, Moderator, Member, Guest) are not capabilities: they are enforced by the services the commands call (`docs/messaging.md`, Roles
And Moderation), so a command can never do more than the same action through GraphQL.

## Trust Boundaries

RIM should make trust boundaries visible.

Recommended trust groups:

- `built_in`
- `user_defined`
- `plugin`
- `integration`

Later, bot/app/webhook principals should also become explicit trust-bearing identities.

Trust boundaries should be visible in:

- installation prompts
- dangerous action confirmations
- debugging and logging surfaces
- future audit/event records

## Visibility Rules

Capability-sensitive behavior should never be hidden.

The UI should make it clear:

- which provider owns the action
- which dangerous capabilities are involved
- whether the action is local or remote
- whether the action may modify external state

## Revocation Rules

Granted capabilities should be revocable later.

Revocation should disable or limit:

- plugin commands that require those capabilities
- panels or automations that depend on those capabilities
- integration behaviors that require those capabilities

The system should fail clearly, not silently, when revoked capabilities make an extension partially
unusable.

## Persistence Model

Recommended persistence split:

- capability declarations -> code/plugin/integration manifest source of truth
- granted capabilities -> persisted
- dangerous confirmation preferences -> persisted where appropriate
- policy restrictions -> later persisted once policy/admin controls exist

## Non-Goals

This model does not define:

- a complete policy engine
- fully general sandbox DSLs
- delegated impersonation flows

Per-origin network grants for web plugins (`network.fetch`) are defined in
`docs/specs/web-plugins-v1.md`. Plugin access to encrypted conversations (`chat.read.encrypted`,
`chat.write.encrypted`) is defined in `docs/specs/e2ee-mls-v1.md`.

## Summary Of Decisions

- capabilities are explicit permissions for sensitive product surfaces and actions
- extensions and future non-user principals are the main capability holders
- `http.outbound` is broad in the first version
- `chat.read` and `chat.write` stay broad in the first version
- dangerous capabilities include at least `local.process`, `local.filesystem`, and broad
  `http.outbound`
- commands inherit capabilities from their provider
- user-defined commands/macros cannot exceed what they compose plus explicitly granted dangerous
  local behavior
- partial grant is desirable, but full-set approval is acceptable for the first version
- admin restriction of dangerous capabilities is expected later
- trust boundaries must be visible to users
- plugin settings need no capability: a plugin reads only its own, and only the user writes them
