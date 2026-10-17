# Local Plugin Host

This document describes how the RIM desktop app hosts `DesktopLocalProcess` plugins: where they
live, how they are installed and started, and how their panels are hosted. `plugin_model.md`
explains the plugin model as a whole. The process protocol is specified in
[`specs/plugin-protocol-v1.md`](specs/plugin-protocol-v1.md), the document to read when writing a
plugin; `WebSandboxed` plugins, which the desktop app hosts too, are specified in
[`specs/web-plugins-v1.md`](specs/web-plugins-v1.md).

The host lives in the desktop app (`rim-ui/src-tauri/src/plugin_host/`). It finds plugins in
`~/.rim/plugins/`, installs, enables, disables and uninstalls them, validates manifests, asks the
user to approve capabilities, starts plugin processes and speaks protocol v1 with them, gates
every command and every plugin-to-host call, and hosts panels in sandboxed iframes. The server
takes no part in any of this; it only keeps workspace plugin registries
(`specs/workspace-plugins-v1.md`).

## Manifest

Each plugin directory holds a manifest, `rim-plugin.json`. The manifest contract and its
install-time validation live in the desktop plugin host, not in the backend:

- `rim-ui/src-tauri/src/plugin_host/manifest.rs` and `validation.rs`
- reference example: `rim-ui/plugins/hello-world/rim-plugin.json`

The manifest uses snake_case keys and describes:

- identity: `manifest_version` (always `1`), `plugin_id` (reverse-domain: at least three
  dot-separated segments of ASCII letters, digits and hyphens), `display_name`, `version`,
  `description`, `author`
- runtime: `runtime_kind` (`"DesktopLocalProcess"` here; `"WebSandboxed"` is the web plugin
  kind) and `entrypoint`
- `capabilities` the plugin requests
- `commands`, each with `id`, `title`, `description`, `arguments`, `dangerous`,
  `required_capabilities`, `ai_callable`, `output_schema` and `keybind_capable`
- `keybind_suggestions`, `context_actions` and `panels`
- `settings_schema`

A plugin without `rim-plugin.json` can instead put its manifest in comment headers at the top of
a `main.ts`, `main.js` or `main.py` entrypoint ("simple mode", `simple_manifest.rs`):

```text
# rim-plugin: dev.example.my-tool
# rim-name: My Tool
# rim-version: 1.0.0
# rim-description: Does useful things
# rim-command: do_thing | Do Thing | Does a thing
```

Install-time validation rejects reserved plugin id prefixes (`dev.rim.`, `com.rim.`), capability
names other than the plugin's own `plugin.<plugin_id>.<verb>` keys and the built-in names the host
accepts, and `ai_callable` commands whose `output_schema` could leak a plugin install id. The
protocol spec, section 12, lists which names each host version accepts.

## Process lifecycle

A plugin process starts when the user enables the plugin, and at app start for every plugin that
was enabled when the app last ran. The connection lifecycle, heartbeats and the shutdown sequence
with its timeouts are protocol v1, sections 6, 13 and 15. Specific to this host:

- A plugin that exits on its own stays stopped; commands for it fail with `PLUGIN_NOT_RUNNING`
  until the user enables it again.
- A plugin marked unresponsive after a missed heartbeat is not shown as such in the UI yet.
- UI plugins and headless plugins (no `panels`) run the same way.

## Capabilities and trust

The user approves a plugin's capabilities before it is enabled; the host checks them before
running a command whose manifest lists `required_capabilities` and on every plugin-to-host call
(protocol v1, section 12). A local-process plugin runs as the user and can do anything the user
can: capabilities govern what the host does for it, not what the process can do on the machine.
Dangerous commands run only after the user confirms each run, and the AI agent can never confirm
one (protocol v1, section 10).

## Install flow (git installs)

Installing a plugin is the moment untrusted code first reaches the machine, so the host treats
every input to it as hostile:

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
4. **Capability approval** follows before the plugin is enabled.

The implementation lives in `rim-ui/src-tauri/src/plugin_host/` (`git_source.rs`,
`post_install.rs`, `git_install.rs`).

## Plugin UI Hosting

### Iframe Sandbox

Plugin UI panels are hosted in sandboxed iframes that load the frame shell web plugins use
(see `docs/specs/web-plugins-v1.md`), at `rim-plugin://shell/v1/local-panel.html`. The desktop
host reads the panel folder `panels/{panel_id}/` of an enabled plugin that declares that panel
(the `plugin_panel_files` command) and hands the files to the shell, which runs them as `blob:`
URLs; the scheme serves no plugin file itself. `local-panel.html` is sent with a policy header
that closes the network (`connect-src 'none'`, images, fonts and media only from `blob:`/`data:`):
a local panel reaches the network only through its plugin's process. Only the files `index.html`
names are mapped to the panel's own files; inline event handlers do not run.

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
| `rim.ui.notification.show` | Gated on `ui.notify`; only logged. |
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
