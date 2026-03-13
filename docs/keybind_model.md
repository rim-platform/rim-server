# Keybind Model

This document defines the Phase 0 keybind model for RIM.

The keybind system is one of the core differentiators of the product. Keybinds should make both
built-in and extended workflows fast to trigger without becoming a separate scripting system.

## Purpose

A keybind is a user-configurable mapping from a key combination to a command invocation.

Keybinds should:

- make important actions fast to access
- support built-in commands, plugin commands, and user-defined commands
- work consistently across communication, workflow, and navigation surfaces
- stay simple, predictable, and safe

## Core Rule

- `keybind -> command`
- not `keybind -> arbitrary custom code`

Commands own behavior. Keybinds only trigger commands.

This keeps:

- command logic centralized
- plugin behavior compatible with built-ins
- macros bindable
- complex behavior reusable from other invocation surfaces

## What A Keybind Can Target

A keybind may target any command marked `keybind-capable`.

That includes:

- built-in commands
- plugin commands
- integration commands
- user-defined commands
- composed commands/macros

Not every command has to be keybind-capable.

## Keybind-Capable Commands

Commands should explicitly declare whether they support keybind invocation.

Why this matters:

- some commands are too interactive
- some only make sense in the command palette
- some are dangerous
- some require context that direct key invocation may not always provide

## Scopes

Recommended scopes:

- `default`
  - shipped by RIM or suggested by a plugin
- `user`
  - user-wide preference
- `workspace`
  - shared or team-managed override
- `device_local`
  - machine-specific override

## Scope Precedence

Recommended precedence:

- `device_local` > `workspace` > `user` > `default`

This allows:

- machine-specific shortcuts
- shared workspace conventions
- personal customization
- fallback defaults

## Contexts

Recommended initial contexts:

- `global`
- `composer`
- `message_list`
- `thread_view`
- `conversation_view`
- `panel`
- `command_palette`

Likely later contexts:

- `call`
- `voice_channel`
- `screen_share`

## Context Resolution Rule

More specific context wins over less specific context.

Examples:

- `Enter` in `composer` -> send message
- `Enter` in `message_list` -> open selected item
- `Enter` in `command_palette` -> run highlighted command

## Conflict Resolution

At runtime, only one binding should win.

Recommended resolution order:

1. more specific context wins
2. higher-precedence scope wins
3. explicit user/workspace override wins over defaults
4. unresolved same-priority conflicts should be shown as configuration errors

## User-Created Keybinds

Users should be able to create their own keybinds for:

- built-in commands
- plugin commands
- integration commands
- user-defined commands/macros

Examples:

- `Ctrl+Shift+G` -> `chat.create_group`
- `Ctrl+Y` -> `integration.youtrack.create_issue`
- `Ctrl+Alt+K` -> `plugin.k8s-tools.open_cluster_panel`
- `Ctrl+Shift+R` -> `user.clone_repo_and_open`

## Stateful / Repeated Keybind Behavior

Repeated behavior belongs to the command, not the keybind itself.

Example:

- keybind: `Ctrl+R`
- command: `workspace.cycle_recent_activity`

Behavior:

- first press -> latest recent activity/chat
- second press -> second latest
- third press -> continue cycling

This keeps:

- keybinds simple
- command runtime free to keep temporary UI/session state

## Dangerous Commands

Dangerous commands may still be bindable.

Examples:

- local process execution
- destructive actions
- commands with external side effects

Rule:

- dangerous local commands should still show confirmation by default, even when invoked by keybind
- users may disable the confirmation explicitly later

## Plugin Interaction

Plugins should be able to:

- register commands
- mark commands as keybind-capable
- suggest default keybinds
- suggest recommended contexts

But:

- plugins should not silently override existing user choices
- plugin-suggested keybinds should be shown as suggestions for approval, not auto-applied
- user/workspace/device overrides always win

## Workspace Keybinds

Decision:

- `workspace` keybinds should be optional shared or team-managed bindings

This allows teams to establish shared shortcuts without making workspace-level bindings mandatory.

## Multiple Bindings

Decision:

- one command may have multiple keybindings in the same context

This supports:

- user preference
- migration from defaults
- alternative access patterns

## Persistence Model

Recommended persistence strategy:

- default keybinds -> code or plugin manifest
- user keybinds -> persisted
- workspace keybinds -> persisted
- device-local keybinds -> local storage only

In the current backend shape:

- the backend stores `user` and `workspace` keybinds
- the backend exposes `default` keybind suggestions as a separate layer, not as persisted keybind
  records
- the client/runtime is expected to own `device_local` keybinds and final runtime keypress handling

The backend may expose:

- `default` keybind suggestions
- stored persisted keybinds by scope
- resolved persisted keybinds
- single-key persisted resolution for a given `(context, keystroke)`
- keybind layer inspection for a given `(context, keystroke)`, so the client can understand which
  backend layers contributed before applying any device-local override

The client/runtime should then own the final effective merge:

- `device_local > workspace > user > default`

That final runtime overlay is intentionally a client concern, not a persisted backend concern.

This matches the wider customization model:

- runtime-composed registry
- persistence for user/team/local customization only

## Minimal Keybind Record

A keybind definition should likely contain:

- `id`
- `keystroke`
- `command_id`
- `scope`
- `context`
- optional `preset_args`
- `enabled`
- `source`

Possible later addition:

- `when_clause`

That should only be added if explicit contexts prove insufficient.

## Non-Goals For Phase 0

The keybind system should not become:

- a scripting language
- a second automation engine
- a privilege bypass
- a place for complex workflow logic

If behavior is complex:

- make it a command
- then bind the command

## Summary Of Decisions

- keybinds invoke commands only
- only `keybind-capable` commands can be bound
- scopes:
  - default
  - user
  - workspace
  - device_local
- precedence:
  - device_local > workspace > user > default
- contexts are explicit and more specific wins
- repeated/cycling behavior belongs to commands, not keybind definitions
- users can create their own keybinds for built-in/plugin/user-defined commands
- dangerous commands still require confirmation by default
- plugins can suggest defaults, but suggestions require approval
- workspace keybinds are optional shared/team-managed bindings
- one command may have multiple keybindings in the same context
- resolved persisted precedence is currently `workspace > user`, while `device_local` remains a
  future client-side layer on top of backend-resolved data
