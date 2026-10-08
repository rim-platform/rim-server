# Plugin Model

This document defines the current plugin architecture direction for RIM.

Plugins are intended to be local, user-facing extensions similar in spirit to VS Code or JetBrains
extensions, with stronger security and brokering assumptions.

## Core Rule

Plugins are not backend-managed by default.

The first real plugin system should be:

- desktop-local
- user-local
- UI/runtime owned
- brokered through a local plugin host

The backend should expose stable APIs and command contracts that plugins can call through the local
host, but it should not own plugin installation or lifecycle in the first version.

## What A Plugin Is

A plugin is an installable local extension package that may contribute one or more extension points,
such as:

- commands
- keybind suggestions
- context actions
- panels
- settings
- local workflow helpers

## Ownership Model

### Local Plugin Host Owns

- plugin folder discovery
- manifest loading
- local install/enable/disable state
- capability approval prompts
- plugin runtime lifecycle
- command registration into the local command catalog
- local keybind suggestions
- local context actions
- local panels
- plugin settings

### Backend Owns

- shared application state
- auth/session validation
- messaging data
- command execution for backend commands
- search/reference APIs
- shared integrations, bots, and webhooks later

## Plugin Runtime

Plugins are standalone processes that communicate with the RIM plugin host via a language-agnostic
wire protocol. A plugin can be written in ANY language — Rust, Python, Go, JavaScript, C#, Java, or
anything else that can read stdin and write stdout.

The design is inspired by Neovim's msgpack-rpc model: one protocol, any language, bidirectional
communication, with the critical addition of capability-based security on every call.

### Wire Protocol

Plugin protocol v1 is frozen. The normative spec is
[`docs/specs/plugin-protocol-v1.md`](specs/plugin-protocol-v1.md); the machine-readable contract is
`rim-ui/protocol/plugin-protocol-v1.schema.json`.

- **Transport:** stdin/stdout; stderr is free-form logging
- **Framing:** 4-byte little-endian length prefix, then UTF-8 JSON, at most 4 MiB per frame
- **Format:** JSON-RPC 2.0 only (no MessagePack in v1)
- **Direction:** Bidirectional — host calls plugin, plugin calls host
- **People:** plugins see a person only as `{ principalId, displayName }`
- **Security:** Every plugin→host call is checked against approved capabilities, and dangerous
  commands run only after the user confirms each run

`docs/local_plugin_host.md` describes how the desktop host installs, starts and stops plugins and
hosts their panels.

### Plugin Types

Plugins may or may not have UI:

- **UI plugins** — register panels, contribute visual surfaces to the workspace
- **Headless plugins** — commands, automations, event handlers, integrations (no UI)
- **Hybrid plugins** — both commands/automations AND UI panels

The host treats all plugin types identically. The manifest declares what the plugin provides.

### Language Agnosticism

The plugin protocol is designed so that:

- A Python script can be a plugin (read stdin, parse JSON, write stdout)
- A compiled Rust binary can be a plugin
- No SDK is required — the protocol is simple enough to implement from scratch (the spec ends
  with a complete plugin in Python)
- A TypeScript SDK is planned in rim-ui (`sdk/`) to reduce boilerplate; other languages may follow

## Runtime Kind

The first real plugin system supports only:

- `DesktopLocalProcess` (the manifest's `runtime_kind`)

This means plugins are local extensions, not backend-hosted services.

## Manifest

Plugins should use a local manifest file, for example:

- `rim-plugin.json`

The manifest should define:

- plugin id
- display name
- version
- description
- runtime kind
- entrypoint
- requested capabilities
- declared extension points

## Plugin Id Format

Recommended internal format:

- reverse-domain style

Examples:

- `dev.example.k8s-tools`
- `com.company.youtrack-helper`

Commands contributed by the plugin should be namespaced under:

- `plugin.<plugin_id>.<command_name>`

## Installation Model

First version should support:

- local folder installation
- local manifest validation
- local enable/disable
- local uninstall
- local capability approval

The plugin host should keep local state for these flows.

Backend-managed plugin installation is not the target architecture for the first version.

## Capabilities

Capabilities are the plugin’s requested permissions over host-managed or backend-brokered features.

Examples:

- `chat.read`
- `chat.write`
- `ui.panel`
- `local.process`
- `local.filesystem`
- `http.outbound`

Protocol v1, section 12, says which capability names a manifest can declare today and which
host methods each built-in name guards.

Capabilities should be:

- explicit
- visible
- grantable
- revocable

## Security Model

The first real plugin system should not ship as an unrestricted local process model.

Manifest capabilities alone are not enough to secure a normal process. The intended direction is:

- local plugin host
- brokered access to sensitive operations
- capability checks at the host boundary
- later stronger sandboxing if needed

So the plugin host should mediate:

- backend API calls
- local process spawning
- local filesystem access
- outbound HTTP
- UI registration

## Relationship To Commands And Keybinds

Plugins should integrate into the shared systems rather than invent new parallel ones.

Plugins may:

- register commands
- mark commands as keybind-capable
- suggest keybinds
- expose context actions that invoke commands
- expose panels that invoke commands or backend APIs via the host

Plugins should not:

- create parallel command systems
- create parallel keybind systems
- bypass host capability checks
- directly own backend installation state

## First Plugin Version Scope

The first real plugin system should prioritize:

- desktop-local plugin host
- local manifest loading
- local command registration
- local capability approval
- brokered backend API access through the host

Commands should come first.

Panels, context actions, and keybind suggestions can follow on top of the same host/runtime model.

## Backend Relationship

Plugins may still need backend data and backend-side actions, but that should happen through the
host using normal authenticated app APIs.

The backend should be thought of as a stable API surface for plugins, not as the owner of plugin
installation in the first version.

## Plugin UI System

Plugins can optionally provide UI through a slot system. Plugins without UI (headless) are equally
supported — they register commands, event handlers, and automations without any visual surface.

### UI Slots

Direction, not implemented yet: today a manifest panel has only `id`, `title` and `description`,
and the host decides where it renders. The intended slots are:

| Slot | Location | Size | Use Cases |
|---|---|---|---|
| `tool_panel` | Left column (w-64) | 256px fixed width | Lists, browsers, navigation (Git changes, K8s pods) |
| `main_area` | Center (flex-1) | Full available width | Full-page views (Kanban, Dashboard, Code review) |
| `side_panel` | Right column (w-80) | 320px fixed width | Contextual panels (AI assistant, previews) |
| `overlay` | Floating modal | Configurable (sm/md/lg) | Dialogs, OAuth flows, settings |
| `message_action` | Inline in context menu | N/A (command-based) | Actions on messages (Translate, Create task) |

### Rendering Model

Plugin UI is rendered in a **sandboxed iframe** (`<iframe sandbox="allow-scripts">`). Plugins
cannot access host DOM, cookies, or tokens. Communication between plugin UI and host uses
`postMessage` with origin validation.

The host provides a **Theme Bridge** — CSS custom properties (rim-void, rim-panel, rim-accent, etc.)
are sent to the plugin iframe so it can match the host appearance. An optional Plugin UI SDK
provides pre-styled components.

### Two Communication Channels

1. **Backend channel:** Plugin process ↔ Tauri Rust host via stdin/stdout JSON-RPC.
   Used for: command execution, data access, backend API brokering.

2. **Frontend channel:** Plugin iframe ↔ Host UI via postMessage.
   Used for: UI rendering, navigation, notifications, theme sync.

Only the backend channel is frozen (plugin protocol v1). The frontend channel is not frozen yet;
`docs/local_plugin_host.md` lists which of its messages work today. A future SDK is meant to
abstract both channels behind a single API.

### Headless Plugins

Plugins without UI panels are fully supported:
- Register commands, context actions, keybinds
- Communicate only via the backend JSON-RPC channel
- Event subscriptions and automations triggered by events are planned; `rim.events.subscribe` is
  not part of protocol v1 yet
- Example: CI/CD bot, issue tracker integration, message auto-responder

### Process Startup

A plugin process starts when the user enables the plugin, and at app start for every plugin that
was enabled when the app last ran. Lazy start on first use and an idle stop are possible later
optimisations; neither exists in v1.

## Future Expansion

Possible later additions:

- backend-managed/shared plugin policy
- plugin sync across devices
- enterprise/admin controls
- plugin marketplace/registry
- stronger sandboxing/runtime isolation
- backend-hosted extension runtimes if a clear use case appears

## Summary

- plugins are local extensions first
- the first plugin architecture is client/runtime owned, not backend-managed
- the backend should provide stable APIs and command contracts for the plugin host to use
- commands are the first extension point to prioritize
- security should be enforced through a brokered host model, not trust in arbitrary local processes
