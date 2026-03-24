# Roadmap

RIM is a programmable communication workspace — not another chat app.

Chat is the communication medium, but the product is defined by how customizable and
programmable the workspace is. Commands, keybinds, macros, and plugins are not side
features; they are the core product. RIM should make it easy to communicate, act on
communication context, and build workflows that match how a team actually works.

No competing product combines a command palette, user-defined keybinds, chainable macro
commands, client-side plugins, and a non-Electron desktop runtime. RIM occupies a new
category: a programmable communication workspace for power users and technical teams.

Privacy and security are core product principles from the beginning. Optional end-to-end
encrypted messaging is a later dedicated phase, not a rushed add-on.

## Product Identity

See also: `docs/vision.md` for the full product vision and competitive positioning.
See also: `docs/architecture.md` for the current high-level system architecture.

- Programmable communication workspace, not a chat app with plugins bolted on.
- Communication is the foundation — commands, keybinds, macros, and plugins are the
  product.
- The user builds their own workflow instead of adapting to the platform.
- Keyboard-first: the user should be able to do everything without a mouse.
- Desktop-first: Tauri v2 for performance and native integration.
- Extensible by design: plugins extend the same systems as built-in features.

## Core Principles

- Communication first: chat must feel fast, modern, and pleasant.
- Customization first: plugins, commands, and keybinds must be first-class and easy to adopt.
- Context becomes action: selected text, messages, threads, channels, and workspace state should be
  usable as workflow input.
- Security by default: explicit capabilities, scoped identities, and clear trust boundaries.
- Extensible by design: developers should be able to replace repositories, services, APIs, and add
  plugins through stable contracts.
- Cross-platform over time: backend and desktop first, then web and mobile expansion.

## Product Pillars

### Communication

- Channels
- DMs
- Groups
- Threads
- Attachments
- Later: voice, video, and screen sharing

### Customization

- Commands
- Custom keybinds
- Macros / chained commands
- User-defined workflows

### Plugins

- Panels
- Commands
- Context actions
- Automations
- Integrations
- Local behaviors

### Security And Privacy

- Capability-based access
- Scoped app identities and tokens
- Clear local vs remote trust boundaries
- Safe extension model
- Optional E2EE later

## Extension Model

A plugin is an extension package that can provide one or more surfaces or behaviors.

Examples:
- a Kubernetes panel
- a custom command
- a context action on selected text
- an automation reacting to events
- a local helper changing desktop/window behavior
- a webhook or bot bridge

Plugins may contribute:
- UI panels
- commands
- context actions
- automations
- integrations
- local system behaviors where allowed

Built-in commands and keybinds should exist from day one. Plugins should extend the same systems,
not create parallel ones.

## Phase 0: Product And Architecture Definitions

See also: `docs/command_model.md` for the first Phase 0 command-system spec.
See also: `docs/keybind_model.md` for the Phase 0 keybind-system spec.
See also: `docs/plugin_model.md` for the Phase 0 plugin-system spec.
See also: `docs/capability_security_model.md` for the Phase 0 capability/security spec.
See also: `docs/context_model.md` for the Phase 0 context-model spec.

- Define product identity clearly.
- Define command model.
- Define keybind model.
- Define plugin model.
- Define context model.
- Define capability and security model.
- Define identity model for users, bots, webhooks, and apps.
- Define extension rules for programmers implementing repositories, services, APIs, and plugins.

## Phase 1: Communication Core

- Workspaces/servers.
- Channels, DMs, and groups.
- Messages, replies, and optional threads.
- Attachments.
- Pagination, permissions, and membership rules.
- Message search baseline.
- Permalinks and selection-ready message references.

## Phase 2: Command System

See also: `docs/command_model.md` for the command contract and current Phase 2 design rules.

- Built-in command registry.
- Command palette.
- Typed command arguments.
- Custom user commands.
- Chained commands and macros.
- Command execution context.
- Stable command contracts for built-ins and plugins.
- Vim-style command mode in the palette (future):
  - Prefix `:` switches the palette from fuzzy search to structured command parsing.
  - Messages in timeline get temporary line numbers (like Vim).
  - Examples: `:l 46` (select message 46), `:d 12` (delete message 12), `:r 5 hello`
    (reply to message 5), `:t 3` (open thread), `:j #general` (jump to channel),
    `:react 5 👍` (add reaction).
  - This is a later enhancement, not a launch requirement.

## Phase 3: Keybind System

- User-customizable keybinds.
- Context-aware keybinds.
- Command-to-keybind mapping.
- User/workspace/device scopes.
- Conflict handling and discoverability.

## Phase 4: Plugin System

See also: `docs/plugin_model.md` and `docs/local_plugin_host.md`.

- Plugin manifest.
- Local plugin host lifecycle.
- Local install/enable/disable/uninstall flows.
- Local capability prompts.
- Extension points for:
  - commands
  - context actions
  - panels
  - automations
  - integrations
  - local behaviors
- Desktop-local plugin host first.

## Phase 5: Context Actions And Workflow Integrations

- Actions on selected text, messages, threads, and channels.
- Example workflows:
  - create YouTrack issue from selected text or message context
  - run a command from selected communication context
  - post workflow results back into chat
- Bots, webhooks, and integration-triggered workflows.
- Event-triggered automations.

## Phase 6: Real-Time Collaboration

- Voice channels.
- Video calls.
- Screen sharing.
- Call/session presence model.
- Permissions and session behavior.

## Phase 7: Security And Platform Hardening

- Scoped app identities and tokens.
- Capability enforcement.
- Audit and event recording.
- Integration registry.
- Webhook signing and retries.
- Bot installation and lifecycle.
- Plugin trust boundaries and isolation.

## Phase 8: Ecosystem And SDK

- SDKs.
- Plugin examples and templates.
- Versioned extension APIs.
- Dev mode and hot reload.
- Reliability controls.
- Extension author documentation.

## Phase 9: Cross-Platform Expansion

- Web runtime.
- Mobile runtime.
- Capability matrix per platform.
- Platform-specific extension support rules.

## Phase 10: Optional E2EE Messaging

- Opt-in encrypted messaging model.
- Clear product limits.
- Search, moderation, and integration tradeoffs.
- Extension access model under E2EE.

## Where We Are Now

- Large parts of the backend foundation for Phase 1 are already in place.
- Early architecture groundwork for later phases is also in place:
  - replaceable repository and service boundaries
  - app service provider composition
  - request authentication boundary
  - principal foundation
  - actions/events/capabilities/integrations/settings skeletons
- The next step is to work through Phase 0 deliberately before implementing the command, keybind,
  and plugin systems.
