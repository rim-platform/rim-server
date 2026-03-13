# Local Plugin Host

This document describes the intended first plugin runtime for RIM.

Plugins are local client/runtime extensions, not backend-managed installs. The local plugin host is
responsible for loading plugin manifests, managing local enable/disable state, brokering
capabilities, and exposing safe host/backend APIs to plugins.

## Core Rule

Plugins are local extensions that talk to a local host.

The host:

- loads the manifest
- validates extension declarations
- approves capabilities locally
- launches the plugin runtime
- brokers backend API access
- brokers local sensitive operations

The backend remains a stable API surface for shared app state; it does not own plugin lifecycle in
the first version.

## Ownership

### Local Plugin Host Owns

- plugin folder discovery
- local install/enable/disable/uninstall
- manifest parsing/validation
- local capability approval
- local runtime lifecycle
- local command registration
- local keybind suggestion registration
- local context action registration
- local panel registration
- local plugin settings

### Backend Owns

- auth/session validation
- shared state
- messaging data
- backend commands
- search/reference APIs
- integrations/bots/webhooks later

## Manifest

The local manifest file should be:

- `rim-plugin.json`

Shared manifest contracts now exist in:

- `rim_services::plugin_host::manifest`

The manifest describes:

- plugin identity
- runtime kind
- entrypoint
- requested capabilities
- commands
- keybind suggestions
- context actions
- panels
- settings schema

## Runtime Protocol

Shared runtime protocol contracts now exist in:

- `rim_services::plugin_host::runtime`

The first protocol supports:

- handshake request/response
- command execution request/response

This is enough to start with command-first plugins.

## Capabilities

Capabilities are local plugin permissions over host-managed or backend-brokered operations.

Examples:

- `chat.read`
- `chat.write`
- `ui.panel`
- `local.process`
- `local.filesystem`
- `http.outbound`

Capabilities should be approved by the local user in the first version.

## Security Direction

The first plugin host should not blindly trust arbitrary local processes.

The intended direction is:

- local plugin host
- brokered access to sensitive features
- capability checks at the host boundary
- plugins call backend APIs through the host

Manifest capabilities are not, by themselves, a complete OS sandbox. The host/broker model is the
minimum security boundary, and stronger sandboxing can be layered later.

## First Plugin Version Priority

The first real plugin host should prioritize:

- commands first
- local manifest loading
- local command registration
- brokered backend API access
- local capability approval

Panels, context actions, and keybind suggestions can follow on top of the same host/runtime model.
