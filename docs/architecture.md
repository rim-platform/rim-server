# Architecture

This document describes the current architectural direction of RIM.

It is intentionally higher level than crate-specific implementation notes. The goal is to explain:

- what RIM is architecturally
- how the system is layered today
- what each crate is responsible for
- where state and behavior should live
- how the system can evolve later without rewriting everything

## Product Framing

RIM is a modern team chat app on steroids.

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

Privacy and security are first-class principles. Optional E2EE is a later product phase, not a
short-term shortcut.

## Architectural Principles

### Layered Boundaries

The system is designed around clean boundaries between:

- domain/repository contracts
- service contracts
- default infrastructure implementations
- transport adapters
- runtime/bootstrap wiring

### Extensibility First

Developers should be able to:

- implement their own repositories
- implement their own services
- build alternate API adapters
- write plugins and integrations later

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
- request/session/bootstrap wiring

It should stay focused on composition and request orchestration.

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

### Conversation

A conversation lives inside a server/workspace and represents one of:

- channel
- DM
- group chat

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

The system has a principal foundation for:

- users
- apps
- bots
- webhooks

Today, runtime behavior is still mostly user-first, but the architecture is preparing for broader
principal types.

### Command

Commands are the common execution unit across:

- command palette
- keybinds
- context actions
- user-defined workflows
- future plugins/integrations

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

The system also has early platform foundations for:

- actions
- events
- capabilities
- integrations
- settings

These exist as skeletons today and will deepen in later phases.

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

Current architectural direction:

- plugins are local/client runtime extensions
- the local plugin host owns install/enable/disable and runtime behavior
- the backend provides stable APIs and command surfaces plugins can call through the host
- backend-managed plugin installation is not the default first-version architecture

See also: `docs/local_plugin_host.md`.

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

## Current Implementation State

### Already Real

- messaging core
- thread model
- attachment baseline
- provider-based service composition
- request authentication boundary
- command backend slice
- keybind persisted/shared backend slice
- platform primitive skeletons

### Intentionally Incomplete

- client-side runtime keybind layer
- desktop-local command execution runtime
- plugin runtime/host
- stronger capability enforcement
- richer principal-aware runtime behavior
- platform primitive persistence/execution

## Future Evolution

The current architecture is compatible with later splitting the system into more independent
services or runtimes if needed.

Possible future service/runtime seams include:

- auth/session service
- messaging service
- command catalog/execution service
- plugin/integration runtime service
- notification/event service
- search/indexing service
- media/call service

The current code is not deployed that way today, but the boundaries are intended to make that
possible later.

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

## Architecture Health Check

At the current stage, the architecture is in a good place.

The main subsystem to keep watching is the command/keybind/plugin substrate, because it is the most
likely area to become too central or too hardcoded if not kept clean as new phases are implemented.
