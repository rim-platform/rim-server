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

## Feature Ideas — Next Wave

Features that would differentiate RIM from Discord, Slack, Teams, and other competitors.
Prioritized by strategic impact and grouped by theme.

### Programmable Runtime (Core DNA)

These turn RIM from "chat with plugins" into a true programmable workspace runtime.

- **Command Piping** (M): Chain commands with Unix-style pipes —
  `/deploy staging | notify #ops | log audit`. Output of one command feeds into the next.
  No competitor has composable commands; they all treat them as isolated endpoints.

- **Workspace State Machine** (M): Shared key-value store with subscriptions. Commands,
  macros, and plugins read/write state. Enables stateful cross-command workflows without
  an external database.

- **Reactive Pipelines** (L): User-defined event-driven workflows — "when X happens in
  channel Y, do Z." Like IFTTT but native to the workspace, no external service required.
  Slack requires Zapier or a custom app; RIM makes it first-class.

- **Scriptable Views / Embedded Panels** (XL): Small TypeScript/Lua scripts that render
  custom inline UI — dashboards, forms, polls, kanban. Lighter than full plugins.
  No competitor offers user-authored inline UI without deploying an external app.

### AI-Native (Not AI-Bolted-On)

Deeply integrated AI that augments the workspace, not a chatbot sidebar.

- **AI Context Roles** (M): Assign persistent AI personas per channel with scoped tool
  access — "security-bot in #alerts can only read, never post externally." Role-based AI
  permissions, not one global copilot.

- **AI Macro Builder** (M): Describe a workflow in natural language, AI generates the
  macro step sequence. Lowers the barrier to programmability for non-power users.

- **Conversation Auto-Summarization** (S): Auto-generated summaries on configurable
  triggers — time threshold, message count, topic drift detection. Stored as pinned
  artifacts. Others offer on-demand summaries; RIM makes them automatic and rule-driven.

### Power User UX

Small touches that make power users never want to leave.

- **Zen Mode** (S): One keybind (`Ctrl+Shift+Z`) strips everything except the current
  conversation. No rails, no panels, no distractions. Like VS Code zen mode for comms.

- **Keyboard Message Actions** (M): `j/k` navigation over messages, then single keys for
  actions — `r` reply, `e` edit, `t` thread, `p` pin. Zero mouse. Superhuman model
  brought to team chat.

- **Layout Presets** (M): Save and switch between panel arrangements — "Focus" (main
  only), "Review" (main + thread + tool panel), "Triage" (side rail expanded + compact).
  Keybind to cycle. Different tasks need different layouts; nobody lets you save them.

- **Compact Mode + Smart Grouping** (S): Three density levels
  (comfortable/compact/ultra-compact). Consecutive messages from the same user within
  2 minutes collapse into a single block. Toggle with `Ctrl+D`.

- **Glanceable Status Bar** (S): Thin bar at the bottom showing unread count per
  workspace, active thread count, connection status, current keybind mode. VS Code style.
  Replaces scanning the entire sidebar for badges.

- **Spatial Keyboard Navigation** (M): Arrow keys move between panels (workspace rail,
  side rail, main surface, thread pane). `Tab` cycles within a panel. `Escape` goes "up"
  one level. Consistent everywhere.

- **Progressive Disclosure Shell** (L): Show only what the user needs at their skill
  level. First launch is clean and minimal. As they use more features, the UI subtly
  introduces advanced controls. No onboarding wizard, no tooltip hell.

### Developer Workflow Integration

Features that make RIM the natural home for engineering teams.

- **Channel Lenses / Filtered Views** (M): Save filtered views of a channel — "only
  messages with links," "only from @backend-team," "only unresolved questions."
  Persistent, shareable, keybind-accessible. Programmable views on existing channels.

- **Incident Mode** (L): One command flips a channel into incident mode — timeline
  auto-generated, messages tagged by phase, AI drafts postmortem from transcript.
  Replaces dedicated incident tools for small-to-mid teams.

- **Git-Aware Channels** (L): Channels linked to repos. Branches, PRs, commits surface
  as structured objects — not webhook text dumps. Commentable inline. Slack gets spam;
  RIM gets interactive, queryable git context.

- **Embedded Terminal Sessions** (XL): Shared terminal panels in channels. Pair debugging
  without screen share.

### Structural Innovation

- **Threaded Threads / Nested Conversations** (L): Nestable sub-threads with collapsible
  tree view. Zulip proved topic-threading works; go further with depth.

- **Local-First Encrypted Spaces** (XL): E2E encrypted channels where data never hits the
  server unencrypted. Tauri's local-first architecture makes this natural. Matrix does
  this but with terrible UX; RIM can do it right. (See also Phase 10.)

### Micro-Interactions

- **Cursor Trail Warmth** (S): When switching panels via keyboard, a subtle 80ms highlight
  pulse on the newly focused panel edge. Spatial orientation in a multi-panel app.

- **Inline Command Preview** (S): Type `/` in the composer and get a filtered command list
  right there — no modal, no context switch. Results appear inline like autocomplete.

### CLI Client

A terminal-native client for RIM. No competitor has a real CLI chat experience. This
aligns perfectly with RIM's identity as a programmable workspace for power users.

The backend GraphQL API and WebSocket subscriptions are already in place — a CLI client
is just another consumer of the same contracts.

**Phase 1 — CLI tool** (M): Scriptable command-line interface (`rim` binary). Useful in
scripts, cron jobs, CI pipelines, and for quick interactions without opening the desktop
app.

  ```
  rim login
  rim channels
  rim send #general "Deploy complete"
  rim search "bug" --channel #dev
  rim status set "In a meeting" --expires 1h
  rim macro run my-deploy-macro
  rim watch #alerts                    # real-time stream
  ```

  Stack: Rust (`clap` + `graphql-client` + `tokio-tungstenite`). Could live as a new
  crate in the `rim-server` workspace or as a standalone `rim-cli` repository.

**Phase 2 — TUI mode** (L): Full terminal UI via `ratatui` — panels, message list,
composer, real-time updates. Think `weechat` meets modern Rust. Invoked as `rim tui`.
Shares the same GraphQL client core as the CLI tool.

**Phase 3 — Convergence** (M): CLI commands become available inside the TUI as a command
palette (`:send`, `:search`, etc.). TUI and CLI share a workspace of keybinds, macros,
and session state.

## Where We Are Now

Phase 0 through Phase 4 are largely implemented, with programmable runtime and power user
UX features shipping:

- **Phase 1 (Communication Core)**: Complete — channels, DMs, groups, threads,
  attachments, search, pagination, permissions, mentions, reactions, pinning, forwarding,
  typing indicators, read state, link previews, custom emoji.
- **Phase 2 (Command System)**: Complete — built-in registry, command palette, user
  commands, chained macros, execution context, command piping with pre-validation and
  per-stage results, macro name-to-ID resolution, 5 built-in workspace state commands.
- **Phase 3 (Keybind System)**: Complete — 19 default keybinds, customizable, categories,
  context-aware, inline editing.
- **Phase 4 (Plugin System)**: Complete — manifest, local host, install/enable/disable,
  capability prompts, sandboxed iframes, context actions, panels.
- **Phase 5 (Context Actions)**: Partially complete — AI agent with tool calling
  (send_message, pin_message, create_channel, add_reaction), context actions from plugins.

Programmable runtime features shipped:
- **Workspace State Machine**: Full-stack KV store with per-server/per-user scoping, JSONB
  values, TTL support, atomic increment, quota enforcement, real-time events via WebSocket.
- **Command Piping Hardening**: Pre-validation pass (dangerous + desktop-local rejection),
  explicit `isPipe` flag, enhanced error messages with previous stage context.
- **Macro Name Resolution**: GraphQL query to resolve macro by name, frontend async lookup.

Power user UX shipped:
- **Zen Mode**: `Ctrl+Shift+Z` strips UI to current conversation only, with animated
  transitions.
- **Compact Mode**: Three density levels (comfortable/compact/ultra-compact), `Ctrl+D`
  toggle.
- **COMPOSE/NAVIGATE Mode**: Vim-like mode model anchored to composer focus, with `j/k`
  message navigation, single-key actions (`r` reply, `e` edit, `t` thread, `p` pin).
- **Status Bar**: Glanceable bar showing mode indicator, density, connection status.
- **Pipeline Result View**: Per-stage result rendering with status icons and collapsible
  JSON payload.

Infrastructure and quality:
- CI/CD: GitHub Actions pipelines for both repos (lint, test, coverage, Docker build).
- Docker: Production Dockerfiles for server and UI, docker-compose for local dev stack.
- Testing: 344 backend tests (95% coverage gate), 691 frontend tests (Vitest).
- Security: HMAC-signed CSRF tokens bound to sessions, Stronghold passwords derived from
  OS keychain, CSP headers, rate limiting, SSRF protection, server membership checks on
  all workspace state operations.

The next step is reactive pipelines, AI macro builder, and inline command preview to
deepen the programmable workspace experience.
