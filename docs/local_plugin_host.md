# Local Plugin Host

This document describes how the RIM desktop app hosts plugins today: who owns what, how a plugin
is installed and started, and how panels are hosted. The process protocol itself is specified in
[`docs/specs/plugin-protocol-v1.md`](specs/plugin-protocol-v1.md), which is the document to read
when writing a plugin.

Plugins are local client extensions, not backend-managed installs. The local plugin host lives in
the desktop app (`rim-ui/src-tauri/src/plugin_host/`). It loads manifests, keeps enable/disable
state, asks the user to approve capabilities, starts plugin processes, and brokers everything a
plugin asks of the app.

## Core Rule

Plugins are local extensions that talk to a local host.

The host:

- loads and validates the manifest
- asks the user to approve capabilities
- launches the plugin process and speaks plugin protocol v1 with it
- gates every command and every plugin-to-host call
- hosts plugin panels in sandboxed iframes

The backend remains a stable API surface for shared app state; it does not own plugin lifecycle.

## Ownership

### Local Plugin Host Owns

- plugin folder discovery (`~/.rim/plugins/`)
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

Each plugin directory holds a manifest, `rim-plugin.json`. The manifest contract and its
install-time validation live in the desktop plugin host, not in the backend:

- `rim-ui/src-tauri/src/plugin_host/manifest.rs` and `validation.rs`
- reference example: `rim-ui/plugins/hello-world/rim-plugin.json`

The manifest uses snake_case keys and describes:

- identity: `manifest_version` (always `1`), `plugin_id` (reverse-domain), `display_name`,
  `version`, `description`, `author`
- runtime: `runtime_kind` (always `"DesktopLocalProcess"`) and `entrypoint`
- `capabilities` the plugin requests
- `commands`, each with `id`, `title`, `description`, `arguments`, `dangerous`,
  `required_capabilities`, `ai_callable`, `output_schema` and `keybind_capable`
- `keybind_suggestions`, `context_actions` and `panels`
- `settings_schema`

A plugin without `rim-plugin.json` can instead put its manifest in comment headers at the top of
a `main.ts`, `main.js` or `main.py` entrypoint ("simple mode", `simple_manifest.rs`):

```text
# rim-plugin: dev.example.my_tool
# rim-name: My Tool
# rim-version: 1.0.0
# rim-description: Does useful things
# rim-command: do_thing | Do Thing | Does a thing
```

Install-time validation rejects reserved plugin id prefixes (`dev.rim.`, `com.rim.`), capability
keys that are not of the form `plugin.<plugin_id>.<verb>`, and `ai_callable` commands whose
`output_schema` could leak a plugin install id. See the protocol spec, section 12, for what that
means for capability names today.

## Runtime

The process protocol is **plugin protocol v1**, specified in
[`docs/specs/plugin-protocol-v1.md`](specs/plugin-protocol-v1.md) and frozen in the JSON Schema
`rim-ui/protocol/plugin-protocol-v1.schema.json`. In short: the host runs the entrypoint, frames
JSON-RPC 2.0 messages over stdin and stdout with a 4-byte little-endian length prefix, handshakes
with `rim.handshake`, runs commands with `rim.command.execute`, and stops the plugin with
`rim.shutdown`. The spec also lists which methods the host implements and which it does not.

Process lifecycle as the host implements it:

- **Start.** A plugin process starts when the user enables the plugin, and at app start for every
  plugin that was enabled when the app last ran. There is no lazy start on first use and no idle
  stop in v1.
- **Crash.** A plugin that exits on its own stays stopped; commands for it fail with
  `PLUGIN_NOT_RUNNING` until the user enables it again. There is no automatic restart.
- **Unresponsive.** A plugin that misses a heartbeat answer for 5 seconds is marked unresponsive
  in the host. The UI does not show this state yet.
- **Stop.** Disabling or uninstalling sends `rim.shutdown`, closes stdin, and escalates to
  `SIGTERM` after 1 second and `SIGKILL` 2 seconds later. App quit does the same for every
  plugin at once and kills whatever is still running after 4 seconds.

UI plugins and headless plugins (no `panels`) run the same way; the only difference is whether the
manifest declares panels.

## Capabilities

Capabilities are local permissions the user grants a plugin over host-managed or backend-brokered
operations. The user approves them locally before the plugin is enabled, and the host checks them
in two places: before running a command whose manifest lists
`required_capabilities`, and on every plugin-to-host call that needs one (with a per-capability
rate limit). The protocol spec, section 12, lists the names and their current state.

## Security Direction

The first plugin host does not blindly trust arbitrary local processes.

The direction is:

- local plugin host
- brokered access to sensitive features
- capability checks at the host boundary
- plugins call backend APIs through the host

Manifest capabilities are not, by themselves, a complete OS sandbox. A local-process plugin runs as
the user and can do anything the user can; the host/broker model governs what the **host** will do
for it, and stronger sandboxing can be layered later. Dangerous commands run only after the user
confirms each run, and the AI agent can never confirm one (protocol spec, section 10).

### Install flow (git installs)

Installing a plugin is the moment untrusted code first reaches the machine, so the host treats
every input to it as hostile (rim-ui findings S-H2 and S-H3, fixed 2026-10-04):

1. **URL and branch validation before git runs.** Only `https://`, `ssh://` and scp-like
   `user@host:owner/repo` URLs are accepted. Remote-helper syntax (`ext::`, `fd::`, anything with
   `::`), `file://`, `git://`, a leading `-`, and non-printable characters are rejected with
   `INVALID_GIT_URL`; branch names use a conservative ref-name allowlist (`INVALID_GIT_BRANCH`).
2. **Hardened git invocation.** Every git child runs with `GIT_ALLOW_PROTOCOL=https:ssh` and
   `GIT_TERMINAL_PROMPT=0`, and every argv puts `--` before positional arguments. Updates use
   `fetch` plus `merge --ff-only -- @{upstream}` rather than `pull`.
3. **Dependency installs need explicit consent.** `npm install` / `pip install` execute code chosen
   by the plugin author with the user's full privileges, so they never run implicitly. Install and
   update first detect which steps would run; without `allowPostInstall: true` the operation stops
   with `POST_INSTALL_CONSENT_REQUIRED` carrying the exact commands and directories, nothing is
   moved into the plugins directory or executed, and a declined update leaves the installed copy
   untouched. The client shows the commands verbatim and retries only on confirm.
4. **Capability approval** follows, as before, before the plugin is enabled.

The implementation lives in `rim-ui/src-tauri/src/plugin_host/` (`git_source.rs`,
`post_install.rs`, `git_install.rs`).

## First Plugin Version Priority

The first real plugin host prioritizes:

- commands first
- local manifest loading
- local command registration
- brokered backend API access
- local capability approval

Panels, context actions, and keybind suggestions follow on top of the same host/runtime model.

## Plugin UI Hosting

### Iframe Sandbox

Plugin UI panels are hosted in sandboxed iframes served via a Tauri custom protocol
(`rim-plugin://{plugin_id}/{panel_id}/index.html`).

Sandbox attributes: `allow-scripts` only. No `allow-same-origin`, no `allow-top-navigation`,
no `allow-popups`.

### PostMessage Bridge

The panel bridge is separate from the process protocol: it is a `postMessage` channel between the
panel iframe and the host UI (`rim-ui/src/features/plugins/runtime/pluginBridge.ts`). It is not
frozen and not part of plugin protocol v1. The host accepts a message only when its origin is
`"null"` (the opaque origin of a sandboxed iframe) and its `event.source` is that plugin's own
iframe, so the plugin id comes from the iframe, never from the message.

Panel → host messages, with what each does today:

| Message | Status |
|---|---|
| `rim.command.execute` | Works. Runs one of the plugin's **own** commands (`plugin.<plugin_id>.` prefix, checked again in the Rust host). A panel cannot confirm a dangerous command. |
| `rim.ui.notification.show` | Gated on `ui.notify`, which cannot be granted yet; only logged. |
| `rim.ui.dialog.confirm` | Only logged. |
| `rim.settings.get` / `rim.settings.set` | Only logged. |
| `rim.plugin.ui_event` | Not wired: the host queues the event but never delivers it to the plugin process. |

Host → panel messages:

| Message | Status |
|---|---|
| `theme.changed` | Sent on mount and on every theme change. |
| `panel.state_update` | Sent on mount with the panel state the plugin last stored. |
| `panel.state_patch`, `panel.visibility`, `panel.focus`, `panel.blur`, `conversation.changed` | Declared in the bridge's types, never sent. |

### Focus Management

- While a panel has focus, its iframe receives the keystrokes; Escape returns focus to the host
  (`rim-ui/src/features/plugins/runtime/pluginFocusManager.ts`)
- Focus state is tracked in `focusedSurface` in the shell store (`useShellStore`)

### Theme Bridge

The host sends the `--color-rim-*` design tokens (`--color-rim-void`, `--color-rim-panel`,
`--color-rim-accent`, `--color-rim-text`, and the rest of the palette) to the panel iframe on mount
and whenever the theme changes, so a panel can match the host.

### Panel Load Failure

If a panel's page fails to load, the host replaces it with a "Failed to load plugin" state with
Retry and Close buttons. Other plugins and the host UI are not affected.
