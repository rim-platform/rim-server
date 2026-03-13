# Roadmap

RIM is a modern team chat app on steroids.

Fast communication is the base, but the product is defined by how customizable and programmable it
is. Plugins, commands, and keybinds are not side features; they are core product differentiators.
RIM should make it easy to communicate, act on communication context, and tailor the workspace to
 how a team works.

Privacy and security are core product principles from the beginning. Optional end-to-end encrypted
messaging is a later dedicated phase, not a rushed add-on.

## Product Identity

See also: `docs/architecture.md` for the current high-level system architecture.

- Modern team communication app with channels, DMs, groups, threads, and later voice/video/screen
  sharing.
- Chat app on steroids: communication plus deep customization.
- Commands, keybinds, and plugins must be easy to create, configure, and use.
- Communication context should turn into action quickly.

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
