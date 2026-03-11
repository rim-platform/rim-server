# Plugin Model

This document defines the Phase 0 plugin model for RIM.

Plugins are one of the core differentiators of the product. They should be powerful enough to add
new workflow and communication surfaces, but safe enough to preserve user trust.

## Purpose

A plugin is an installable extension package that can register one or more extension points in RIM.

Plugins are the main way to make RIM customizable beyond the built-in product surface.

Examples:

- a Kubernetes panel
- a YouTrack helper
- custom commands
- context actions
- local automation behaviors

## Core Rule

A plugin is a package or container for extension points, not one specific feature type.

This means a plugin may provide one or more of:

- commands
- keybind suggestions
- context actions
- panels
- automations
- settings
- integrations
- local behaviors

## Runtime Kinds

RIM should distinguish plugin runtime kinds explicitly.

Potential runtime kinds:

- `desktop_local`
- `backend`
- `web_safe`
- `mobile_safe`

## Phase 0 Decision

The first real plugin system should support only:

- `desktop_local`

Reasoning:

- it matches the strongest product differentiation
- it enables local tools and workflows early
- it avoids overdesigning distributed plugin execution too soon

## Plugin Identity

Each plugin should have:

- stable plugin id
- display name
- version
- publisher or author
- runtime kind
- declared capabilities
- declared extension points
- optional settings schema
- optional minimum host version

## Plugin Id Format

Recommended internal format:

- reverse-domain style

Examples:

- `dev.example.k8s-tools`
- `com.company.youtrack-helper`

The UI should show a separate human-readable display name.

## Installation Model

The first plugin installation flow should support:

- install from local file
- install from local folder
- enable
- disable
- uninstall
- compatibility checks
- capability prompts before enabling

## Phase 0 Decision

The first plugin version should install from local file or folder only.

Later additions may include:

- install from URL
- plugin registry/marketplace
- signed packages

## Extension Points

The first serious plugin model should support these extension points.

### Commands

- Plugin may register commands.
- Plugin commands live in the shared command system.
- Recommended namespace pattern:
  - `plugin.<plugin_id>.<command_name>`

### Keybind Suggestions

- Plugin may suggest default keybinds for its commands.
- Suggested keybinds must require approval.
- Plugins must not silently override user-defined keybinds.

### Context Actions

- Plugin may contribute actions for selected text, messages, threads, channels, or other supported
  contexts.

## Phase 0 Decision

Plugin context actions should map to commands, not use a separate execution model.

This keeps the architecture unified:

- palette invokes commands
- keybinds invoke commands
- context actions invoke commands
- plugins register commands

### Panels

- Plugin may provide one or more UI panels.
- Panels are intended for rich tool surfaces such as Kubernetes tooling or dashboards.

## Phase 0 Decision

Panels should be supported in the first real plugin version.

### Automations

- Plugin may react to events later.
- Plugin may trigger commands or plugin-owned workflows.

For the first serious version, automations may exist as a declared extension point without the full
event engine being complete yet.

### Settings

- Plugin may expose its own settings schema.
- Settings should fit the broader product settings model.

### Integrations

- Plugin may wrap or surface integrations.
- Integrations are still a separate product concept, but plugins may provide local UX or command
  surfaces for them.

## Capabilities

Plugins must declare capabilities explicitly.

Examples:

- `chat.read`
- `chat.write`
- `action.invoke`
- `event.emit`
- `ui.panel`
- `local.process`
- `local.filesystem`
- `http.outbound`

## Capability Rules

- capabilities are explicit
- capabilities are visible to the user
- capabilities are granted or denied intentionally
- capabilities should be revocable later
- no implicit privilege escalation

## Dangerous Capabilities

Plugins may be allowed to run local processes in the first version.

## Phase 0 Decision

- local process execution is allowed
- only with explicit dangerous capability
- dangerous actions should follow the same confirmation model as dangerous commands

This is important for workflows like:

- git helpers
- Kubernetes tooling
- local script execution
- device-level automation

## Plugin Relationship To Commands And Keybinds

Plugins should integrate into the shared product systems.

Plugins may:

- register commands
- mark commands as keybind-capable
- suggest keybinds
- expose context actions that invoke commands
- expose panel UI that invokes commands

Plugins should not:

- create parallel command systems
- create parallel keybind systems
- bypass command/capability checks

## Enable/Disable Behavior

Disabling a plugin should disable all of its registered extension points.

That includes:

- commands
- keybind suggestions/defaults
- context actions
- panels
- local behaviors

The system should not leave stale active behavior behind once a plugin is disabled.

## Persistence Model

Recommended persistence split:

- plugin package and runtime definition -> file/folder source of truth
- installation record -> persisted
- enabled/disabled state -> persisted
- granted capabilities -> persisted
- plugin settings -> persisted
- plugin command definitions -> runtime-derived from the plugin, not stored as the primary source

This matches the broader approach already chosen for commands:

- runtime-composed registry
- persistence for install/config/customization state

## First Plugin Version Scope

The first real plugin system should support:

- desktop-local plugins only
- local file/folder installation
- plugin manifest
- commands
- keybind suggestions
- context actions
- panels
- settings
- explicit capabilities
- enable/disable/uninstall lifecycle

This is already strong enough for:

- Kubernetes panel plugin
- YouTrack helper plugin
- git helper commands
- local desktop automation helper

## Non-Goals For First Plugin Version

The initial plugin system should not try to solve:

- backend plugin hosting
- distributed plugin execution
- plugin marketplace/registry
- advanced cross-plugin dependency management
- unrestricted scripting
- full hot-reload complexity beyond basic developer iteration

## Summary Of Decisions

- a plugin is an extension package that may expose multiple extension points
- first real plugin system supports `desktop_local` plugins only
- plugins may run local processes with explicit dangerous capability
- panels are supported in the first real plugin version
- plugins install from local file/folder first
- plugin context actions should map to commands
- plugin commands/keybinds/actions/panels should integrate with shared product systems, not create
  parallel ones
