# Web plugins v1

_Status: implemented, normative. Extends `docs/specs/plugin-protocol-v1.md` (read it first:
the messages, the handshake, capabilities and the dangerous-command gate are defined there and do
not change). This document is the contract between the RIM host, on the desktop and on the web,
and plugins of a second kind, `WebSandboxed`, which run in a sandboxed iframe instead of a local
process. `docs/specs/workspace-plugins-v1.md` defines signed manifests and the workspace
registry that web bundles come from._

## Summary

1. A `WebSandboxed` plugin is a bundle: `rim-plugin.json`, one self-contained ES module, optional
   panels; no native code, no Node APIs. The same bundle runs in the web app and the desktop app.
2. It runs in a hidden **plugin frame**, an `<iframe sandbox="allow-scripts">` with an opaque
   origin; each open panel is a separate **panel frame**, and the host relays panel state between
   them, as `PanelRelay` does for local processes.
3. Both load the **frame shell**, a static page shipped with the client, and get the verified files
   by `postMessage`. Its CSP comes from `<meta>` elements, so it never inherits the host page's
   policy, which `srcdoc` and `blob:` documents would.
4. Transport: protocol v1's JSON-RPC 2.0 messages, each a JSON string on a `MessagePort` the host
   transfers once; no length prefix. Handshake, commands, heartbeats, principals, errors and the
   confirmation gate are v1's, unchanged.
5. Shutdown is `rim.shutdown`, one second of grace, then the iframe is removed. A frame that
   navigates itself, floods the host or misses two heartbeats is removed.
6. The plugin reaches RIM only through host methods behind the same capability prompts. An opaque
   origin has no RIM cookies and cannot read the CSRF token, so it cannot call GraphQL.
7. `ui.panel` and `ui.notify` are prompted as on the desktop; `plugin.<id>.<verb>` keys are
   refused at install, as on the desktop. A new `network.fetch` lets the plugin reach the
   origins its manifest lists, enforced by CSP. `local.process`, `fs.*` and `network` are refused.
8. New host methods `rim.storage.get` and `rim.storage.set` (not frozen) give 1 MiB per plugin and
   account, because an opaque origin has no `localStorage` or IndexedDB. The settings the user
   saves arrive as for a local process, through v1's `rim.settings.get` and
   `rim.settings.changed`; secret settings only on the desktop.
9. The desktop installs web plugins like any plugin or from the workspace registry; the web app
   only from a registry entry (signed, `workspace-plugins-v1.md`) or a pasted link carrying the bundle's SHA-256. A
   bundle is one JSON file, hashed again at every start.
10. The SDK gets a transport abstraction and a browser build; `rim-plugin-bundle` writes a
    `.rimbundle` and prints its hash. Showcase: a pull-request panel for public GitHub
    repositories. The Kubernetes plugin stays desktop-only.

## Manifest

Same `rim-plugin.json`, same `manifest_version` 1. What differs for this kind:

| Field | Rule |
|---|---|
| `runtime_kind` | `"WebSandboxed"` |
| `entrypoint` | Bundle path of one ES module ending in `.mjs` or `.js`. Self-contained: no static `import` and no `import()` of bundle files, because the shell loads it from a `blob:` URL where relative specifiers resolve to nothing. No `#!` line. |
| `network_origins` | New, optional. 1 to 16 origins of the form `https://host[:port]` or, for WebSockets, `wss://host[:port]`: lowercase, no path, query, fragment, userinfo or wildcard, no duplicates. Present exactly when `capabilities` lists `network.fetch` (`null` counts as absent, as for the other optional fields that have no default, `author`, `output_schema`, `preset_args` and `slot`; a list field that defaults to empty, such as `capabilities`, refuses `null`). No origin may share the host and port of the app's or the RIM API's origin. |
| `capabilities` | See [Capabilities](#capabilities). |
| `panels` | As today (`id`, `title`, `description`, `slot`). A panel's files live under `panels/<id>/`, so its `id` is one bundle path segment; `index.html` is required. The entrypoint is not under `panels/`. |

`network_origins` and `network.fetch` are refused for `DesktopLocalProcess`. A host without web plugin
support refuses a web plugin at install with `failed to parse manifest: unknown variant
WebSandboxed`, because `RuntimeKind` is a closed enum; it never tries to execute the module.

The showcase's manifest, as installed (the bundler builds its source, `src/main.ts`, into the
`main.mjs` it names). A panel reaches its plugin only through commands, so adding and
removing repositories are commands too; they take an argument, so the palette leaves them
to the panel:

```json
{
  "manifest_version": 1,
  "plugin_id": "chat.rim.github-prs",
  "display_name": "GitHub Pull Requests",
  "version": "0.0.1",
  "author": "RIM team",
  "runtime_kind": "WebSandboxed",
  "entrypoint": "main.mjs",
  "description": "Lists the open pull requests of public GitHub repositories you choose. Talks only to GitHub, without a token.",
  "capabilities": ["ui.panel", "network.fetch"],
  "network_origins": ["https://api.github.com", "https://avatars.githubusercontent.com"],
  "commands": [
    {
      "id": "refresh",
      "title": "Pull requests: Refresh",
      "description": "Fetches the open pull requests again.",
      "keybind_capable": true
    },
    {
      "id": "add_repo",
      "title": "Pull requests: Add repository",
      "description": "Adds a public GitHub repository to the list, at most 10.",
      "arguments": [
        {
          "name": "repo",
          "type": "string",
          "required": true,
          "description": "The repository as owner/name, such as rust-lang/rust."
        }
      ]
    },
    {
      "id": "remove_repo",
      "title": "Pull requests: Remove repository",
      "description": "Removes a repository from the list.",
      "arguments": [
        {
          "name": "repo",
          "type": "string",
          "required": true,
          "description": "The repository as owner/name."
        }
      ]
    }
  ],
  "panels": [
    {
      "id": "prs",
      "title": "Pull requests",
      "description": "Open pull requests of the repositories on your list.",
      "slot": "tool_panel"
    }
  ]
}
```

## Bundles

A bundle is one UTF-8 JSON file, extension `.rimbundle`, media type
`application/vnd.rim.plugin-bundle+json`:

```json
{"format":"rim-plugin-bundle","v":1,"files":[{"path":"main.mjs","text":"…"},{"path":"rim-plugin.json","text":"…"}]}
```

- `format` is `rim-plugin-bundle` and `v` is `1`. Unknown fields at any level are refused.
- `files` has 1 to 256 entries, sorted by `path` in byte order, each path once. An entry has
  `path` and exactly one of `text` (the file's content as a JSON string; the file must be UTF-8)
  or `base64` (standard base64 with padding, for any other file).
- A `path` is 1 to 255 bytes of `/`-separated segments of `A-Z a-z 0-9 . _ -`. No segment is
  empty, `.` or `..`, or starts with `.`.
- `rim-plugin.json` and the manifest's `entrypoint` are present. In a `WebSandboxed` bundle every
  other file is under `panels/`.
- The whole file is at most **8 MiB** (8,388,608 bytes).
- **The bundle hash** is the SHA-256 of the file's bytes exactly as fetched, written as 64
  lowercase hex characters (what `sha256sum` prints, so a publisher can check it with standard
  tools). Readers never re-serialize a bundle. Writers (the SDK bundler and the desktop signer)
  write `files` sorted and the JSON compact, without spaces, so the same files give the same hash.

One JSON file rather than a zip or a tarball: a browser reads it with `JSON.parse`, no archive
library, one hash covers every path and byte, and base64 costs a third more only on binary files.

A bundle reference, in the same words as in `workspace-plugins-v1.md`: **a registry entry names a
web bundle by `bundleSha256` (64 lowercase hex characters, the SHA-256 of the bundle file's bytes),
`bundleSize` (its length in bytes, at most 8 MiB) and `bundleUrl` (where to fetch it; by default
the workspace server's copy, `/api/plugin-bundles/<serverId>/<bundleSha256>`). The SHA-256 and the
size are inside the publisher's signed statement; the URL is not, because it says only where the
bytes are, and the hash decides whether they are the right ones.**

The desktop installs a bundle by unpacking it into `~/.rim/plugins/<plugin_id>/` with the staging
and path checks of a local-folder install (a `DesktopLocalProcess` entrypoint gets mode `0755`,
every other file `0644`), then records its bundle fingerprint as today. A `WebSandboxed` plugin
installed from a folder or a git checkout must hold exactly what a bundle of it would: entries
whose name starts with `.` (`.git`, `.DS_Store`) are skipped, and every other file follows the
rules above.

## Runtime

### Two kinds of frame

- **The plugin frame**, one per enabled plugin, hidden (`display: none`), runs the entry module
  and speaks protocol v1 over the port. It starts when the plugin is enabled and at app start, like
  a process. On the web, where installs belong to an account, signing out stops every plugin frame
  and signing in starts that account's; on the desktop it keeps running and gets
  `rim.principal.changed`, as a process does.
- **A panel frame**, one per open panel, created when the panel is shown and removed when it
  closes, has no port. A panel is shown only for a running plugin that holds `ui.panel`. It
  speaks the existing panel bridge (`docs/local_plugin_host.md`, "PostMessage Bridge") through
  `window.parent`, exactly as a local-process plugin's panel does.

This is the split `panel_relay.rs` already makes: logic apart from views, the host relaying state
between them. Commands work from the palette, keybinds and the agent with no panel open, panels
are written the same way for both kinds, and one SDK serves the logic. **One frame holding both**
was rejected: the plugin would live only while its panel is visible, and panels would differ by
kind. **A Web Worker** for the plugin was rejected: a worker the host creates has the host's
origin, cookies and credentialed `fetch`; only a sandboxed frame gets an opaque origin. (A plugin
may start a worker inside its frame; it inherits the frame's origin and policy.)

### The frame shell

A fixed page in rim-ui, `public/plugin-frame/v1/index.html` and `shell.js`. The web app serves it
from its own origin at `/plugin-frame/v1/`; the desktop serves the same files through
`rim-plugin://shell/v1/index.html` (the authority `shell` cannot collide with a plugin id, which
has at least three dot-separated segments). The frame loads `shell.js` with `crossorigin`, and the
servers send `Access-Control-Allow-Origin: *` for it: WebKit refuses a no-cors load from a
custom scheme into a null-origin frame.

| Option | Verdict |
|---|---|
| `srcdoc` | Rejected: the document inherits the embedding page's CSP, so the web app's `script-src 'self'` blocks the plugin, and loosening the app's policy for plugins weakens the host |
| A `blob:` URL made by the host | Rejected for the same reason: a `blob:` document takes its creator's policy |
| A page served per plugin with a CSP header (by the RIM server or a sandbox domain) | Works, but ties the client's shell to a server route, needs a dynamic header that a static host for the web build cannot set, and a separate domain is one more service for a self-hoster |
| **A static shell with `<meta>` CSP, files delivered by `postMessage`** | **Chosen**: independent of the host page's policy, static, identical on both platforms, and the host verifies every byte before handing it over |

The web server must not send a CSP header on `/plugin-frame/` stricter than the shell's own
(policies intersect); a CSP header on the app's pages, if any, must allow `frame-src 'self'`.

### Iframe attributes

```html
<iframe src="/plugin-frame/v1/index.html" sandbox="allow-scripts" referrerpolicy="no-referrer"
        allow="camera 'none'; microphone 'none'; geolocation 'none'; display-capture 'none';
               clipboard-read 'none'; clipboard-write 'none'; fullscreen 'none'; payment 'none';
               usb 'none'; serial 'none'; hid 'none'; bluetooth 'none'"
        credentialless title="Plugin: GitHub Pull Requests"></iframe>
```

`sandbox` is exactly `allow-scripts`: never `allow-same-origin` (with `allow-scripts` it lets the
frame lift its own sandbox), `allow-top-navigation*`, `allow-popups*`, `allow-modals`,
`allow-forms`, `allow-downloads` or `allow-pointer-lock`. `credentialless`, where the browser
supports it, makes the frame's requests carry no cookies, so a plugin cannot act on a site with the
cookies the user has there.

### Policy

The baseline, a `<meta http-equiv="Content-Security-Policy">` in the shell file:

```
default-src 'none'; script-src 'self' rim-plugin://shell blob: 'wasm-unsafe-eval'; style-src 'self' blob: 'unsafe-inline';
img-src 'self' blob: data: https:; font-src blob: data:; media-src blob: data:; connect-src https: wss:;
worker-src blob:; frame-src 'none'; form-action 'none'; base-uri 'none'
```

`script-src` names `rim-plugin://shell` because WebKit does not match a sandboxed frame's
custom-scheme page to `'self'`; the scheme serves only the shell's own files.

`connect-src` lists `wss:` beside `https:` because the scheme source `https:` does not match a
`wss:` URL, and policies intersect: without it no `wss://` origin a plugin lists could connect.

and the per-plugin policy, a second `<meta>` the shell appends to `<head>` before any plugin file
loads: `connect-src <O>; img-src 'self' blob: data: <O>`, where `<O>` is the granted
`network_origins` joined by spaces, or `'none'` without `network.fetch` (then `img-src 'self'
blob: data:`). Policies intersect, so nothing a plugin runs later can widen the result.
`blob:` scripts and `'wasm-unsafe-eval'` are allowed (there is no `'unsafe-eval'`) because the
shell runs the entry from a `blob:` URL and the boundary is the sandbox, not where a script came
from: a plugin can always build code from data its listed origins send. The signature binds what
ships, not what a plugin downloads later.

### The load rule

The host counts the iframe's `load` events. The first triggers the init message; a panel frame
instead waits for the `{type: "rim.frame.hello", v: 1}` the shell posts to its parent before any
plugin byte runs, and gets its init once, after a hello whose source is that frame. Any later load
means the frame navigated, which only the plugin's own code can do: the host removes the frame at
once, marks the plugin stopped with "the plugin navigated its frame", and does not restart it. The
port was transferred once, so a navigated document never holds it.

## Transport

### Init

On the first `load`, the host posts one message to `iframe.contentWindow` with target origin `"*"`
(an opaque origin cannot be named), transferring the port's other end for a plugin frame:

```js
{ type: "rim.frame.init", v: 1, kind: "plugin", pluginId: "chat.rim.github-prs",
  manifest: "<rim-plugin.json text>", policy: { origins: ["https://api.github.com"] },
  files: [{ path: "main.mjs", type: "text/javascript", bytes: ArrayBuffer }] }
```

For a panel frame `kind` is `"panel"`, `panelId` names the panel, `files` holds every file under
`panels/<panelId>/` with paths relative to it, and no port is transferred. This message is a
structured-clone object; every later message on the port is a JSON string.

The shell ignores messages not from `window.parent` and every init after the first. On init it
appends the per-plugin policy, then for a plugin frame defines `globalThis.rimPluginPort` and
`globalThis.rimPluginManifest` (the manifest text) as non-writable, non-configurable properties and
imports the entry from a `blob:` URL, or for a panel frame renders the panel ([Panels](#panels)).
If the entry fails to load or throws while evaluating, the shell posts `{ type: "rim.frame.error",
v: 1, message }` to `window.parent`, and the host stops the plugin with that message rather than
waiting for the handshake timeout. From a plugin frame's window the host accepts nothing else.

### Messages on the port

- **One JSON-RPC 2.0 message per `postMessage`, as a string of JSON text**, not a structured-clone
  object: the same text as v1's frame payloads, checked against the same schema, with no `Date`,
  `Map`, `ArrayBuffer` or `undefined` that JSON lacks, and a size both sides can compute.
- At most **4 MiB** of UTF-8 per message, both ways; a larger one ends the connection (the frame
  is removed). A message that is not a string, not JSON, or not JSON-RPC is logged and dropped.
- Sections 4 to 15 and 17 of protocol v1 apply unchanged (envelope, ids, handshake and
  negotiation, errors, commands, the confirmation gate, principals, capabilities, heartbeats,
  logging, settings, versioning). This section replaces v1's sections 2 and 3, and Shutdown below
  replaces section 16. `hostVersion` is the app's version. The 10-second handshake timeout counts
  from the init message, so it includes loading the module.
- **Heartbeats** stay as in v1 (every 30 s, 5 s to answer): where the browser gives the frame its
  own process, a hung plugin does not hang the host, and the heartbeat is how the host notices.
  Two misses in a row remove the frame ("not responding"); a frame is cheap to remove, unlike a
  process. While `document.visibilityState` is `hidden` the host sends none, because browsers
  throttle background timers.

### Shutdown

The host sends `rim.shutdown`; for **1 second** it still answers the plugin's requests (so a
`rim.storage.set` in flight lands) and sends nothing else; then it closes its end of the port and
removes the iframe, which ends the plugin's scripts, timers and connections. There is no
escalation and nothing a plugin can do to delay it. App quit, a reload or a closed tab take the
frame with the page and send no `rim.shutdown`, so a plugin writes what must survive as it
changes.

## Host methods

| Method | Capability | For `WebSandboxed` |
|---|---|---|
| `rim.log` | none | As v1; the host keeps at most 120 lines a minute per plugin and drops the rest, with one warning |
| `rim.ui.update_panel` | `ui.panel` | As for a local process (routed, not frozen): `panel_id`, `state`, optional `patch`; 1 MiB per plugin |
| `rim.storage.get`, `rim.storage.set` | none | New, not frozen; below |
| `rim.settings.get`, and `rim.settings.changed` from the host | none | As v1 section 15. In the web app a secret setting never has a value; below |
| `rim.chat.*` | `chat.*` | The client message service, as for a local process (routed, not frozen; `e2ee-mls-v1.md`, "Plugins and the client message service"). Reads need a local message store; until the web app's service reads the browser's encrypted store, it answers them `INTERNAL_ERROR` with `data.reason: "noCache"` |
| `rim.events.subscribe` | `events.read` | `CAPABILITY_DENIED`, as v1 section 18 |
| `rim.process.*`, `rim.fs.*`, `rim.network.*` | `local.process`, `fs.*`, `network` | `CAPABILITY_DENIED`: this kind can never hold them |

**Storage.** An opaque origin has no `localStorage`, IndexedDB or cookies, so the host keeps a
small store per plugin. `rim.storage.get { key }` answers `{ value }` (the stored JSON, or `null`);
`rim.storage.set { key, value }` answers `{ ok: true }`, and `value: null` deletes the key.

- `key` is 1 to 128 characters of `A-Z a-z 0-9 . _ -`; a value is any JSON. At most 1 MiB per
  store (key lengths plus the values' JSON text, in UTF-8 bytes): a `set` past it is
  `INVALID_PARAMS` with `data: { "reason": "quota" }`. With nobody signed in, both answer
  `INVALID_PARAMS` (`nobody is signed in`). 120 calls a minute.
- A store belongs to (plugin id, signed-in account, device). The host names the account by its
  provisional `rimp0_` principal id, which never changes, so the one switch to a key-based id
  keeps the plugin's data. It is not synced, is deleted on uninstall, and is not secret: the host
  keeps it unencrypted (IndexedDB on the web, the app data directory on the desktop), so a plugin
  must not keep tokens in it.
- A local-process host answers both with `METHOD_NOT_FOUND`: a process has its own files.

**Settings and storage.** Settings (protocol v1, section 15) are scoped and kept like a store: per
(plugin id, signed-in account, device), never synced, deleted on uninstall. What differs:

| | `rim.storage` | Settings |
|---|---|---|
| Who writes | The plugin, with `rim.storage.set` | Only the user, in the settings form; the plugin has no method to write |
| Shape | Any key the plugin picks, any JSON | The keys and types of the manifest's `settings_schema`, checked when the user saves |
| How the plugin reads | One key per `rim.storage.get` | All of them in one `rim.settings.get`, and again in `rim.settings.changed` after every save |
| Secrets | Never: kept unencrypted, so no tokens | `is_secret` settings, on the desktop only, in the secret vault; the web app has no encrypted store for them, so its form shows them as needing the desktop app |
| Local-process plugins | `METHOD_NOT_FOUND` | Served the same way |

A web plugin running on the desktop gets its secret settings in plain form, like any plugin. Its
policy limits where it can send them to its granted `network_origins`, and the channels in
[Security](#security) that the policy does not close remain. Host-attached secrets, which the
host would add to the plugin's requests without the plugin seeing them, are
[deferred](#deferred).

## Panels

The shell renders a panel's `index.html`: it parses it with `DOMParser`; replaces every relative
URL in `script[src]`, `link[rel~=stylesheet]`, `link[rel~=icon]`, `img`, `source`, `audio` and
`video` that names a file of the panel with a `blob:` URL of that file (media type from the
extension), and turns inline `<script>`s into `blob:` scripts too; swaps in the panel's `<head>` and
`<body>`; recreates the scripts in document order with `async = false`; then dispatches
`DOMContentLoaded` and `load`, and posts `{ type: "rim.frame.ready", v: 1 }` to `window.parent`, on
which the host sends `theme.changed` and the panel's stored state (sent earlier, they would
reach no listener yet). A relative URL that names no file of the panel is removed rather than
resolved against the shell's URL, and so are `<base>` and `<meta http-equiv="refresh">`.
Absolute URLs stay and fall under the policy. Not supported: module
scripts that import other files (bundle them), CSS `url()` and `@import` of panel files (use
`data:` URIs), and inline event handler attributes, which the policy forbids. The k8s panel's shape
(an `index.html` with a `<style>` block and `<script src="app.js">`) works unchanged.

The bridge is unchanged: the host accepts a panel's message only with origin `"null"` and
`event.source` that panel's iframe, takes the plugin id from the iframe, lets `rim.command.execute`
run only the plugin's own commands, and sends `theme.changed` and `panel.state_update`. A panel
frame gets its plugin's policy, so it can show images from the listed origins. The TypeScript relay
keeps `PanelRelay`'s semantics (replace or shallow merge, 1 MiB per plugin, `state: null` to open
panels when the plugin stops). Focus and Escape work as for other panels.

## Capabilities

| Capability | `WebSandboxed` | Notes |
|---|---|---|
| `ui.panel` | Prompted | As on the desktop |
| `plugin.<pluginId>.<verb>` | Refused at install | As on the desktop: the prompt lists only built-in names and could never approve one, so neither `capabilities` nor a command's `required_capabilities` may name one |
| `ui.notify` | Prompted | Panel notifications show the plugin's name, as today |
| `network.fetch` | Prompted (new) | Lists the origins; this kind only; enforced by the policy |
| `chat.read`, `chat.write`, `chat.read.encrypted`, `chat.write.encrypted` | Prompted | As on the desktop, one line each; an encrypted one without its plain one is refused at install |
| `events.read` | Refused at install | As on the desktop: nothing serves it |
| `local.process`, `fs.read`, `fs.write` | Refused, always | `WebSandboxed plugins cannot run programs or read files; use DesktopLocalProcess` |
| `network` | Refused, always | Names host-brokered requests for local processes |

`rim.log`, `rim.storage.*` and `rim.settings.get` need no capability, since each reaches only the
plugin's own data; nothing else is granted without a prompt.
`network.fetch` is a new name rather than `network` because the enforcement differs (a browser
policy on the plugin's own requests, not a host method), and per-origin rather than broad because
the policy can enforce origins and a prompt that names hosts is one a person can judge. It is one
checkbox for all listed origins; without it the frame runs with `connect-src 'none'`. The policy is
fixed when the frame loads, so the host restarts the frame when grants change. Revoking any
capability stops the plugin, as on the desktop.

**Consent text.** Above the capability list, in the app's words and apart from the author's
description, the prompt says what kind of plugin this is. Both lines are in
`PluginCapabilityPrompt`:

- `WebSandboxed`: "Runs sandboxed in this app. It cannot read your files, run programs, or use
  your RIM session. It reaches RIM only through the permissions below."
- `DesktopLocalProcess`: "Runs as a program on this computer with your account's access. It is not
  sandboxed: it can read your files and use the network whatever you approve below. Install it
  only if you trust its author."

`network.fetch` reads "Connect to listed sites: send and receive data from api.github.com,
avatars.githubusercontent.com. Anything you show this plugin can be sent to these sites." (hosts
without `https://` or a default port).

## Where bundles come from

| Source | Desktop | Web | Signed |
|---|---|---|---|
| Local folder (the bundler's output directory) | Yes, as today | No | No: fingerprint on first use |
| Git URL | Yes; no post-install steps for this kind (`web plugins are installed built`) | No | No: fingerprint on first use |
| Workspace registry entry | Yes | Yes | Yes |
| Pasted link | Yes | Yes | No |

**Pasted links.** `https://host/path/name.rimbundle#sha256=<64 lowercase hex>`. A link without
the fragment, or with another scheme, is refused. The host fetches it with `credentials: "omit"`,
`cache: "no-store"`, following redirects, and stops reading past 8 MiB; the server must answer with
`Access-Control-Allow-Origin: *`. The bytes must hash to the fragment's value, and the install then
goes through manifest validation and consent like any other. Settings shows such a plugin as
"Unsigned · installed from a link (host)". It is never updated automatically; a new link installs
over it after showing what changed (version, capabilities).

**Registry entries.** A client verifies a registry entry, in the same words as in
`workspace-plugins-v1.md`:

> A client verifies a registry entry before it offers to install it, and again before it installs
> or updates, without trusting the server:
>
> 1. Read the statement: `type` `plugin-manifest`, `v` 2, payload at most 96 KiB, no unknown or
>    duplicate fields (a browser checks duplicates with its own scan, since `JSON.parse` keeps the
>    last one).
> 2. Get the publisher's key log: `keyLog(principalId)` from the workspace's server or, when that
>    answers `[]`, the entry's `publisherKeyLog`. Replay it from the inception; its principal id
>    must be the statement's `principalId`.
> 3. `keySeq` must be an entry of the log, and no later entry may be a recovery rotation
>    (`"signer":"recovery"`). The signature must verify over `rim.plugin-manifest.v1\0` followed by
>    the payload, under that entry's `identityKey`.
> 4. The `manifest` text must be a valid manifest for this host, with `plugin_id` equal to
>    `pluginId` and `version` equal to `version`. A `WebSandboxed` manifest must come with a
>    `bundle` source.
> 5. The fetched bundle must have exactly `size` bytes and the SHA-256 `sha256`, and its
>    `rim-plugin.json` must be byte-identical to `manifest`. For a `git` source (desktop only), the
>    checkout of `commit` must have the tree hash `treeSha256`.
> 6. Only then does the client show the publisher, the trust badge and the capability consent.

**Caching and updates.** The web app keeps verified bundles in IndexedDB (database `rim-plugins`,
store `bundles`, keyed by hash) and install records per account (store `installs`), and hashes the
cached bytes again at every start; a mismatch or an evicted bundle is fetched again from its entry
or link and must hash the same, or the plugin does not start ("bundle damaged or unavailable"). The
desktop keeps the unpacked directory and checks its fingerprint at enable, as today. Registry
installs update when the entry's statement changes; link installs only by a new link.

## Host implementation (rim-ui)

- `src/features/plugins/web/`: `webPluginHost.ts` (frames, the JSON-RPC session, gates, limits,
  heartbeats), `webPanelRelay.ts`, `bundle.ts`, `manifestRules.ts` (the TypeScript port of the
  manifest checks and `validation.rs`), and a `WebPluginStore` interface backed by IndexedDB on the
  web and by Tauri commands on the existing `PluginStateStore` on the desktop, so Settings lists
  both kinds together. `dispatchCommand` routes `plugin.<pluginId>.<command>` by runtime kind.
- The command gates run in TypeScript in the order of `command_exec.rs`: for an agent run, AI
  consent first (on the desktop the Rust host's own check); then the arguments are an object,
  the command is declared, a panel's request names its own plugin, the dangerous-command
  confirmation (a host dialog; only the host sets `confirmed`, and the agent never confirms),
  required capabilities, the plugin running; then 30 seconds to answer. Refusals use the
  desktop's error kinds (`UNKNOWN_COMMAND`, `CONFIRMATION_REQUIRED`, `PLUGIN_NOT_RUNNING`, …).
- A new runtime capability, `webPlugins` (on for desktop and web, off for mobile); `pluginHost`
  keeps meaning local processes. On the web, Settings > Plugins lists web plugins, the workspace
  registry and "Install from link", and shows local-process entries as "Needs the desktop app".
- **Safe start.** The host records a `starting` marker before starting a plugin and clears it after
  the handshake. A plugin whose marker is still set at load (the page died while starting it) is
  not started, and Settings says so.
- The web app derives the provisional `rimp0_` id exactly as `protocol.rs` does and keeps the
  `rimp0_` to `rimp1_` mappings per device in IndexedDB.
- Install records keep their source as `{ kind: "local" | "git" | "link" | "registry", … }`: on
  the desktop in the plugin's `PluginRecord`, on the web in the `installs` store.

## SDK

- **Transport.** `Session` works on JSON text only, and framing moves into the stdio transport:
  `interface Transport { readonly messages: AsyncIterable<string>; send(text: string): void;
  close(): void }`, with `stdioTransport()` (today's behaviour) and `portTransport(port)`. `run()`
  uses the port when `globalThis.rimPluginPort` exists, taking the manifest check from
  `globalThis.rimPluginManifest` instead of the disk; `RunOptions.transport` overrides it.
- **Browser build.** `exports` gains a `browser` condition, `src/index.browser.ts`, with no `node:`
  imports and the same API. `host.storage.get`/`set` exist in both builds and reject with
  `METHOD_NOT_FOUND` on a local-process host.
- **Bundler.** For `WebSandboxed`, `rim-plugin-bundle` builds with `platform: "browser"`,
  `conditions: ["browser"]`, `format: "esm"`, `target: "es2022"` and no `#!` banner, fails when the
  output still has an `import` or a `node:` reference, and refuses files outside `panels/`. For
  both kinds it writes the directory as today plus `<plugin_id>-<version>.rimbundle` beside it
  (not inside, so the directory stays installable as a folder), and prints
  `sha256 <hex>  size <bytes>`.
- **Testing.** `startTestHost` gains `transport: "port"`: the plugin runs over a Node
  `MessageChannel` with the same assertions as over stdio.

**Showcase: `plugin-github-prs`** (`chat.rim.github-prs`, the manifest above), written with the SDK
in [its own repository](https://github.com/rim-platform/plugin-github-prs). Its panel lists the open
pull requests (title, author, age, draft) of up to 10 public repositories the user adds, kept with
`rim.storage`; `refresh` is keybind-capable. It calls the GitHub REST API without a token (60
requests an hour per IP address, so it refreshes at most every 2 minutes and says when GitHub
refuses). It exercises both frames, `network.fetch`, storage, the panel bridge and, on the web, a
registry install. [`plugin-k8s-pods`](https://github.com/rim-platform/plugin-k8s-pods) stays
`DesktopLocalProcess`: it runs `kubectl`.

## A web plugin without the SDK

`rim-plugin.json`:

```json
{"manifest_version":1,"plugin_id":"dev.example.hello-web","display_name":"Hello Web","version":"0.1.0","description":"Says hello from a sandboxed frame.","runtime_kind":"WebSandboxed","entrypoint":"main.mjs","commands":[{"id":"greet","title":"Hello Web: Greet","description":"Greets the signed-in user."}]}
```

`main.mjs` (it has no `rim.shutdown` case: removal of the frame stops it):

```js
const port = globalThis.rimPluginPort;
const ID = "dev.example.hello-web";
const send = (m) => port.postMessage(JSON.stringify({ jsonrpc: "2.0", ...m }));
let who = null;
port.onmessage = (event) => {
  let m;
  try { m = JSON.parse(event.data); } catch { return; }
  const p = m.params ?? {};
  switch (m.method) {
    case "rim.handshake": {
      const ok = p.supportedVersions?.includes(1) && p.pluginId === ID;
      who = p.principal;
      send({ id: m.id, result: { accepted: ok, protocolVersion: 1, pluginId: ID } });
      break;
    }
    case "rim.heartbeat":
      send({ id: m.id, result: {} });
      break;
    case "rim.principal.changed":
      who = p.principal;
      break;
    case "rim.command.execute":
      if (p.commandId === "greet") send({ id: m.id, result: { payload: `Hello, ${who?.displayName ?? "world"}!` } });
      else send({ id: m.id, error: { code: -32602, message: `unknown command: ${p.commandId}` } });
      break;
    default:
      if (m.id != null && m.method) send({ id: m.id, error: { code: -32601, message: `method not found: ${m.method}` } });
  }
};
```

Messages wait in the port's queue until `onmessage` is set, so the handshake the host sent while
the module loaded is not lost.

## Security

| Threat | Stopped? | How, or why not |
|---|---|---|
| Reading the host page, the RIM session cookie, the CSRF token, device keys | Yes | Opaque origin, no `allow-same-origin`. Device keys are non-extractable `CryptoKey`s in the host origin's IndexedDB. |
| Calling GraphQL as the user | Yes | A request from an opaque origin is cross-site, so the `SameSite=Lax` session cookie is not sent; in the desktop app, whose cookies are `SameSite=None` over HTTPS, the server refuses the `null` origin with 400; the frame cannot read the CSRF token; the RIM origins are never in its policy. |
| Reading files, running programs | Yes | The browser |
| Reading another plugin's data | Yes | Each frame has its own opaque origin; the host scopes panel state, storage and settings by the plugin id it takes from the iframe or the port, never from a message. |
| Drawing over the host, clickjacking host controls | Yes | The iframe's box. Host dialogs render above frames, and frames are `inert` while one is open. |
| Faking host UI inside its own box | No | A plugin can draw a fake RIM dialog or password form in its box. Rules: RIM never asks for a password, recovery phrase or confirmation inside a plugin's area; host chrome around each panel shows the plugin's name and trust badge; dangerous confirmations are host dialogs naming the plugin. |
| Sending data to unlisted origins with `fetch`, XHR, WebSocket, images, fonts | Yes | The policy |
| Leaking data by other channels | No | A frame can navigate itself to any URL (the load rule removes it after the request left); DNS prefetch and timing are outside CSP; the shell removes WebRTC (rule 2). Anything shown to a web plugin should be treated as available to its author, which the `network.fetch` description says. |
| Passing on its own secret settings | No | The plugin that declares a secret setting receives it in plain form (on the desktop; the web app keeps none) and can send it to its listed origins and by the channels above. A secret setting is kept from other plugins and web pages, not from the plugin that owns it. Host-attached secrets are deferred. |
| Using the user's cookies on a listed site | Partly | `credentialless` where supported, otherwise the browser's third-party cookie rules |
| CPU and memory exhaustion | Partly | Where the browser gives the frame its own process, missed heartbeats remove it. Where the frame shares the host's thread (WebKit-based webviews), a busy loop freezes the app until reload, and safe start keeps it from starting again. |
| Flooding the host with messages | Yes | [Limits](#limits) |

Rules:

1. `sandbox` is exactly `allow-scripts`.
2. The shell appends the per-plugin policy before any plugin byte runs, and sets
   `RTCPeerConnection`, `webkitRTCPeerConnection`, `RTCSessionDescription`, `RTCIceCandidate`
   and `RTCDataChannel` to `undefined`, non-writable and non-configurable: ICE gathering reaches
   any STUN or TURN server, past `connect-src`.
3. The host never puts the RIM API origin, the web app's origin, `http:` or a wildcard into a
   frame's policy. A manifest whose `network_origins` names the app's or the API's origin is
   refused at install.
4. A window message is accepted only with origin `"null"` and `event.source` the expected iframe's
   `contentWindow`; the plugin id comes from the iframe.
5. The port is transferred once, on the first `load`; a second `load` removes the frame.
6. Every request is checked against capabilities and limits before it is dispatched.
7. Bundle bytes are hashed before they reach the shell, at install and at every start.
8. Text from a plugin (names, descriptions, messages, notifications) is rendered as text, never as
   HTML, and marked as the plugin's.

## Limits

| What | Limit |
|---|---|
| Bundle | 8 MiB, 256 files |
| Message on the port | 4 MiB |
| Plugin-to-host messages | 600 a minute per plugin: a request over it gets `RATE_LIMITED`, a notification is dropped. More than 1,200 in a minute stops the plugin. |
| Per capability | 120 calls a minute (v1) |
| `rim.log` | 120 lines a minute |
| `rim.storage.*` | 1 MiB per store, 120 calls a minute |
| Panel state | 1 MiB per plugin |
| Panel bridge messages | 600 a minute per panel frame, the rest dropped |
| Handshake / command | 10 s from init / 30 s |
| Heartbeat | Every 30 s, 5 s to answer, frame removed after 2 misses |
| `network_origins` | 16 |

## Test plan (rim-ui)

- **Bundles.** Appendix A byte for byte; refused: unsorted or duplicate paths, `..`, a dot
  segment, both `text` and `base64`, no manifest or entrypoint, a web bundle file outside
  `panels/`, 8 MiB plus one byte, unknown fields.
- **Manifests.** A shared `protocol/manifest-vectors.json` of valid and invalid manifests (both
  kinds; `network_origins` with `http:`, `ws:`, a path, a wildcard, uppercase, 17 origins, the
  API's origin), run by `validation.rs` and `manifestRules.ts` with identical verdicts.
- **Host.** `webPluginHost` with a fake frame and a real `MessageChannel`: host messages valid
  against `plugin-protocol-v1.schema.json`; handshake timeout from init; `rim.frame.error`; a second
  `load`; a non-string message; 4 MiB plus one byte; capability denial and limits; two missed
  heartbeats; none while hidden; shutdown grace; the gates in order; storage quota and scoping;
  `rim.settings.get` returning saved values and defaults, `rim.settings.changed` after a save,
  no secret value in the web app; safe start. **Conformance:** the template's browser build over
  `portTransport` against the host's session code in Node, through every v1 message, as the stdio
  conformance tests do.
- **Shell** (jsdom): the policy `<meta>` precedes the entry import; panel rendering rewrites
  relative URLs, runs scripts in order, dispatches the load events; messages not from
  `window.parent` are ignored. **Consent:** each kind's line; `network.fetch` lists the hosts.
- **Manual, in Chromium, Firefox and the Tauri webview:** the showcase from a registry entry on the
  web and from a folder on the desktop; a `fetch` to an unlisted origin fails with a CSP error; a
  frame that sets `location` is removed; a panel's `alert` does nothing.

## Deferred

- Brokered secrets: the host attaching a secret setting to a `network.fetch` request (a GitHub
  token for private repositories) so that the plugin never holds it. Until then the plugin
  receives its secret settings in plain form.
- Secret settings in the web app, which needs an encrypted store for them first.
- Wildcard origins and an "any origin" grant.
- `rim.ui.show_notification` from the plugin frame; today only panels notify.
- Installing a `.rimbundle` file on the web, and live reload for developers.
- Clipboard writes from panels.
- Signed links: a link that carries a signed statement outside any registry.
- Web plugins on mobile, with the PWA.
- The chat and event methods in v1, for both kinds.
- CPU limits per plugin, and a browser test suite in CI.

## Appendix A: known-answer bundle

The bundle of the plugin in [A web plugin without the SDK](#a-web-plugin-without-the-sdk):
`main.mjs` is the listing above with a final newline (1,101 bytes), and `rim-plugin.json` is the
manifest above, one line with no final newline (306 bytes). Computed with Python's `json` and
`hashlib`, checked with `sha256sum`.

| Item | Value |
|---|---|
| Bundle (`files` sorted: `main.mjs`, `rim-plugin.json`; compact JSON) | 1,605 bytes |
| SHA-256 | `5b154b3eb45c3ebc44f6820ba30a389ce85b43dea34bcbf652b9b1a5416fbe85` |

The bundle file is `{"format":"rim-plugin-bundle","v":1,"files":[{"path":"main.mjs","text":` +
the JSON string of `main.mjs` + `},{"path":"rim-plugin.json","text":` + the JSON string of the
manifest + `}]}`, where a JSON string escapes only `"`, `\` and control characters (`\n` for a line
feed), as `JSON.stringify` does. `workspace-plugins-v1.md`, appendix A, signs this bundle.

### Browser installation from a pinned link

For an external bundle, the web app posts `{url, sha256}` to
`POST /api/plugin-bundles/fetch` on its configured RIM server, with the session cookie and
CSRF token. The server checks the lowercase SHA-256 pin and bundle structure before returning
bytes. The browser checks the pin again. This route needs `plugin_link_fetch`, enabled by
default and independent of `workspace_plugins`; it stores no bundle in a workspace.

Only HTTPS URLs without credentials or fragments are accepted. Every connection uses a
public-address resolver, with no proxy, including redirect targets. IP literals are checked
separately. Redirects remain HTTPS and stop after ten; downloads stop at 8 MiB or 120 seconds.
Each account has twenty attempts an hour; four downloads run concurrently, with a five-second
slot wait. Audit events record the account and original host, without paths, query strings or
bundle contents. Responses are authenticated attachments with `no-store` and `nosniff`.
An older server may return 400/404 or treat the path as a workspace upload; clients explain
that an update is needed. The disabled feature returns FEATURE_DISABLED, invalid pins or
bundles return 422, and exhausted attempt budgets return 429.
