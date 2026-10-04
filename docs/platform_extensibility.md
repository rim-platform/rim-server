# Platform Extensibility Plan

This document turns the current roadmap direction into a concrete backend milestone plan.

The goal is to make RIM not only a programmable workspace for end users, but also a backend that
other programmers can extend by implementing their own repositories, services, or API adapters as
long as they follow stable traits.

## What Is Ready Now

### Architectural Baseline

- `rim_core` already owns domain models and repository traits.
- `rim_services` already owns service traits plus command/query/DTO contracts.
- `rim_infra_services` already acts as the default orchestration layer over repository traits.
- `rim_api` already depends on `Arc<dyn ...Service>` instead of concrete business logic types.
- `rim_server` already composes trait-object services and repositories into a running app.

### Messaging Foundation

- Conversations, channels, DMs, and groups are implemented.
- Timeline messages and optional threads are implemented with documented semantics.
- `thread_id` and `root_thread_id` behavior is defined and tested.
- Conversation titles now persist for channel/group and are rejected for DM.
- Attachments now have a minimal real lifecycle for existing messages.

### Correctness And Hardening

- Membership uniqueness constraints exist at the DB level.
- One-thread-per-root-message uniqueness exists at the DB level.
- Multi-step conversation/thread writes are now atomic in the DB layer.
- Session lookup failures are no longer silently treated as anonymous.
- Cursor pagination is overfetch-based and better aligned across endpoints.

### Performance Improvements Already Landed

- Conversation list paths batch member hydration.
- Message list paths batch root-thread lookup.
- Thread list paths batch root-message hydration.
- Message list paths batch attachment hydration.

### Platform Foundation Progress

- `Actor` now carries a `Principal` foundation while preserving the current user-first path.
- The backend now has first-class core/service modules for:
  - actions
  - events
  - capabilities
  - integrations
  - settings
- `rim_api` now exposes read-only platform-definition queries over those service traits.

## Main Gaps Still Open

See also:

- `docs/command_model.md` for the Phase 0 command-system spec
- `docs/keybind_model.md` for the Phase 0 keybind-system spec
- `docs/plugin_model.md` for the Phase 0 plugin-system spec
- `docs/capability_security_model.md` for the Phase 0 capability/security spec
- `docs/context_model.md` for the Phase 0 context-model spec

### Extensibility Gaps

- `rim_server` still knows too much about request/session auth internals.
- There is no explicit app/container/provider trait for composition.
- There is no formal transport-adapter contract; `rim_api` is one concrete GraphQL adapter.
- Some service contracts still mirror repo/domain cursor details too directly.

### Product-Direction Gaps

- The backend vocabulary is still heavily `server` and `message` centric.
- There is no first-class principal model for bots, apps, or webhooks yet.
- There are no first-class platform primitives yet for actions, events, capabilities, settings, or
  integrations.

### Optimization Gaps

- Some GraphQL paths still over-hydrate fields even when clients do not request them.
- Some likely hot-path DB indexes are still missing.
- Some repo methods still use read-then-update patterns that can be tightened later.

## Milestone Goal

Build a platform substrate that proves:

- repositories are replaceable behind stable traits
- service implementations are replaceable behind stable traits
- API transports are adapters over services, not business-logic owners
- chat is one built-in capability of the workspace, not the whole backend identity

## Milestone Scope

This milestone is intentionally backend-first. It should add platform seams without requiring a full
plugin runtime yet.

## Phase 1: Composition Boundary

### Outcome

Make application wiring replaceable without editing core server logic.

### Work

- Add an application-provider/container trait, for example `AppServices`.
- Move API/server wiring to consume the provider trait rather than manually passing each service.
- Keep the default composition in `rim_server`, but behind one clearly documented assembly layer.

### Proposed Crate Work

#### `rim_services`

- Add a small provider trait module that groups the service traits required by API adapters.

#### `rim_api`

- Accept either:
  - a provider trait object, or
  - a thin struct built from provider-backed service trait objects.

#### `rim_server`

- Move current composition logic into a dedicated builder module.
- Keep `main`/bootstrap small and transport-focused.

### Success Criteria

- A programmer can swap in a different service bundle without rewriting GraphQL schema wiring.

## Phase 2: Request Authentication Boundary

### Outcome

Transport code stops talking directly to low-level session persistence.

### Work

- Introduce a request-authentication port, for example `RequestAuthenticator`.
- Move cookie/session/CSRF-to-actor logic behind that boundary.
- Keep `AuthService` for login/logout/bootstrap flows, but stop mixing it with direct repo access in
  `rim_server`.

### Proposed Crate Work

#### `rim_services`

- Define a trait for request authentication / session validation.

#### `rim_infra_services`

- Add the default implementation using the existing session repository and auth utilities.

#### `rim_server`

- Replace direct session repo usage with the new request-auth port.

### Success Criteria

- A programmer can replace request auth behavior without editing Actix handler internals.

## Phase 3: Principal Model

### Outcome

The backend stops assuming that every actor is only a human user session.

### Work

- Introduce a core principal model.
- Support at least:
  - user principal
  - app/bot principal
  - webhook/integration principal
- Keep user/session behavior working as the first concrete path.
- Define where principal-scoped capabilities will live.

### Proposed Crate Work

#### `rim_core`

- Add principal types and ids.
- Add any needed repository traits for bot/app identity lookup.

#### `rim_services`

- Generalize `Actor` into a principal-aware contract.
- Update service commands incrementally where needed.

#### `rim_infra_services`

- Add default principal resolution rules.

### Success Criteria

- Service boundaries can represent bots/webhooks/apps without redesigning the whole auth model.

## Phase 4: Platform Primitive Skeletons

### Outcome

The backend gets first-class non-chat building blocks.

### Work

- Add initial domain models and traits for:
  - `Action`
  - `Event`
  - `Capability`
  - `Settings`
  - `Integration`
- Keep v1 intentionally small, but make the contracts real.

### Proposed Crate Work

#### `rim_core`

- Add primitive models and repository traits.

#### `rim_services`

- Add service traits, commands, queries, and DTOs.

#### `rim_infra_services`

- Add default implementations, even if initially in-memory or DB-light where appropriate.

#### `rim_api`

- Expose a small initial surface for reading/installing/listing or invoking primitives.

### Success Criteria

- The repo no longer looks like messaging plus auth only; the platform layer becomes visible in code.

## Phase 5: One Proving Vertical Slice

### Outcome

Demonstrate that messaging is a platform capability, not the whole platform.

### Work

- Implement built-in action: `post_message`.
- Implement built-in event: `message.created`.
- Route message creation through the action path where practical, or at least ensure the action uses
  the same service layer.
- Emit the event whenever a message is created.

### Proposed Crate Work

#### `rim_core`

- Add action/event models and persistence contracts.

#### `rim_services`

- Add invoke-action and list-events service contracts.

#### `rim_infra_services`

- Implement `post_message` over existing message service logic.
- Emit `message.created` from the message write path.

#### `rim_api`

- Expose one action invocation endpoint and one event read/debug endpoint.

### Success Criteria

- Chat is visibly one action/event source inside the platform model.

## Phase 6: Adapter Story

### Outcome

Make it obvious how outside programmers can build alternate APIs.

### Work

- Document transport rules:
  - transports depend on service traits
  - transports do not talk directly to repos
  - transports do not contain business rules
- Keep `rim_api` as the reference GraphQL adapter.
- Document how a REST adapter or local-plugin adapter would sit on the same services.

### Proposed Crate Work

#### `docs`

- Add an architecture doc for extension authors.
- Include examples of custom repo, service, and transport implementations.

### Success Criteria

- A programmer can understand how to write an alternate API adapter without reverse-engineering the
  whole server crate.

## Phase 7: Optimization Hardening

### Outcome

Make the new platform substrate safe to build on.

### Work

- Add likely missing indexes for current hot queries.
- Revisit over-hydration on GraphQL-heavy message/conversation/thread paths.
- Replace read-then-update repo methods where direct updates are enough.
- Add query-path profiling notes for key endpoints.

### Candidate Indexes To Evaluate

- `conversation_member(user_id, conversation_id)`
- `message_attachment(message_id)`
- `message_thread(conversation_id, last_message_at, id)`
- conversation listing indexes shaped to inbox/channel list queries

## Suggested Execution Order

1. Composition boundary
2. Request authentication boundary
3. Principal model
4. Platform primitive skeletons
5. `post_message` action + `message.created` event
6. Adapter/extensibility docs
7. Optimization hardening

## Programmer Extension Rules

These are the rules we should explicitly preserve as the code evolves.

- `rim_core` traits define backend persistence contracts.
- `rim_services` traits define business-service contracts.
- `rim_api` is an adapter over service traits, not a domain owner.
- `rim_server` is composition and HTTP wiring, not business logic.
- Default infra implementations should be replaceable, not privileged by hidden side channels.
- If an invariant requires a custom repository behavior, document it in the trait contract.

## Definition Of Done For This Milestone

The milestone is complete when:

- a custom repository backend can be implemented against documented `rim_core` traits
- a custom service implementation can be plugged in without rewriting transport code
- request auth/session validation is behind a service-facing boundary
- a custom transport can target service traits without talking to repositories
- bots/apps/webhooks fit the principal model without reworking existing user flows
- at least one action and one event exist as first-class backend primitives
