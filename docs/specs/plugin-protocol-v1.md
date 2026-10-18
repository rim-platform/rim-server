# RIM plugin protocol v1

**Status: frozen, normative.** This is the contract between the RIM desktop host and a plugin
process. It is written for someone building a plugin from scratch, in any language, without an
SDK.

The machine-readable source of truth is the JSON Schema
[`protocol/plugin-protocol-v1.schema.json`](https://github.com/rim-platform/rim-ui/blob/master/protocol/plugin-protocol-v1.schema.json)
in rim-ui. Every frame described here is a `$defs` entry there, with an example, and the host's
conformance tests (`rim-ui/src-tauri/src/plugin_host/protocol/conformance.rs`) check those
examples against the frames the host really builds. **If this document and the schema ever
disagree, the schema wins**, and this document has a bug.

The key words MUST, MUST NOT, SHOULD and MAY are used as in RFC 2119.

Two later documents build on this one without changing it: `web-plugins-v1.md` (the
`WebSandboxed` kind: the same messages over a sandboxed iframe's `MessagePort`) and
`workspace-plugins-v1.md` (per-workspace registries and manifests signed by a publisher's key).

## Contents

1. [The protocol in one minute](#1-the-protocol-in-one-minute)
2. [Process and transport](#2-process-and-transport)
3. [Framing](#3-framing)
4. [Message envelope](#4-message-envelope)
5. [Request and response ids](#5-request-and-response-ids)
6. [Connection lifecycle](#6-connection-lifecycle)
7. [Handshake and version negotiation](#7-handshake-and-version-negotiation)
8. [Errors](#8-errors)
9. [Running commands](#9-running-commands)
10. [The dangerous-command confirmation gate](#10-the-dangerous-command-confirmation-gate)
11. [People: principals](#11-people-principals)
12. [Capabilities](#12-capabilities)
13. [Heartbeats](#13-heartbeats)
14. [Logging](#14-logging)
15. [Shutdown and app quit](#15-shutdown-and-app-quit)
16. [Compatibility and versioning](#16-compatibility-and-versioning)
17. [Method reference: what exists and what does not](#17-method-reference-what-exists-and-what-does-not)
18. [Worked example: one whole session](#18-worked-example-one-whole-session)
19. [A complete plugin in Python](#19-a-complete-plugin-in-python)
20. [Plugin author checklist](#20-plugin-author-checklist)

## 1. The protocol in one minute

- The host starts your plugin as a child process and talks to it over **stdin and stdout**.
  stderr is yours for free-form logging.
- Every message is a **frame**: a 4-byte **little-endian** unsigned length, then that many bytes of
  UTF-8 JSON. Each JSON document is one **JSON-RPC 2.0** message.
- The host's first request is **`rim.handshake`**. You answer which protocol version you speak.
- The host runs your commands with **`rim.command.execute`** and pings you with
  **`rim.heartbeat`** every 30 seconds.
- The host tells you who is signed in with a **`Principal`** (`{ principalId, displayName }`),
  both in the handshake and in **`rim.principal.changed`** when that changes.
- You may write to the host log with **`rim.log`**.
- When the host sends **`rim.shutdown`** or closes your stdin, you exit.

That is all of frozen v1. Everything else a plugin can do today is listed, with its real status,
in [section 17](#17-method-reference-what-exists-and-what-does-not).

## 2. Process and transport

A plugin is a directory with a manifest (`rim-plugin.json`) and an `entrypoint` file. The manifest
format is defined by the host (`rim-ui/src-tauri/src/plugin_host/manifest.rs`), not by this
protocol; this document only refers to the manifest fields the protocol depends on. Section 19
shows a minimal manifest.

A plugin id (`plugin_id` in the manifest) is reverse-domain: at least three dot-separated
segments of ASCII letters, digits and hyphens, such as `dev.example.hello-world`. Underscores are
not allowed. Ids starting with `dev.rim.` or `com.rim.` are reserved.

The host starts the plugin like this:

- It executes `<plugin dir>/<entrypoint>` **directly**, with no interpreter, no arguments, the
  plugin directory as the working directory and the user's environment. On Unix the entrypoint
  must be executable; a script needs a shebang line such as `#!/usr/bin/env python3`.
- stdin is a pipe from the host to the plugin. stdout is a pipe from the plugin to the host.
  **Nothing but frames may be written to stdout**: a stray `print` corrupts the stream.
- stderr is inherited by the host process and is not part of the protocol.
- One process is one connection. The host starts a plugin when the user enables it and, at app
  start, every plugin that was enabled when the app last ran. Start-up is not lazy in v1.

## 3. Framing

```
+----------------+-----------------------------------------+
| length: u32 LE | payload: `length` bytes of UTF-8 JSON   |
+----------------+-----------------------------------------+
```

- The length prefix is **4 bytes, unsigned, little-endian**. Earlier RIM docs said big-endian;
  that was wrong, and the host has always used little-endian.
- The length counts payload bytes only, not the 4 prefix bytes.
- The payload is exactly one JSON-RPC 2.0 message encoded as UTF-8 JSON. There is no newline
  delimiter, no batching (JSON arrays are not valid frames) and no MessagePack in v1.
- **A payload is at most 4,194,304 bytes (4 MiB)**, in both directions. A frame that announces a
  larger length ends the connection: the host stops reading and the plugin is stopped. Keep
  results small; page large data yourself.
- A frame whose payload is not valid JSON, or not a valid JSON-RPC message, is logged and dropped.
  The connection survives, because the length prefix kept the framing intact. Nobody answers it,
  so neither side ever has to handle a response with a `null` id.

For example, the notification
`{"jsonrpc":"2.0","method":"rim.log","params":{"level":"info","message":"started"}}` is 82 bytes
long, so its frame starts with the bytes `52 00 00 00`.

Reading a frame in Python:

```python
header = sys.stdin.buffer.read(4)          # b"" means the host closed stdin: exit
(length,) = struct.unpack("<I", header)    # "<I" = little-endian u32
message = json.loads(sys.stdin.buffer.read(length))
```

Reads from a pipe can return fewer bytes than asked for. Loop until you have all 4 header bytes
and all `length` payload bytes; treat end-of-file in the middle of a frame as a closed connection.

## 4. Message envelope

v1 uses plain JSON-RPC 2.0. Three shapes travel in **both** directions:

| Shape | Fields | Answered? |
|---|---|---|
| Request | `jsonrpc: "2.0"`, `id`, `method`, `params` (optional) | Yes, exactly once, with a response carrying the same `id` |
| Response | `jsonrpc: "2.0"`, `id`, and exactly one of `result` or `error` | No |
| Notification | `jsonrpc: "2.0"`, `method`, `params` (optional), **no `id`** | No |

How a receiver tells them apart, which is exactly what the host does:

- has `method` and a non-null `id`: a request;
- has `method` and no `id` (or `id: null`): a notification;
- has `id` and no `method`: a response;
- anything else: invalid, logged and dropped.

Further rules:

- **Field names in params and results are camelCase** (`protocolVersion`, `commandId`,
  `principalId`).
- Requests can be in flight in both directions at once. The host answers your requests while
  one of its own requests to you is pending, and you MUST be able to do the same, or at least
  never deadlock waiting for one response while another frame arrives first.
- Responses may arrive in any order. Match them by `id`, never by position.
- **Senders emit exactly the frozen shapes. Receivers ignore fields they do not know** rather
  than fail, so one side's bug does not break the connection. The host does.
- An unknown **request** is answered with `METHOD_NOT_FOUND` (`-32601`). An unknown
  **notification** is ignored. This is what lets v1 grow new methods without breaking anyone
  (see [section 16](#16-compatibility-and-versioning)).

## 5. Request and response ids

- An `id` is an integer or a string, **never `null`**. The answering side echoes it unchanged,
  with the same JSON type (`2` and `"2"` are different ids).
- The host numbers its requests `1, 2, 3, ...` per connection, so the handshake is normally id
  `1`. It numbers heartbeats `-1, -2, -3, ...`, so a negative integer id from the host is always
  a heartbeat.
- A plugin MAY number its own requests however it likes (integers, `"log-1"`, UUIDs). The two
  directions are independent: your ids only need to be unique among your own pending requests.
- The host answers every request you send, including denials, so a plugin never has to time out
  on the host. The host does time out on plugins (10 seconds for the handshake, 5 for a
  heartbeat, 30 for a command); a late answer to a request the host gave up on is dropped.

## 6. Connection lifecycle

```
host                                              plugin
 | spawn entrypoint                                  |
 |------------------ rim.handshake (id 1) ---------->|
 |<----------- rim.log (optional, any number) -------|
 |<------------- handshake result (id 1) ------------|   accepted: true
 |                                                   |
 |------- rim.command.execute (id 2, 3, ...) ------->|   whenever the user or agent runs a command
 |<-------------- result or error -------------------|
 |------- rim.heartbeat (id -1, -2, ...) ----------->|   every 30 s
 |<-------------------- {} --------------------------|
 |------- rim.principal.changed (notification) ----->|   on sign-in, sign-out, account switch
 |<--------------- rim.log (any time) ---------------|
 |                                                   |
 |------------- rim.shutdown (notification) -------->|
 | close stdin                                       |   plugin exits
```

1. **Spawn.** The host starts the process (section 2).
2. **Handshake.** The host's first frame is `rim.handshake`. Until you have answered it, send
   nothing but `rim.log`. The host waits up to **10 seconds** for the answer; a plugin that does
   not answer in time, refuses, or answers wrongly is stopped (section 7).
3. **Running.** The host sends commands, heartbeats and principal changes; the plugin answers
   and may log. A plugin with nothing to do just waits on stdin.
4. **Stopping.** The host sends `rim.shutdown`, then closes stdin. The plugin exits
   (section 15). A plugin MUST also exit when stdin reaches end-of-file without a
   `rim.shutdown`, because that is how a host that crashed says goodbye.

If the plugin exits or crashes on its own, the connection ends, commands for it fail with
"plugin process is not running", and the user can start it again by re-enabling it. The host does
not restart plugins automatically in v1.

## 7. Handshake and version negotiation

The handshake has **the same shape in every protocol version**, so any host can negotiate with
any plugin, including one from the future.

### Request: `rim.handshake` (host to plugin)

| Param | Type | Meaning |
|---|---|---|
| `protocolVersion` | integer ≥ 1 | The version the host prefers: the first entry of `supportedVersions`. |
| `supportedVersions` | array of integers, newest first, at least one | Every protocol version the host speaks. Today `[1]`. |
| `pluginId` | string | The plugin id from the manifest the host loaded for this process. |
| `hostVersion` | string | The RIM app version, for logs and diagnostics. Do not branch on it; branch on the protocol version. |
| `grantedCapabilities` | array of strings | Capabilities the user approved for this plugin (section 12). |
| `principal` | `Principal` or `null` | The signed-in user the plugin acts for, or `null` when nobody is signed in yet (section 11). |

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "method": "rim.handshake",
  "params": {
    "protocolVersion": 1,
    "supportedVersions": [1],
    "pluginId": "dev.example.hello-world",
    "hostVersion": "0.1.0",
    "grantedCapabilities": [],
    "principal": {
      "principalId": "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU",
      "displayName": "Jakub"
    }
  }
}
```

### Response (plugin to host)

| Result field | Type | Meaning |
|---|---|---|
| `accepted` | boolean | Whether the plugin accepts the connection. |
| `protocolVersion` | integer ≥ 1 | The version the plugin chose. MUST be one of `supportedVersions`. |
| `pluginId` | string | The plugin's own id. MUST equal `params.pluginId`. |
| `error` | string or `null`, optional | Why the plugin refused, when `accepted` is false. |

```json
{
  "jsonrpc": "2.0",
  "id": 1,
  "result": { "accepted": true, "protocolVersion": 1, "pluginId": "dev.example.hello-world" }
}
```

### Negotiation rules

- **The plugin picks.** It answers with one version from `supportedVersions`, normally the
  newest one it speaks. Every frame after the handshake follows the chosen version.
- **A plugin that speaks none of the offered versions** answers `accepted: false`, sets
  `protocolVersion` to the version it does speak, and says why in `error`. The host logs the
  reason and stops the plugin, so a plugin that refuses SHOULD exit right after sending its
  answer. For example:

  ```json
  {
    "jsonrpc": "2.0",
    "id": 1,
    "result": {
      "accepted": false,
      "protocolVersion": 2,
      "pluginId": "dev.example.hello-world",
      "error": "host speaks protocol [1], this plugin needs 2"
    }
  }
  ```

- **A plugin that refuses for any other reason** (a `pluginId` it does not recognise, a missing
  capability it cannot work without) answers the same way, with `accepted: false` and `error`.
- The host also stops the plugin when the answer picks a version it did not offer, when
  `pluginId` differs from the one it sent, when the result does not have the shape above, or when
  the answer is a JSON-RPC `error` instead of a `result`.
- After a failed handshake the host sends no `rim.shutdown` and does not close stdin first; it
  goes straight to steps 3 to 5 of section 15 (one second to exit, then `SIGTERM`, then a kill).
- The handshake is the host's first request and happens once per connection. There is no
  re-negotiation; a plugin that wants another version is restarted.

## 8. Errors

A failed request is answered with a JSON-RPC error response:

```json
{ "jsonrpc": "2.0", "id": 2, "error": { "code": -32002, "message": "kubectl failed: context not found" } }
```

| Field | Type | Meaning |
|---|---|---|
| `code` | integer | One of the codes below, or another integer the plugin documents. |
| `message` | string | One line, human-readable. The host may show it to the user, so write it for a person. |
| `data` | any JSON, optional | Machine-readable details. |

The codes v1 defines:

| Code | Name | Meaning | Typically sent by |
|---|---|---|---|
| `-32700` | `PARSE_ERROR` | The frame is not valid JSON. | Reserved: the host drops such frames and does not answer. |
| `-32600` | `INVALID_REQUEST` | The JSON is not a valid JSON-RPC 2.0 message. | Reserved, as above. |
| `-32601` | `METHOD_NOT_FOUND` | The receiver does not implement the method. | Both sides, for any unknown request. |
| `-32602` | `INVALID_PARAMS` | Params are missing or have the wrong shape, **including an unknown `commandId`**. | Both sides. |
| `-32603` | `INTERNAL_ERROR` | The receiver failed while handling a valid request. | Both sides. |
| `-32000` | `CAPABILITY_DENIED` | The user has not approved the capability the method needs. | Host. |
| `-32001` | `RATE_LIMITED` | The plugin called a capability more often than its per-minute limit. | Host. |
| `-32002` | `PLUGIN_ERROR` | The plugin ran the command and it failed; `message` says why. | Plugin, for a failed command. |

JSON-RPC 2.0 reserves `-32768` to `-32000`, and RIM uses only the codes above from that range. A
plugin's own error codes SHOULD lie outside it. A new protocol error code can be added within v1
only after it is documented in the schema (section 16).

How the host surfaces a plugin's error: when your answer to `rim.command.execute` is an error,
the command fails for the user with your `message`, and your `code` and `data` are passed along
to the UI unchanged.

## 9. Running commands

A plugin's commands are declared in its manifest. The fields this protocol depends on:

| Manifest field | Effect on the protocol |
|---|---|
| `id` | The `commandId` the host sends. The UI and keybinds address the command as `plugin.<pluginId>.<id>`; the plugin only ever sees `<id>`. |
| `dangerous` | Turns on the confirmation gate (section 10). |
| `required_capabilities` | The host refuses to run the command unless every listed capability is approved (section 12). |
| `ai_callable` | The AI agent may run the command, after the user consented to this plugin's AI commands. |
| `arguments` | Describes the `args` object for the UI and the agent. The host does not validate `args` against it; the plugin MUST validate its own input. |

### Request: `rim.command.execute` (host to plugin)

| Param | Type | Meaning |
|---|---|---|
| `commandId` | string | The `id` of a command in the plugin's manifest. |
| `args` | object | The command's arguments, keyed by argument name. `{}` when there are none; never `null`, never an array. |
| `invoker` | `Principal` or `null` | The signed-in user on whose behalf the command runs, or `null` when nobody is signed in. |
| `initiator` | `"user"` or `"agent"` | `user`: a person ran it from the palette, a keybind, a panel or a context action. `agent`: the AI agent ran it, after the user consented to this plugin's AI commands. |
| `confirmed` | boolean | `true` only for a `dangerous` command the user confirmed for this run. Always `false` for a command that is not dangerous. |

```json
{
  "jsonrpc": "2.0",
  "id": 2,
  "method": "rim.command.execute",
  "params": {
    "commandId": "greet",
    "args": { "name": "RIM" },
    "invoker": { "principalId": "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU", "displayName": "Jakub" },
    "initiator": "user",
    "confirmed": false
  }
}
```

### Response (plugin to host)

On success the `result` is an object:

| Result field | Type | Meaning |
|---|---|---|
| `payload` | any JSON, optional | The command's output. Missing means `null`. |
| `message` | string, optional | A one-line summary the host may show to the user. |

```json
{ "jsonrpc": "2.0", "id": 2, "result": { "payload": "Hello, RIM!" } }
```

On failure, answer with an error response instead (section 8): normally `PLUGIN_ERROR`
(`-32002`) when the command ran and failed, and `INVALID_PARAMS` (`-32602`) for an unknown
`commandId` or bad `args`.

Rules:

- The host gives up on a command after **30 seconds** and reports a timeout to the user. A
  command that takes longer should answer early with what it started and report progress some
  other way; v1 has no progress or cancellation messages.
- A `result` that is not an object fails the command with "invalid command result", even if it
  is valid JSON.
- The host only sends `commandId`s that the manifest declares, but a plugin MUST still answer an
  unknown one with `INVALID_PARAMS`, never by crashing.
- Several commands can be in flight at once. A plugin MAY handle them one at a time; the host
  matches answers by `id`.

## 10. The dangerous-command confirmation gate

A command whose manifest entry has `"dangerous": true` (restarting a pod, deleting a branch,
sending money) runs only after **the user confirmed that run**. The gate works like this:

1. **The host refuses an unconfirmed dangerous command before anything reaches the plugin.** The
   UI asks the user first; without that confirmation the command fails with
   `CONFIRMATION_REQUIRED` and the plugin never hears about it.
2. **`confirmed: true` is set only by the host, and only after the user confirmed this run.** It
   is not a field a panel, a keybind preset or a plugin can supply: the host computes it.
3. **The AI agent can never confirm.** A dangerous command the agent starts is always refused
   before it reaches the plugin, even if the command is `ai_callable`. The agent can propose it;
   only a person can run it.
4. **A plugin SHOULD still refuse a dangerous command that arrives with `confirmed: false`**,
   with `PLUGIN_ERROR` and a message such as "this command needs confirmation". The host never
   sends one, so this check costs nothing and protects you from a host bug.

A confirmed dangerous run looks like this:

```json
{
  "jsonrpc": "2.0",
  "id": 7,
  "method": "rim.command.execute",
  "params": {
    "commandId": "restart_pod",
    "args": { "namespace": "prod", "pod": "api-7f9c" },
    "invoker": { "principalId": "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU", "displayName": "Jakub" },
    "initiator": "user",
    "confirmed": true
  }
}
```

Changing what the gate means (for example letting a confirmation cover several runs) is a v2
change (section 16).

## 11. People: principals

### What a plugin sees of a person

A plugin sees a person **only** as a `Principal`:

```json
{ "principalId": "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU", "displayName": "Jakub" }
```

| Field | Rules |
|---|---|
| `principalId` | Opaque and stable. 1 to 128 characters from `A-Z a-z 0-9 _ . -`. **Never** the user's account UUID and never a `name@domain`. It does not change when the user's keys change. |
| `displayName` | A label to show. Not unique, may change at any time, never empty. Never use it as a key. |

A plugin **may** store and compare `principalId`. It **must not** parse it, derive anything from
its format, or assume its length or prefix. No other information about the user (account id,
email, server address) reaches a plugin through this protocol.

Principals appear in three places: `principal` in the handshake, `invoker` in every
`rim.command.execute`, and `principal` in `rim.principal.changed`.

### The signed-in-user notice: `rim.principal.changed`

The host sends this notification to every running plugin whenever the signed-in user changes:
sign-in, sign-out, switching account, or a new principal id scheme (below).

```json
{ "jsonrpc": "2.0", "method": "rim.principal.changed", "params": { "principal": null } }
```

`params.principal` is the signed-in user from now on, or `null` after sign-out. It replaces the
`principal` the plugin got in the handshake. A plugin that keeps per-user state SHOULD switch to
the new user's state, and after sign-out SHOULD stop doing anything on the old user's behalf. If
the user changes while the handshake is in flight, the host sends `rim.principal.changed` right
after the handshake completes, so a plugin never keeps a stale principal.

A plugin that does not care about the user simply ignores this notification, as it would any
notification it does not use.

### Today's ids are provisional, and will change once

Until RIM ships key-based identity, every `principalId` is **provisional**: it starts with
`rimp0_` and is a one-way hash of the user's account id. It is the same for one account on every
device and in every session, and it cannot be turned back into the account id.

When key identity ships:

- each person's id becomes a hash of the first entry of their key log (`rimp1_`, see
  `docs/specs/key-identity-v1.md`), which survives key rotation;
- the switch happens **once**. Every `rimp0_` id maps to exactly one new id, and new ids never
  start with `rimp0_`;
- the host tells running plugins with `rim.principal.changed`, carrying the new id of the
  signed-in user;
- the host also sends the full old-to-new mapping in `rim.principals.remapped` (below), so a
  plugin that stored `rimp0_` ids can rewrite them.

So: **keep stored principal ids somewhere you can rewrite** (a table column or a map key, not a
filename baked into a git repo), and treat "the same person under a new id" as possible exactly
once.

### The id switch notice: `rim.principals.remapped`

Added within v1 (section 16) with the desktop client of key identity. Old hosts never send it;
old plugins ignore it like any notification they do not use.

```json
{ "jsonrpc": "2.0", "method": "rim.principals.remapped",
  "params": { "mappings": [ { "from": "rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU",
                              "to": "rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo" } ] } }
```

| Param | Type | Meaning |
|---|---|---|
| `mappings` | array of `{ from, to }`, 1 to 1000 items | Every provisional id this host has seen switch, with its new id. Each `from` appears once. |

- The host sends it **right after every successful handshake** whenever it knows at least one
  mapping, and to every running plugin when it learns a new one (in that case after the
  `rim.principal.changed` that carries the new id of the signed-in user).
- The list is the **whole** mapping this host knows, not only the new entries, so the same
  mapping arrives many times. Applying it must be idempotent: replace every stored `from` with its
  `to`, and leave ids that are not a `from` alone.
- A host learns a mapping when it sees a person's key-based id for the first time; today that is
  the signed-in user, since no other person reaches a plugin yet. Mappings are kept per device,
  so another device may send a different (shorter or longer) list.
- `from` always starts with `rimp0_` and `to` never does; still treat both as opaque ids.

## 12. Capabilities

A capability is a permission the user grants a plugin. The plugin lists the capabilities it wants
in its manifest (`capabilities`); the user approves some or all of them; the approved list
arrives in `grantedCapabilities` in the handshake.

The host enforces capabilities in two places:

- **On commands.** A command whose manifest entry lists `required_capabilities` is refused
  before it reaches the plugin unless every one of them is approved.
- **On plugin-to-host calls.** Each host method that needs a capability is checked against the
  approved list. A call without it is answered with `CAPABILITY_DENIED` (`-32000`); a
  notification without it is dropped. Each capability also has a rate limit, **120 calls per
  minute** by default, counted per plugin; a call over the limit gets `RATE_LIMITED` (`-32001`).
  `rim.log` needs no capability and has no limit.

### Capability names

The protocol knows two kinds of capability name. Both are valid in `grantedCapabilities` in v1,
though the rim-ui host declares and grants built-in names only (see "What the host enforces"):

- **Plugin-scoped names**, `plugin.<pluginId>.<verb>`, such as
  `plugin.dev.example.k8stools.restart_pod`. `required_capabilities` on a command can point at
  them. The host guards no method of its own with them; it checks them only on commands.
- **Built-in names**, which guard things the host does for the plugin:

| Capability | Guards |
|---|---|
| `chat.read` | `rim.chat.list_messages`, `rim.chat.get_message`, `rim.chat.search_messages`, in plaintext conversations |
| `chat.write` | `rim.chat.post_message`, in plaintext conversations |
| `chat.read.encrypted` | The `chat.read` methods in end-to-end encrypted conversations too; declared only with `chat.read` |
| `chat.write.encrypted` | `rim.chat.post_message` in end-to-end encrypted conversations too; declared only with `chat.write` |
| `events.read` | `rim.events.subscribe` |
| `ui.panel` | `rim.ui.update_panel` (and the unimplemented `rim.ui.show_panel`) |
| `ui.notify` | Notifications a plugin's panel sends through the panel bridge; `rim.ui.show_notification` is unimplemented on the process side |
| `local.process` | `rim.process.spawn`, `rim.process.kill` (unimplemented) |
| `network` | `rim.network.http_request`, `rim.network.websocket_connect` (unimplemented) |
| `fs.read`, `fs.write` | `rim.fs.read_file`, `rim.fs.list_dir`, `rim.fs.write_file` (unimplemented) |

None of the methods the built-in names guard is part of frozen v1 (section 17).

### What the host enforces

These are properties of the rim-ui host, not of the protocol, and a host may change them without
a protocol change. They changed with the TypeScript SDK PR (rim-ui #16), with rim-ui #44 and
with rim-ui #190, so each version is described; check
`rim-ui/src-tauri/src/plugin_host/validation.rs` in the host version you target.

- **Which names a manifest may declare.** A manifest that declares any other name fails to
  install.
  - From rim-ui #190 onward: only the built-in names `ui.panel`, `ui.notify`, `chat.read`,
    `chat.write`, `chat.read.encrypted` and `chat.write.encrypted`, and for a `WebSandboxed`
    plugin also `network.fetch` (`web-plugins-v1.md`), in `capabilities` and in every command's
    `required_capabilities`. A plugin-scoped key `plugin.<pluginId>.<verb>` is refused, since
    the approval prompt lists only built-in names and could never approve one. An encrypted
    chat name declared without its plain one is rejected, and the other built-in names are
    rejected as from #44.
  - From rim-ui #44 to #190: the same built-in names, and the plugin's own keys
    `plugin.<pluginId>.<verb>`, where the verb uses only `[a-z0-9_.]` and neither starts nor
    ends with `.` nor contains `..`. An encrypted chat name declared without its plain one is
    rejected. The other built-in names are rejected: `events.read` guards a call that nothing
    serves and that is not in v1, and `local.process`, `network`, `fs.read` and `fs.write` guard
    nothing at all. A `WebSandboxed` plugin may also declare `network.fetch`. The approval
    prompt lists only built-in names, so a plugin-scoped key was never approved, and a command
    whose `required_capabilities` named one was always refused.
  - From rim-ui #16 to #44: the same rules with only `ui.panel` and `ui.notify` of the built-in
    names; `chat.read` and `chat.write` are rejected too.
  - Before rim-ui #16: only `plugin.<pluginId>.<verb>` keys, with the **whole** key in
    `[a-z0-9_.]`. No plugin could hold a built-in name, so `rim.ui.update_panel` was always
    answered `CAPABILITY_DENIED`, and a plugin id with an uppercase letter or a hyphen could
    declare no capability at all.
  - Before rim-ui #190: a key is 4 to 128 bytes long and has no segment named `system`,
    `internal`, `admin` or `audit`.
- **Which names a command may require.** From rim-ui #190 onward, every name in a command's
  `required_capabilities` must also be in the manifest's `capabilities`; otherwise install fails
  with `UNDECLARED_CAPABILITY`, naming the command and the key. Before rim-ui #190 the two lists
  were not compared at install, and a command requiring an undeclared name was refused when it
  ran.
- **Approving while the plugin runs.** `grantedCapabilities` is the approved list when the
  connection started, and v1 has no notification for changes to it. A capability the user
  approves while the plugin runs reaches the command gate at once, but the plugin's own calls to
  the host are checked against the start-time list until the process restarts. A plugin that
  needs a newly approved capability should ask the user to disable and re-enable it.
- **Revoking.** Revoking a capability marks the plugin disabled, so it does not start with the
  app next time.
  - From rim-ui #16 onward: revoking also stops the running plugin, so a revoked capability is
    never used again.
  - Before rim-ui #16: the running process was not stopped, and its own calls kept the
    start-time list until it stopped.

### Rules for v1

- `grantedCapabilities` is a list of strings. A plugin MUST ignore names it does not know.
- A new capability name may be added within v1 (section 16). A plugin learns about it only by
  finding it in `grantedCapabilities`.
- A capability is a host-side permission, not a sandbox. A local plugin process runs as the user
  and can do anything the user can; capabilities govern what the **host** will do for it.

## 13. Heartbeats

Every 30 seconds the host sends:

```json
{ "jsonrpc": "2.0", "id": -1, "method": "rim.heartbeat" }
```

There are no params. The plugin answers with **any object** as `result`; `{}` will do, and the
reference plugin sends `{"pong": true}`:

```json
{ "jsonrpc": "2.0", "id": -1, "result": { "pong": true } }
```

A plugin that has not answered within **5 seconds** is marked unresponsive. A late answer to
that heartbeat is dropped; the mark clears when the plugin answers a later one. A plugin that does long work SHOULD do it off the thread that reads stdin, so
heartbeats keep flowing.

## 14. Logging

`rim.log` (plugin to host) writes one line to the host log, tagged with the plugin id. It needs no
capability and is the one call allowed before the handshake is answered.

| Param | Type | Meaning |
|---|---|---|
| `level` | `"debug"`, `"info"`, `"warn"` or `"error"` | Severity. Any other value is `INVALID_PARAMS`. |
| `message` | string | The line to log. |

Send it as a **notification** (no `id`), which gets no answer:

```json
{ "jsonrpc": "2.0", "method": "rim.log", "params": { "level": "info", "message": "handshake complete" } }
```

or as a **request** (with an `id`), which the host answers with `{ "ok": true }`:

```json
{ "jsonrpc": "2.0", "id": "log-1", "result": { "ok": true } }
```

stderr also works for debugging, but only `rim.log` lines carry the plugin id and a level.

## 15. Shutdown and app quit

`rim.shutdown` is a notification from the host: **exit now.**

```json
{ "jsonrpc": "2.0", "method": "rim.shutdown" }
```

What the host does, in order:

1. sends `rim.shutdown` (it waits at most 250 ms to write it, so a plugin that stopped reading
   cannot hold up the stop);
2. closes the plugin's stdin;
3. waits **1 second** for the process to exit;
4. on Unix, sends `SIGTERM` and waits **2 more seconds**; on Windows, it kills the process at
   this point instead;
5. kills the process (`SIGKILL`) if it is still running.

When the **whole app quits**, every plugin gets the same sequence at the same moment, and any
plugin still running **4 seconds** after the quit began is killed outright.

What a plugin should do:

- On `rim.shutdown` or on end-of-file on stdin, flush what must be flushed, and exit. Aim for
  well under a second; anything after the first second is borrowed time.
- Do not send anything after `rim.shutdown`; nobody is reading.
- Do not rely on `SIGTERM` handlers for cleanup: Windows never sends one, and a plugin killed at
  the deadline gets no signal it can handle.

## 16. Compatibility and versioning

`protocolVersion` is a single integer. **There are no minor versions**: v1 is everything in the
schema plus any methods added under the rules below.

### What may still be added within v1

- A new method or notification in either direction. Old receivers answer an unknown request with
  `METHOD_NOT_FOUND` and ignore an unknown notification, so old plugins and old hosts keep
  working.
- Freezing a method that is listed as not frozen (section 17), as a new schema entry. Until
  then its shape may change without notice.
- A new capability name in `grantedCapabilities`.
- A new error code that is not one of the codes in section 8, documented in the schema before
  the host sends it.

### What forces v2

- Adding, removing, renaming or retyping a field in any frozen frame, **including an optional
  field**.
- A new value in a closed set, such as `initiator` or the `rim.log` levels.
- A different meaning for an existing method, field, value or error code, including the
  handshake, which keeps the same shape in every version.
- A change to framing, the frame size limit, the id rules or the confirmation gate.

### How the two sides stay compatible

- Senders emit exactly the frozen shapes; receivers ignore unknown fields rather than fail.
- The host lists every version it speaks in `supportedVersions`, newest first, and the plugin
  picks one (section 7). When a v2 exists, a host will offer both `[2, 1]` for as long as it
  supports v1; the RIM roadmap commits to supporting the current and the previous protocol
  version. A v1 plugin keeps working on that host by answering `1`.
- A plugin that only speaks a newer version than the host offers refuses the handshake, as in
  section 7, and the user sees why.

## 17. Method reference: what exists and what does not

This is the complete list. If a method is not in this section, the host does not implement it.

### Frozen in v1

| Method | Direction | Kind | Section |
|---|---|---|---|
| `rim.handshake` | host → plugin | request | 7 |
| `rim.heartbeat` | host → plugin | request | 13 |
| `rim.command.execute` | host → plugin | request | 9, 10 |
| `rim.principal.changed` | host → plugin | notification | 11 |
| `rim.principals.remapped` | host → plugin | notification | 11 |
| `rim.shutdown` | host → plugin | notification | 15 |
| `rim.log` | plugin → host | request or notification | 14 |

### Routed by the host but not part of v1

The host's broker recognises these names, but their shapes are **not frozen**. The chat methods
may still change before they are frozen; the others will change (to camelCase, with a
`Principal` for every person) when they are wired end to end, so do not build on them.

| Method | Direction | Capability | What happens today |
|---|---|---|---|
| `rim.chat.list_messages`, `rim.chat.get_message`, `rim.chat.search_messages` | plugin → host | `chat.read`, and `chat.read.encrypted` in an encrypted conversation | Served by the client message service. |
| `rim.chat.post_message` | plugin → host | `chat.write`, and `chat.write.encrypted` in an encrypted conversation | Served by the client message service. |
| `rim.events.subscribe` | plugin → host | `events.read` | Answered `CAPABILITY_DENIED`, since no host lets a manifest request `events.read` (section 12). Behind the gate it is not wired either. |
| `rim.event.<type>` | host → plugin | `events.read` | Never sent today, since nothing can subscribe. |
| `rim.ui.update_panel` | plugin → host | `ui.panel` | Wired to the panel state store. Hosts before rim-ui #16 cannot grant `ui.panel` (section 12) and answer `CAPABILITY_DENIED`. The panel page receives the stored state as `panel.state_update` (`docs/local_plugin_host.md`). |

The chat calls are deliberately **not in v1**: they go through the client's single message
service, so that a plugin sees exactly what the user sees, and their shapes are not frozen yet.
`e2ee-mls-v1.md`, "Plugins and the client message service", defines what they return and what
the encrypted capabilities add.

### Not implemented

Earlier RIM documents listed methods that the host has never implemented. They do not exist:

- `initialize`, `execute_command`, `deliver_event`, `shutdown` (the real names are
  `rim.handshake`, `rim.command.execute`, `rim.event.<type>` and `rim.shutdown`);
- `rim.chat.get_conversations`, `rim.chat.edit_message`, `rim.chat.delete_message`;
- `rim.ui.register_panel`, `rim.ui.show_panel`, `rim.ui.show_notification` (panels are declared
  in the manifest; notifications are a panel-side feature, see `docs/local_plugin_host.md`);
- `rim.settings.get`, `rim.settings.set`;
- `rim.process.*`, `rim.network.*`, `rim.fs.*`;
- MessagePack framing and any `supported_formats` / `chosen_format` negotiation.

Calling one gets `METHOD_NOT_FOUND`, except that a name in the host's capability table
(section 12) is checked first and gets `CAPABILITY_DENIED` when the plugin lacks that capability.

## 18. Worked example: one whole session

A plugin `dev.example.k8stools` with two commands, `list_pods` and `restart_pod` (dangerous).
Arrows show direction; each line is one frame's JSON payload.

```text
# 1. The host starts the plugin and handshakes. The plugin logs before answering, which is allowed.
host → plugin  {"jsonrpc":"2.0","id":1,"method":"rim.handshake","params":{"protocolVersion":1,"supportedVersions":[1],"pluginId":"dev.example.k8stools","hostVersion":"0.1.0","grantedCapabilities":["ui.panel"],"principal":{"principalId":"rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU","displayName":"Jakub"}}}
plugin → host  {"jsonrpc":"2.0","method":"rim.log","params":{"level":"info","message":"k8stools starting"}}
plugin → host  {"jsonrpc":"2.0","id":1,"result":{"accepted":true,"protocolVersion":1,"pluginId":"dev.example.k8stools"}}

# 2. The user runs "List pods" from the palette.
host → plugin  {"jsonrpc":"2.0","id":2,"method":"rim.command.execute","params":{"commandId":"list_pods","args":{"namespace":"prod"},"invoker":{"principalId":"rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU","displayName":"Jakub"},"initiator":"user","confirmed":false}}
plugin → host  {"jsonrpc":"2.0","id":2,"result":{"payload":[{"name":"api-7f9c","status":"Running"}],"message":"1 pod in prod"}}

# 3. The plugin asks for something the host does not implement; the host says so.
plugin → host  {"jsonrpc":"2.0","id":"p-1","method":"rim.kube.config"}
host → plugin  {"jsonrpc":"2.0","id":"p-1","error":{"code":-32601,"message":"method not found: rim.kube.config"}}

# 4. The AI agent tries to restart a pod. The command is dangerous, so the host refuses it
#    before it reaches the plugin: no frame is sent at all.

# 5. The user restarts the pod and confirms the dialog. The host sets confirmed: true.
host → plugin  {"jsonrpc":"2.0","id":3,"method":"rim.command.execute","params":{"commandId":"restart_pod","args":{"namespace":"prod","pod":"api-7f9c"},"invoker":{"principalId":"rimp0_PWcag22GowE59KogGTaUReeoF_q4E5EVi28q83uaGBU","displayName":"Jakub"},"initiator":"user","confirmed":true}}
plugin → host  {"jsonrpc":"2.0","id":3,"error":{"code":-32002,"message":"kubectl failed: context not found"}}

# 6. Thirty seconds after the connection started, a heartbeat.
host → plugin  {"jsonrpc":"2.0","id":-1,"method":"rim.heartbeat"}
plugin → host  {"jsonrpc":"2.0","id":-1,"result":{}}

# 7. The user signs out.
host → plugin  {"jsonrpc":"2.0","method":"rim.principal.changed","params":{"principal":null}}

# 8. The user disables the plugin. The host sends rim.shutdown and closes stdin; the plugin exits.
host → plugin  {"jsonrpc":"2.0","method":"rim.shutdown"}
```

Step 5 shows the command failing with `PLUGIN_ERROR`: the user sees "kubectl failed: context not
found". Note in step 3 that the plugin's string id `"p-1"` comes back unchanged.

## 19. A complete plugin in Python

A conforming v1 plugin, standard library only, with one command, `greet`. Put two files in one
directory: the manifest `rim-plugin.json`,

```json
{
  "manifest_version": 1,
  "plugin_id": "dev.example.greeter",
  "display_name": "Greeter",
  "version": "0.1.0",
  "description": "Says hello.",
  "runtime_kind": "DesktopLocalProcess",
  "entrypoint": "main.py",
  "commands": [
    {
      "id": "greet",
      "title": "Greeter: Greet",
      "description": "Greets someone by name.",
      "arguments": [{ "name": "name", "type": "string", "required": false }]
    }
  ]
}
```

and `main.py`, made executable (`chmod +x main.py`):

```python
#!/usr/bin/env python3
import json, struct, sys

PLUGIN_ID = "dev.example.greeter"   # must match plugin_id in rim-plugin.json
VERSION = 1

def read_exact(n):
    buf = b""
    while len(buf) < n:
        chunk = sys.stdin.buffer.read(n - len(buf))
        if not chunk:
            raise EOFError
        buf += chunk
    return buf

def read_frame():
    (length,) = struct.unpack("<I", read_exact(4))
    return json.loads(read_exact(length))

def send(message):
    body = json.dumps(message).encode("utf-8")
    sys.stdout.buffer.write(struct.pack("<I", len(body)) + body)
    sys.stdout.buffer.flush()

def answer(request, result=None, error=None):
    reply = {"jsonrpc": "2.0", "id": request["id"]}
    reply["error" if error else "result"] = error or result
    send(reply)

def log(level, message):
    send({"jsonrpc": "2.0", "method": "rim.log", "params": {"level": level, "message": message}})

def handle(msg):
    method, params = msg.get("method"), msg.get("params") or {}
    if method == "rim.handshake":
        ok = VERSION in params.get("supportedVersions", []) and params.get("pluginId") == PLUGIN_ID
        answer(msg, {"accepted": ok, "protocolVersion": VERSION, "pluginId": PLUGIN_ID,
                     **({} if ok else {"error": "need protocol 1 and plugin id " + PLUGIN_ID})})
        if not ok:
            sys.exit(0)  # refused: the host stops us anyway
    elif method == "rim.heartbeat":
        answer(msg, {})
    elif method == "rim.command.execute":
        if params.get("commandId") == "greet":
            name = (params.get("args") or {}).get("name") or "world"
            answer(msg, {"payload": f"Hello, {name}!"})
        else:
            answer(msg, error={"code": -32602, "message": f"unknown command: {params.get('commandId')}"})
    elif method == "rim.shutdown":
        sys.exit(0)
    elif msg.get("id") is not None and method is not None:
        answer(msg, error={"code": -32601, "message": f"method not found: {method}"})
    # Anything else (unknown notifications, responses to our own requests) is ignored.

def main():
    log("info", "greeter started")
    while True:
        try:
            msg = read_frame()
        except EOFError:
            return  # the host closed stdin: exit
        except ValueError:
            continue  # not JSON: drop it, framing is still intact
        if isinstance(msg, dict):  # anything else is not JSON-RPC: drop it too
            handle(msg)

if __name__ == "__main__":
    main()
```

The in-tree reference plugin, `rim-ui/plugins/hello-world/`, does the same with more comments and
is driven through every v1 message by the host's conformance tests.

## 20. Plugin author checklist

- [ ] The entrypoint is executable and writes nothing but frames to stdout.
- [ ] Frames use a 4-byte **little-endian** length and stay under 4 MiB.
- [ ] Partial reads are looped; end-of-file on stdin means exit.
- [ ] Nothing but `rim.log` is sent before the handshake answer.
- [ ] The handshake picks a version from `supportedVersions`, echoes `pluginId`, and refuses with
      `accepted: false` and an `error` otherwise.
- [ ] Every request gets exactly one answer with the same `id`, including heartbeats.
- [ ] Unknown requests get `METHOD_NOT_FOUND`; unknown notifications and unknown fields are
      ignored.
- [ ] An unknown `commandId` or bad `args` gets `INVALID_PARAMS`; a failed command gets
      `PLUGIN_ERROR` with a message a person can read.
- [ ] A dangerous command with `confirmed: false` is refused.
- [ ] `principalId` is stored only where it can be rewritten, and never parsed.
- [ ] `rim.shutdown` makes the plugin exit within a second.
- [ ] Commands answer within 30 seconds; heartbeats keep flowing during long work.

## Where this lives in the host

| Concern | File in `rim-ui/src-tauri/src/plugin_host/` |
|---|---|
| Frozen frame types, principal derivation | `protocol.rs` |
| Conformance tests against the schema | `protocol/conformance.rs` |
| Framing, error codes, size limit | `wire.rs` |
| Handshake validation | `handshake.rs` |
| Broker loop, plugin-to-host dispatch, heartbeats, rate limits | `broker.rs` |
| Capability table | `capability.rs` |
| Command gates and dispatch | `command_exec.rs` |
| Start, stop, app quit, principal notices | `runtime.rs`, `process.rs` |
| Manifest and install-time validation | `manifest.rs`, `validation.rs` |
