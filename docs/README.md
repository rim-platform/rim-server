# Documentation

## Product

- `vision.md`: what RIM is for, who it is for and who not, and the product principles.
- `architecture.md`: layers, crates, the domain model, and what the backend and the client own.
- `../ROADMAP.md`: where it stands, what comes next and later, and what is not planned.

## Models

How the programmable layer works, independent of the transport:

- `command_model.md`: commands, builtins, the AI-callable set, the execution gates.
- `keybind_model.md`: keybind scopes, contexts and precedence.
- `context_model.md`: the context a command runs in.
- `capability_security_model.md`: capabilities, sensitivity, confirmations.
- `plugin_model.md`: what a plugin is and what the server's part in plugins is.
- `local_plugin_host.md`: how the desktop app hosts plugin processes and panels.

## Server contracts

- `api.md`: talking to a server over HTTP: the routes, sessions and CSRF, origins, error codes
  and version negotiation, for client authors.
- `messaging.md`: conversations, channels, roles and moderation, notification rules, threads.
- `pagination.md`: the cursor pagination contract.
- `message-change-feed.md`: the change feed client caches sync from.
- `search.md`: full-text search syntax, ranking and visibility.
- `email.md`: SMTP configuration and the mails the server sends.
- `server-identity.md`: the server key, the franking key file, key login audiences, roles.
- `relay.md`: what a relay and a host do, for operators and for clients.

## Specifications

Normative, versioned contracts in `specs/`:

- `specs/key-identity-v1.md`: keys, signed statements, challenge login.
- `specs/device-linking-v1.md`: giving a new device its certified key.
- `specs/e2ee-mls-v1.md`: end-to-end encryption with MLS.
- `specs/server-to-server-v1.md`: server identity, the signed HTTP API, relays and hosts.
- `specs/plugin-protocol-v1.md`: the plugin process protocol.
- `specs/web-plugins-v1.md`: sandboxed web plugins.
- `specs/workspace-plugins-v1.md`: signed manifests and workspace plugin registries.

## Operations

- `self-hosting.md`: running a server with `deploy/compose.yml`: first start, updates, backups,
  monitoring.
- `operations/relay-public.md`: the public relay's runbook: keys, backups, quotas, incidents, abuse.
- `operations/closed-company-server.md`: a server closed to DMs from outside, or open to partner
  servers only.
- `operations/upgrade.md`: moving a server to a new release and back, and why migrations only go
  forward.
- `operations/release-checklist.md`: the steps that cut a release.
- `operations/audit-retention.md`: audit log retention, export and purge.
- `operations/background-jobs.md`: the timers the server runs.
- `operations/features.md`: instance feature switches (`RIM_FEATURES_DISABLED`).
- `operations/metrics.md`: the Prometheus metrics.

## Security

- `security-overview.md`: what RIM protects and from whom, the threat model, the known gaps and
  the review status, for someone deciding whether to run or join a server.
- `security/audit-access-matrix.md`: who can read which audit rows.
- `security/nested-resolver-access.md`: which nested GraphQL fields load other objects and what
  check stops a viewer from reading what the matching root would refuse.
- `security/crypto-review-brief-2026-10.md`: the brief for the outside cryptographic review,
  pinned to the commits it describes.
