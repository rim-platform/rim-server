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

The current direction assumes a local plugin host with a brokered runtime.

The local plugin process should not directly own backend installation state or private backend
credentials. It should instead talk to a local host that can:

- expose host UI/runtime features
- broker local dangerous operations
- call backend APIs on behalf of the current user/session where allowed

## Runtime Kind

The first real plugin system should support only:

- `desktop_local`

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
