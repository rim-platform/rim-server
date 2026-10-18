# Plugin Model

This document explains what a RIM plugin is, who owns which part of the plugin system, and how
plugins fit the command, keybind and agent model. It is an overview: the contracts are the specs
it links to, and when this page and a spec disagree, the spec wins.

| Contract | What it defines |
|---|---|
| [`specs/plugin-protocol-v1.md`](specs/plugin-protocol-v1.md) | The protocol between the host and a plugin: transport, framing, handshake, commands, the dangerous-command gate, principals, capabilities, heartbeats, shutdown |
| [`specs/web-plugins-v1.md`](specs/web-plugins-v1.md) | The `WebSandboxed` kind: bundles, the sandboxed frames, the `MessagePort` transport, `network.fetch`, storage, install sources |
| [`specs/workspace-plugins-v1.md`](specs/workspace-plugins-v1.md) | Manifests signed by their publisher and the per-workspace registry the server keeps |
| [`specs/e2ee-mls-v1.md`](specs/e2ee-mls-v1.md), "Plugins and the client message service" | What a plugin may read and write in encrypted conversations |

`local_plugin_host.md` describes how the desktop app installs, starts and hosts local-process
plugins.

## What a plugin is

A plugin is an extension of the client application, in the spirit of VS Code or Obsidian
extensions. It is a package with a manifest (`rim-plugin.json`) that contributes some of:

- commands, which appear in the palette, can be bound to keys and, when the plugin marks them
  `ai_callable`, can be run by the AI agent;
- keybind suggestions, which the user approves;
- context actions on messages;
- panels, rendered in sandboxed iframes inside the shell;
- settings: fields the user fills in, in a form the host builds from the manifest's
  `settings_schema`, and the plugin reads with `rim.settings.get` (protocol v1, section 15).

A plugin with no panels is headless: it contributes commands only. The four official plugins
each have their own repository under [github.com/rim-platform](https://github.com/rim-platform):
[`plugin-k8s-pods`](https://github.com/rim-platform/plugin-k8s-pods) lists the pods of the user's
Kubernetes cluster and restarts one through a dangerous command,
[`plugin-github-prs`](https://github.com/rim-platform/plugin-github-prs) lists pull requests of
public GitHub repositories, [`plugin-releases`](https://github.com/rim-platform/plugin-releases)
posts the latest GitHub releases of chosen repositories into the open conversation, and
[`plugin-poll`](https://github.com/rim-platform/plugin-poll) runs a poll whose votes are messages.
rim-ui's `plugins/` directory holds the starting points, `hello-world` and `template-ts`.

## Two kinds

| Kind | Where it runs | What it can reach |
|---|---|---|
| `DesktopLocalProcess` | A process the desktop app starts, speaking protocol v1 over stdin and stdout | Whatever the user can do on the machine (it runs as the user), plus the host methods its granted capabilities allow |
| `WebSandboxed` | A sandboxed iframe with an opaque origin, in the desktop app and in the web app | Only the host methods its granted capabilities allow, and the network origins its manifest lists when `network.fetch` is granted |

A local-process plugin can be written in any language that reads stdin and writes stdout; the
protocol spec ends with a complete plugin in Python. A web plugin is one self-contained ES module
plus optional panel files, shipped as a bundle whose SHA-256 the client checks at every start.
rim-ui's TypeScript SDK (`sdk/plugin-sdk`) builds both kinds from the same source.

## Who owns what

Plugins run in the client. The server never runs plugin code.

**The client's plugin host owns** installation (from a local folder, a git URL, a pasted bundle
link or a workspace registry entry), manifest validation, the capability consent and its
revocation, starting and stopping plugins, routing commands to them, enforcing the
dangerous-command confirmation, hosting panels, and the settings form and the values the user
saves in it.

**The server owns** what a workspace shares about plugins: the per-workspace registry of
recommended or required plugins, each entry carrying a manifest its publisher signed with their
identity key, and the storage of web plugin bundles for those entries. Owners and moderators
manage the registry; clients verify every entry against the publisher's key log before they offer
it, so they do not have to trust the server for what a plugin is. The registry can be switched
off per instance (`workspace_plugins` in `operations/features.md`). An entry's manifest carries
the plugin's settings schema; the values a person saves never reach the server.

Plugins reach shared data only through the host, with the user's own session and under the
capabilities the user granted. A web plugin's opaque origin has no cookies and cannot read the
CSRF token, so it cannot call the GraphQL API directly.

## Plugins and the command model

Plugins join the shared systems instead of building parallel ones:

- A plugin command is `plugin.<plugin_id>.<command>` in the same registry as builtins and user
  commands, so it shows up in the palette and the keybind picker and can be used in macros
  (`command_model.md`).
- A command the manifest marks `dangerous` runs only after the user confirms that run in a host
  dialog. The agent can never confirm one, and a panel cannot confirm one for its own plugin.
- A command marked `ai_callable` is offered to the agent under the same confirmation gate as
  builtins, so the agent can operate whatever a workspace has plugged in.
- Plugins see a person only as `{ principalId, displayName }`, never a server-local id.

## Security model

A capability is a permission the user grants a plugin over something the host does for it. The
manifest asks for capabilities, the consent prompt names each one, the user approves some or
all, and revoking one stops the plugin. The names and what each guards are in protocol v1,
section 12, and in the web plugin spec's capability table.

Capabilities are not a sandbox for a local process: a `DesktopLocalProcess` plugin runs as the
user and can do anything the user can, and the consent prompt says so. They govern what the
host will do for the plugin. A `WebSandboxed` plugin is confined by the browser: the iframe
sandbox, an opaque origin and a content security policy built from its granted origins.

Decrypted text of encrypted DMs and groups reaches a plugin only through the client message
service and only with the separate `chat.read.encrypted` or `chat.write.encrypted` grant; plugins
never see keys, MLS state or ciphertext.

Settings are the plugin's own data, so reading them needs no capability, as `rim.storage` needs
none; the host gives each plugin only its own and takes the plugin's identity from the
connection. Only the user changes them. They stay on the device: never synced, never sent to the
server, deleted on uninstall. A setting the manifest marks `is_secret` is kept in the desktop's
secret vault and never shown again once saved, and is not available in the web app, which has no
encrypted store for it. A secret is kept from other plugins, from web pages and from the screen,
not from the plugin that declares it, which receives it in plain form and can send it wherever it
can send anything: a web plugin to its granted origins and by the channels its policy does not
close, a local-process plugin anywhere, since it runs as the user and could read the vault itself.
Panels never receive setting values.

## Not built

- Panel slots beyond what the manifest's `slot` names today, a plugin UI component kit, and a
  frozen panel bridge: the bridge between a panel and the host is not part of protocol v1, and
  `local_plugin_host.md` lists which of its messages work.
- Event subscriptions (`rim.events.subscribe`) and automations triggered by events.
- Lazy start on first use and an idle stop for local-process plugins.
- Enforcing required registry plugins, and per-workspace limits on what capabilities a plugin
  may be granted.
- Plugin sync across a person's devices, settings included: each device keeps its own values.
- Host-attached secrets for `network.fetch` (`specs/web-plugins-v1.md`, "Deferred").
