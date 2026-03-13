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
