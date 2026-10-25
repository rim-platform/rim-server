# Architecture

This document describes the architecture of the RIM server.

It is intentionally higher level than crate-specific implementation notes. The goal is to explain:

- what RIM is architecturally
- how the system is layered today
- what each crate is responsible for
- where state and behavior should live
- which rules keep new features compatible with encryption and more than one server

## Product Framing

RIM is a programmable communication workspace (`docs/vision.md`).

Communication is the base surface, but the product is differentiated by:

- plugins
- commands
- keybinds
- context-aware workflows

The core product idea is that communication and action should live in the same environment.

Examples:

- highlight text and create a YouTrack issue
- open a Kubernetes panel from chat context
- bind a custom workflow to a keybind
- run a command chain without leaving the app

Privacy and security are first-class principles. DMs and private groups are end-to-end encrypted
with MLS, a person is a key pair rather than an account on one server, and servers act as relays
and workspace hosts for each other (`docs/specs/`).

## Architectural Principles

### Layered Boundaries

The system is designed around clean boundaries between:

- domain/repository contracts
- service contracts
- default infrastructure implementations
- transport adapters
- runtime startup wiring

### Extensibility First

Developers should be able to:

- implement their own repositories
- implement their own services
- build alternate API adapters
- write plugins and integrations

This means contracts and boundaries matter more than local convenience.

### Backend-Owned vs Client-Owned Responsibilities

The backend should own:

- persisted shared state
- workspace/server state
- messaging state
- user-defined commands
- persisted keybind layers
- auth/session validation
- shared permissions/capabilities

The client/runtime should own:

- actual input handling
- device-local configuration
- UI session state
- runtime-local navigation behaviors
- desktop-local command execution

### Security By Default

The system should separate:

- user permissions
- extension capabilities
- local vs remote trust boundaries
- dangerous vs ordinary behavior

No subsystem should silently gain power just because it exists.

### Design Guardrails

Every new server feature answers two questions: what it does when the message content is
encrypted, and what it does when there is more than one server. These rules keep the answers
cheap.

- **Message identity is global, not server-local.** Ids are UUIDv7, row ids are never exposed,
  and nothing is keyed on "the server". New tables are designed with an origin column in mind
  even where it is a constant today.
- **Content and metadata are separable.** Data derived from message text (search vectors, link
  previews, mention rows) can be absent for an encrypted message without breaking the row. No
  server feature may require reading message text; it may benefit from it.
- **The client stays capable.** Commands, macros, keybinds, lenses and plugin logic stay
  evaluable on the client, and server features prefer metadata (membership, timing, structure)
  over content.
- **One principal model.** People, bots and the system user are principals; keys and remote
  users add a kind and a key to a principal rather than a new concept. Plugins and new APIs see
  an opaque `principalId`, never a local UUID or `name@domain`.
- **GraphQL is the client contract.** Server-to-server traffic is a small signed HTTP API, a
  non-GraphQL transport like the upload routes, not federation over GraphQL.
- **Versioned contracts overlap.** The plugin protocol, the archive format and the
  server-to-server API each keep serving the previous version while a new one exists.
- **Migrations only expand.** A column or table is added in one release and removed at the
  earliest a release later, so a host can roll the binary back (`docs/operations/upgrade.md`).
- **Single-node stores stay behind traits.** See "Scaling Limits" below.
- **No server-only verticals.** A feature for one kind of community is a plugin, a bot or a
  pipeline (`CONTRIBUTING.md`, "What belongs in the core").

## Current Crate Responsibilities

### `rim_core`

Owns:

- domain models
- strongly typed ids
- repository traits

This crate should stay transport-agnostic and mostly infrastructure-agnostic.

### `rim_services`

Owns:

- service traits
- commands/queries
- DTOs
- provider contracts

This is the main contract layer between product behavior and transport/runtime adapters.

### `rim_infra_db`

Owns:

- SeaORM-backed repository implementations
- DB error mapping
- persistence concerns

It should implement repository contracts, not product workflows.

### `rim_infra_services`

Owns:

- default service implementations
- orchestration across repositories
- validation and policy checks
- execution routing for the current product slices

This crate is the main default business-logic layer.

### `rim_api`

Owns:

- GraphQL types
- query/mutation mapping
- transport-level validation/mapping

It should remain an adapter over service traits, not a business-logic owner.

Layout: resolvers are split per domain under `rim_api/src/query/` and
`rim_api/src/mutation/` (one `#[Object]` per domain, merged into `Query` and
`Mutation` with `#[derive(MergedObject)]` in each `mod.rs`, in groups of at
most five to stay under rustc's recursion limit; member order is the SDL field
order, so new objects are appended). GraphQL types mirror that split
under `rim_api/src/types/` and are all re-exported from `types/mod.rs`.
Resolver unit tests live in `rim_api/src/tests/` (shared mock fixtures in
`mod.rs`, one submodule per domain). Shared resolver helpers stay in their own
modules: `auth_context` (`require_actor`, `require_session_id`,
`require_server_member`), `auth_intent`, `errors` (`map_service_error`), `ids`
(`parse_uuid_id`) and `resolve` (DataLoader accessors).

### `rim_server`

Owns:

- app/runtime assembly
- HTTP wiring
- request/session/startup wiring

It should stay focused on composition and request orchestration.

It is also the composition root (`app.rs`) and serves the transports that are not GraphQL:
uploads and downloads, plugin bundles, health and metrics, the server-to-server routes and the
relay's forwarding route. `CLAUDE.md` describes these two exceptions to the layer chain.

### `rim_auth`, `rim_identity`, `rim_s2s`

Leaf crates with no database or HTTP code:

- `rim_auth`: password hashing, session and CSRF tokens.
- `rim_identity`: verification of key identity statements (key logs, device certificates,
  profile cards, login proofs, plugin manifests, MLS credentials).
- `rim_s2s`: the server key log and `rims1_` ids, the signed descriptor, server-signed
  statements and the RFC 9421 signature profile of the server-to-server transport.

### `rim_logging`

Structured JSON logging and the daily audit files.

### `entity`

Owns:

- SeaORM entity definitions
- entity-to-domain mapping helpers

### `migration`

Owns:

- schema evolution
- DB migration history

## Core Domain Model

### Workspace / Server

The persisted/core identifier still uses `ServerId`, but product terminology prefers `workspace`.

Today:

- a server/workspace is the top-level shared container
- members hold one of four roles: `Owner`, `Moderator`, `Member` (default), `Guest`
  (restricted to conversations they were added to). Moderation state — bans, timeouts and
  message reports — lives in `rim_core::moderation` (`ModerationRepository`) and is managed by
  `ModerationService` (`rim_infra_services/src/moderation.rs`), which also owns the shared
  moderation audit writer (`record_moderation_action`). See `docs/messaging.md`, Roles And
  Moderation.

### Conversation

A conversation lives inside a server/workspace and represents one of:

- channel (public, readable by every workspace member, or private, readable by its members only)
- DM
- group chat

Access rules (`rim_infra_services/src/util.rs`): `ensure_conversation_member_access`
lets any workspace member read a public channel; private channels, DMs and groups need a
`conversation_member` row (a private-channel non-member gets `NotFound`, a DM/group non-member
`PermissionDenied`). `ensure_conversation_writable` adds the archive check
(`ConversationArchived`) for every write path. See `docs/messaging.md`.

The gate is role-aware: it loads the actor's `MemberStanding` (role plus any active
timeout, `ServerMemberRepository::get_member_standing`) instead of a bare membership check. A
`Guest` needs a `conversation_member` row even for a public channel (`NotFound` otherwise), and
`ensure_conversation_writable` also refuses a timed-out member (`TimedOut`). Slow mode is
checked in `MessageService` on send/reply/forward.

### Message

A message belongs to a conversation.

Messages exist in two major forms:

- timeline messages
- thread replies

### Thread

A thread is a separate entity anchored to a root timeline message.

Rules:

- the root message remains in the timeline
- thread replies live in the thread view

### Attachment

An attachment belongs to a message.

### Principal

A principal is who acts. Every user with a key identity has a stable principal id (`rimp1_…`)
that plugins and other servers see instead of a server-local id; a person from another server is
a `user` row of kind `remote` keyed by that id. `Principal` and `PrincipalKind` in `rim_core` are
the seam for further kinds such as bots, which do not exist yet.

### Command

Commands are the common execution unit across:

- command palette
- keybinds
- context actions
- user-defined workflows
- plugins and the AI agent

Current backend state:

- built-in command registry/metadata exists
- backend command execution exists
- persisted user-defined command composition exists
- command lookup/listing/filtering exists as a service/API contract

### Keybind

Keybinds map key combinations to commands.

The backend currently owns persisted/shared keybind layers, while the client is expected to own
device-local bindings and actual runtime key handling.

The intended final runtime precedence is:

- `device_local > workspace > user > default`

### Platform Primitives

Of the early platform foundations, only capabilities remain: the capability registry backs the
runtime gate. The hardcoded `actions`,
`events`, `integrations` and `settings` stubs were removed; actions are commands, events are the
workspace event stream, and integrations and settings will get real models when they are built.

## Commands, Keybinds, Plugins, and Context

### Commands

Commands are the backbone for execution.

Important rule:

- commands own behavior
- transports, keybinds, and context actions trigger commands

### Keybinds

Keybinds should stay simple:

- they trigger commands
- they do not own workflow logic

### Plugins

Plugins are extension packages that may contribute multiple extension points, such as:

- commands
- panels
- context actions
- automations
- integrations

Plugins run in the client, never on the server. The client's plugin host owns installation,
consent and runtime; the server keeps per-workspace registries of signed plugins and serves web
plugin bundles. See `docs/plugin_model.md`.

### Context

Context is structured runtime state available at invocation time.

Examples:

- selected text
- selected message
- current conversation
- active panel
- focused surface

Context should be typed and shared across command palette, keybinds, and context actions.

## Persisted vs Runtime Layers

### Persisted Backend State

Examples:

- users
- workspaces/servers
- conversations
- messages
- threads
- attachments
- user-defined commands
- user keybinds
- workspace keybinds

### Backend-Resolved But Not Runtime-Final

Examples:

- default keybind suggestions
- resolved persisted keybind layers

These are useful to clients, but they are not the full final runtime behavior.

### Runtime-Local Client State

Examples:

- device-local keybinds
- current selection/focus state
- recent-activity cycling state
- open panels/routes
- desktop-local command execution state

## Security Model

### Permissions vs Capabilities

- users primarily operate through membership/permission rules
- plugins/integrations/future non-user principals operate through capabilities

### Dangerous Behavior

Dangerous behaviors should be explicit, especially for:

- local process execution
- local filesystem access
- broad outbound network access

### Trust Boundaries

The architecture should preserve visible trust boundaries between:

- built-in behavior
- user-defined behavior
- plugin behavior
- integration behavior

## Scaling Limits

One server process holds some state in memory behind traits: the event broadcaster, the
confirmation store, rate limiters, presence and typing. Some background jobs, such as the notification digest, assume one node. Running
several nodes of one server needs those traits backed by shared stores first; new features must
not depend on single-node semantics.

## Contributor Rules

When adding new features:

- add new domain models/repo traits to `rim_core`
- add service contracts to `rim_services`
- add default implementations to `rim_infra_services`
- keep DB-specific logic in `rim_infra_db`
- keep transport logic in `rim_api`
- keep HTTP/runtime composition in `rim_server`

Avoid:

- business logic in GraphQL resolvers
- transport concerns in domain models
- persistence details leaking into service contracts unnecessarily
- runtime-local concerns being pushed into backend persistence just for convenience

Every new server feature also answers two questions: what it does when the message content is
encrypted, and what it does when the people involved are on more than one server.
