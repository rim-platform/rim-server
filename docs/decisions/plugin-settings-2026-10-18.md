> **Status:** Accepted 2026-10-18 by Jakub. Implementation follows in rim-ui's plugin hosts and SDK.
> **Do not edit** once implementation lands — historical record. For changes, write a new ADR.

# Plugin settings: declared in the manifest, kept on the device, read by the plugin

**Date:** 2026-10-18
**Decided by:** Jakub
**Builds on:** `docs/specs/plugin-protocol-v1.md`, `docs/specs/web-plugins-v1.md`,
`docs/specs/workspace-plugins-v1.md`.

## Context

A plugin manifest could already declare `settings_schema`, a list of `{ key, title, description,
value_type, is_secret }`, and the rim-ui hosts parsed it, but nothing read it: no form was built
from it, `value_type` was any string, and the panel bridge only logged `rim.settings.get` and
`rim.settings.set`. Protocol v1 listed both under "Not implemented", the web plugin spec deferred
`settings_schema` together with brokered secrets, and the capability model kept a "potential
later split" into `settings.read` and `settings.write`.

Plugins needed settings all the same. `plugin-github-prs` can show only public repositories
because it has nowhere to take a token from, and `plugin-k8s-pods` picks its cluster context and
namespace through a command of its own. Without settings each plugin invents such a command and
keeps the result in its own files or in `rim.storage`, which is unencrypted and not meant for
tokens.

What constrains the answer:

- Plugins run in the client and the server stores nothing per user per plugin
  (`plugin_model.md`, "Who owns what").
- The desktop has an encrypted secret vault; the web app has none, which is why AI provider keys
  are desktop-only.
- A local-process plugin runs as the user and can read whatever the user can, the vault file
  included (`plugin-protocol-v1.md`, section 12).
- Protocol v1 is frozen: a new method may be added within v1, but a new field in a frozen frame,
  such as the handshake, forces v2 (section 17).

## Decision

### 1. The schema

Each `settings_schema` entry has `key` (1 to 128 characters of `A-Z a-z 0-9 . _ -`, the alphabet
of a `rim.storage` key, unique within the manifest), `title`, `description` (default `""`),
`value_type` (`string`, `boolean`, `integer`, `path` or `enum`) and `is_secret` (default `false`,
allowed only with `string`), and optionally `default` (of the setting's type, not allowed on a
secret), `min` and `max` (`integer` only), `options` (`enum` only, required there, a non-empty
list of strings that holds the `default`) and `placeholder` (`string`, `path` and `integer`). A
host refuses a manifest that breaks these rules at install.

The optional fields an entry leaves out are left out of the manifest's content hash, so existing
manifests keep their hash. Adding or changing an entry changes it, and for a plugin with
`ai_callable` commands that asks for AI consent again, as any manifest change does.

### 2. Storage

Values live only on the device, in the client's plugin host, scoped like `rim.storage` to
(plugin id, signed-in account, device). They are never synced and never sent to the server, and
uninstalling deletes them. A workspace registry entry carries the manifest, and so the schema,
but never values. On the desktop non-secret values go in the host's per-plugin store and secret
values in the secret vault. In the web app non-secret values go in browser storage, and secret
settings are not available: the form says they need the desktop app. A saved secret is never
shown again; the form says it is set and offers to replace or clear it.

### 3. Delivery

Two methods are added within v1, served by both host kinds:

- `rim.settings.get`, a plugin-to-host request without params, answered with
  `{ "values": { <key>: <value> } }`: every declared setting that has a saved value or a
  default, secrets in plain form.
- `rim.settings.changed`, a host-to-plugin notification with the same `{ "values" }`, the whole
  set, after the user saves.

Neither needs a capability, and there is no method to write settings: the user is the only
writer. The host takes the plugin's identity from the connection, never from a message field.
Values are checked against the schema when the user saves; after an update a plugin can still
receive values saved under an older schema and handles missing and unknown keys.

### 4. Secrets reach the plugin in plain form

A web plugin receives its secret settings in plain form, and its policy limits where it can send
them to its granted `network_origins`. A local-process plugin receives them in plain form too: it
could read the vault anyway. Secret settings protect a value from other plugins, from web pages
and from the screen, not from the plugin that declares it. Host-attached secrets for
`network.fetch` stay deferred as a later, stronger option.

### 5. Panels

Panels never receive setting values. The panel bridge gains `rim.settings.open`, which opens the
owning plugin's settings in the plugin manager, in place of the logged `rim.settings.get` and
`rim.settings.set`.

### 6. No settings capability

There is no `settings.read` or `settings.write`. Settings are the plugin's own data, as
`rim.storage` is, and the "potential later split" note is replaced by this reasoning.

## Rejected alternatives

- **Brokered secrets now.** The host would keep a secret and attach it to the plugin's
  `network.fetch` requests (for example as an `Authorization` header to one listed origin), so a
  web plugin never holds it. It is the stronger design for web plugins, but it needs a request
  path through the host instead of the plugin's own `fetch`, rules for which header goes to which
  origin, and a per-secret consent line; it gives a local-process plugin nothing, since that
  plugin can read the vault. Plain delivery serves both kinds today, and brokering can come later
  without changing the schema.
- **A `settings.read` / `settings.write` capability.** A plugin reading its own settings exposes
  nothing the user has not typed in for that plugin, and a plugin cannot write them at all. A
  prompt line for it would guard nothing and teach people to approve lines without reading
  them.
- **Settings synced through the server.** One value on every device would be convenient, but the
  server would hold per-user plugin data for the first time, secrets would need end-to-end
  encryption between a person's devices, and values such as a path or a cluster context are often
  different on each device anyway. Plugin sync across devices stays not built
  (`plugin_model.md`, "Not built").
- **Secret settings in the web app.** The browser has no store a token can sit in safely: the
  web app's IndexedDB is readable by any script in the app's origin, and wrapping the value under
  a non-extractable key protects it only against someone reading the disk. AI provider keys are
  desktop-only for the same reason. The form shows a secret field as needing the desktop app.

## Consequences

- rim-ui implements the schema rules in `validation.rs` and `manifestRules.ts`, with shared
  vectors in `protocol/manifest-vectors.json`, the two methods in both hosts and the protocol
  schema, the settings form in the plugin manager, `rim.settings.open` in the panel bridge, and
  the SDK's read side. Its manifest reference page describes the new fields.
- A manifest that declares a `value_type` outside the five, or breaks another rule, no longer
  installs. None of the official plugins declares a setting, so they are not affected.
- `plugin-github-prs` can take a token for private repositories, which it then holds in plain
  form inside its sandbox, where its policy lets it connect only to the GitHub origins it lists.
- On the desktop a plugin's secret settings are as safe as the vault: a program running as the
  user can read them, as it can every other secret there.
- A web plugin with secret settings is only partly usable in the web app. Its manifest cannot
  say "desktop only", so the plugin has to work, or explain itself, without them.
