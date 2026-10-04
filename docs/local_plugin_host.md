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

The manifest contract and its install-time validation live in the desktop plugin host:

- `rim-ui/src-tauri/src/plugin_host/manifest.rs` and `validation.rs`

The backend does not define plugin manifest types.

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

The runtime protocol is implemented by the desktop plugin host:

- `rim-ui/src-tauri/src/plugin_host/` (`handshake.rs`, `wire.rs`, `broker.rs`)

### Wire Protocol (Neovim-inspired)

Inspired by Neovim's msgpack-rpc and LSP, the plugin wire protocol uses:

- **Transport:** stdin/stdout (universal — every language supports this)
- **Framing:** 4-byte big-endian length prefix + payload
- **Format:** JSON-RPC 2.0 as primary. MessagePack as optional upgrade (negotiated at handshake).
- **Directionality:** Bidirectional — host calls plugin AND plugin calls host.

Handshake is always JSON. After handshake, communication uses the negotiated format.

### Message Types

Three message types (following JSON-RPC 2.0):

- **Request** `{ jsonrpc: "2.0", id: N, method: "...", params: {...} }` — expects response
- **Response** `{ jsonrpc: "2.0", id: N, result: {...} }` or `{ ..., error: {...} }`
- **Notification** `{ jsonrpc: "2.0", method: "...", params: {...} }` — no id, no response expected

### Host → Plugin Methods

- `initialize` — handshake, capability negotiation, wire format selection
- `execute_command` — invoke a command registered by the plugin
- `deliver_event` — push a subscribed event to the plugin
- `shutdown` — graceful shutdown request

### Plugin → Host Methods (capability-gated)

Every plugin→host call is checked against the plugin's approved capabilities.

- `rim.chat.get_messages` — requires `chat.read`
- `rim.chat.send_message` — requires `chat.write`
- `rim.chat.get_conversations` — requires `chat.read`
- `rim.ui.register_panel` — requires `ui.panel`
- `rim.ui.update_panel` — requires `ui.panel`
- `rim.ui.show_notification` — requires `ui.notify`
- `rim.events.subscribe` — requires relevant capability for the event type
- `rim.settings.get` — requires `settings.read`
- `rim.settings.set` — requires `settings.write`
- `rim.log` — always allowed (plugin logging)

### Event Subscription

Plugins can subscribe to workspace events through the host:

```
Plugin → Host: { method: "rim.events.subscribe", params: { events: ["chat.message_created"] } }
Host → Plugin: { method: "deliver_event", params: { type: "chat.message_created", data: {...} } }
```

The host checks event subscriptions against capabilities and conversation access (same rules as
WebSocket subscription filtering).

### Format Negotiation

During `initialize`, the host sends `supported_formats: ["json", "msgpack"]`. The plugin responds
with `chosen_format: "json"` or `"msgpack"`. All subsequent messages use the chosen format.

This allows:
- Simple plugins (Python scripts) to use JSON with zero dependencies
- Performance-sensitive plugins to use MessagePack
- Any language with stdin/stdout support to participate

### Lazy Process Startup

Plugin processes are NOT started at application launch:

1. On startup: load all `rim-plugin.json` manifests
2. Register commands, keybinds, panels from manifest (metadata only)
3. Plugin process starts ONLY when first needed (command invoked, panel opened, event fires)
4. After idle timeout: plugin process may be stopped (graceful shutdown)

### Headless Plugins (No UI)

Plugins may or may not have UI. A headless plugin:

- Has no `panels` in manifest
- Only registers commands, context actions, automations, or event subscriptions
- Still communicates via the same wire protocol
- Example: a CI/CD integration that posts build results to chat
- Example: an automation that creates YouTrack issues from messages
- Example: a bot that responds to specific message patterns

The plugin host treats UI and headless plugins identically — the only difference is whether
the manifest declares panels.

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

## Plugin UI Hosting

### Iframe Sandbox

Plugin UI panels are hosted in sandboxed iframes served via a Tauri custom protocol
(`rim-plugin://{plugin_id}/{panel_id}/index.html`).

Sandbox attributes: `allow-scripts` only. No `allow-same-origin`, no `allow-top-navigation`,
no `allow-popups`.

### PostMessage Bridge

The host UI listens for `postMessage` events from plugin iframes. Each message is validated
for origin (`rim-plugin://{expected_plugin_id}`) and checked against the plugin's approved
capabilities.

Plugin → Host messages (capability-gated):
- `rim.ui.notification.show` — display toast notification
- `rim.ui.dialog.confirm` — show confirmation dialog
- `rim.ui.navigate.conversation` — switch to a conversation
- `rim.command.execute` — execute a registered command
- `rim.chat.messages.list` — fetch messages (requires `chat.read`)
- `rim.chat.message.send` — send a message (requires `chat.write`)
- `rim.settings.get` / `rim.settings.set` — plugin settings

Host → Plugin messages:
- `theme.changed` — CSS custom properties update
- `panel.visibility` — panel shown/hidden
- `panel.focus` / `panel.blur` — focus state change
- `conversation.changed` — active conversation changed

### Focus Management

- Host owns global keyboard shortcuts (Ctrl+K, Ctrl+1-9) — plugins never intercept these
- Escape always returns focus to host — plugins cannot block this
- When a plugin has focus, its container shows a visual accent border
- Focus state is tracked in `focusedSurface` in the shell store

### Theme Bridge

Host sends CSS custom properties to plugin iframes on mount and on theme change:
- Color tokens: `--rim-void`, `--rim-panel`, `--rim-accent`, `--rim-text`, etc.
- Typography: `--rim-font-sans`
- Border radius: `--rim-radius-sm`, `--rim-radius-md`, `--rim-radius-lg`

### Plugin Crash Handling

If a plugin process crashes or becomes unresponsive (5s timeout):
- Host displays "Plugin not responding" state with Reload and Close buttons
- Plugin panel is replaced with host-rendered error state
- Other plugins and host UI are not affected
