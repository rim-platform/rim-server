# Server identity and server-to-server traffic

Every rim-server has its own **server key**, an Ed25519 key whose signed history (the **key log**)
fixes the server's id, `rims1_…`. Other servers use the id and key to tell this server apart from
any other, to check the requests it sends them and the answers it gives. The format and protocol
are in `docs/specs/server-to-server-v1.md`; this page is the operator's view of the server's own
identity. What a relay and a host do with it is in `docs/relay.md`; running the public relay is
in `docs/operations/relay-public.md`.

## The key file

| What | Where |
|---|---|
| Server key seed | `RIM_SERVER_KEY_FILE`, default `<RIM_DATA_DIR>/server-key.json` (`RIM_DATA_DIR` defaults to `./data`) |
| Key log, descriptor, pinned peers | The database (`server_key_log_entry`, `server_descriptor`, `peer_server`, and `peer_server_forgotten` for the heads of pins forgotten after 90 idle days) |
| Recovery key, after first start | `<RIM_DATA_DIR>/server-recovery-key.txt` until you move it |

The key file is `{"v":1,"seed":"<base64url>"}`, mode `0600`. The server refuses to start when:

- the file is readable by group or others (`chmod 600` it);
- the file exists but the key log is empty (a new database next to an old key), or the log exists
  but the file is missing (a lost key: restore the file or run `server-key recover`);
- the file holds another key than the one the log's head names.

Back the key file up with the database: the database without the file cannot sign, the file
without the database is a key nobody can prove belongs to this server id. Never copy the key file
to another deployment; two servers with one key are one server to everyone else.

While the server runs, `GET /readyz` answers 503 with the `serverKey` check failed when the key
file cannot be opened for reading, and `frankingKey` likewise for the franking key file.

## First start

With neither a key file nor a key log, the server creates a server key and a recovery key, signs
the log's first entry, writes the key file and writes the recovery key to
`<RIM_DATA_DIR>/server-recovery-key.txt` (one line, `rim-server-recovery:1:<base64url>`, mode
`0600`). It logs a warning at every start while that file is there:

> move the server recovery key offline: anyone who can read it can replace this server's key

Copy the file to offline storage (a password manager, a printed copy in a safe) and delete it
from the server. The recovery key is only needed if the server key is lost or stolen.

## Commands

```sh
rim_server server-key show      # server id, key seq, keys, key hash
rim_server server-key rotate    # a routine rotation, signed by the current key
rim_server server-key recover --recovery-key-file /path/to/recovery.txt [--new-recovery-key]
rim_server peer reset <server-id>
rim_server peer block <server-id>
rim_server peer unblock <server-id>
```

They read the same configuration as the server (`DATABASE_URL`, `RIM_DATA_DIR`,
`RIM_SERVER_KEY_FILE`). A running server notices a rotation or recovery within a minute and
switches to the new key; no restart is needed. With several nodes, every node needs the new key
file: copy it before the minute passes, or nodes still holding the old file refuse to pick the
rotation up and keep signing with the old key until they get it.

- **Rotate** when you want a new key while the current one is safe (a schedule, a staff change).
  Peers keep accepting the previous key for 24 hours after they see the rotation, so in-flight
  requests keep working.
- **Recover** when the server key is lost or may be stolen. The rotation is signed by the
  recovery key; peers stop accepting the previous key at once. Works without the key file. With
  `--new-recovery-key` the recovery key is replaced too and the new one is written to
  `<RIM_DATA_DIR>/server-recovery-key.txt` (the command refuses to overwrite a file there): move it
  offline again.
- **Peer reset** forgets a peer this server refuses because its key log forked (its history was
  rewritten: a restored backup, a stolen key, or an attack). Check with the peer's operator first,
  out of band; its next request pins it afresh. It also deletes the key-log head kept for a peer
  whose idle pin was forgotten, and frees a place at an origin that holds the most pins allowed
  (4).
- **Peer block** refuses a pinned peer both ways from its next request, without a restart: its
  requests get `403 peer_not_allowed` and nothing is sent to it. **Peer unblock** lifts it. A
  peer never pinned cannot be blocked this way; list it in `RIM_S2S_DENIED_PEERS`.

## The franking key file

Reports of encrypted messages are checked with the server's **franking keys**
(`docs/specs/e2ee-mls-v1.md`, "Franking"). They live in their own file, next to the server key
and with the same rules:

| What | Where |
|---|---|
| Franking keys | `RIM_FRANKING_KEY_FILE`, default `<RIM_DATA_DIR>/franking-key.json` |

The file is `{"v":1,"keys":[{"id":1,"key":"<base64url>","createdAt":"…"}]}`, mode `0600`; the
server refuses to start when the group or others can read it. The first start creates it with
one key. The server refuses
to start when the file is missing but messages carry receipts under a key it no longer has:
restore the file from a backup.

```sh
rim_server franking-key show      # the keys and which one signs
rim_server franking-key rotate    # a new signing key; every older key keeps verifying
```

Rotate on a schedule or when the file may have leaked. Rotation never removes a key: a receipt is
never signed again, so the key that signed it must stay for as long as the message can be
reported. A running server picks the new file up within a minute. With several nodes, every node needs
the same file: copy it to each node after a rotation.

Back the file up with the database. Whoever reads it can make receipts, but a report must also
match the content commitment stored with the message, so the file alone does not let anyone have
a forged report verified.

## Key login audiences

A device's login proof names the server it is meant for (`host[:port]`), so a proof sent to one
server cannot be replayed at another. The server accepts the names in `RIM_KEY_LOGIN_AUDIENCES`
(comma-separated), else the host of `RIM_PUBLIC_BASE_URL`, plus its own server id. With neither
variable set it falls back to `localhost:PORT` and `127.0.0.1:PORT`, and only when
`RIM_SERVER_HOST` is `localhost` or a loopback address. A server bound to any other address,
`0.0.0.0` (the default and the Docker image's) included, refuses to start until one of the two
variables is set:

```text
RIM_SERVER_HOST=0.0.0.0 accepts connections from other machines: set RIM_KEY_LOGIN_AUDIENCES or
RIM_PUBLIC_BASE_URL to the host[:port] clients reach this server at (the localhost default is
only for loopback binds)
```

For local development bind `RIM_SERVER_HOST=127.0.0.1`, or set
`RIM_KEY_LOGIN_AUDIENCES=localhost:8080,127.0.0.1:8080`.

## The discovery document

`GET /.well-known/rim-server` answers, to anyone and cacheable for five minutes, the key log and
the **descriptor**: a statement signed by the current key with the server's roles, its endpoints
(`/api`, `/api/ws`, and `/s2s/v1` when server-to-server traffic is on), the protocol versions,
MLS cipher suites, limits, registration mode and **handle domain**. The server signs a new
descriptor with a higher `version` whenever its configuration changes it and after every rotation.

### Handles and delegation

People are shown to other servers by their **handle**, `@<username>:<domain>`, such as
`@kuba:rim-platform.org`. The domain is `RIM_HANDLE_DOMAIN`, by default the host of
`RIM_PUBLIC_BASE_URL`; the descriptor declares it as `handleDomain`. A handle is a readable name
for a person's key, never the key itself: a username change changes the handle and nothing else.

With the default nothing more is needed. To give handles a domain other than the server's own
host, so that they survive moving the server to another machine or subdomain (the server at
`https://relay.rim-platform.org` with handles at `rim-platform.org`), set
`RIM_HANDLE_DOMAIN=rim-platform.org` and serve this file at
`https://rim-platform.org/.well-known/rim-server`, from whatever answers there (a static site
will do):

```json
{"delegate": "https://relay.rim-platform.org"}
```

The value is the origin of `RIM_PUBLIC_BASE_URL`: `https`, no path, no trailing slash. Other
servers check the claim both ways before they show a handle: the domain must hand them to this
server, and this server's descriptor must name the domain. They fetch the file with the same rules
as a discovery document (5 seconds, 64 KiB, no redirects, public addresses only) and follow a
delegation once; the file at the delegate origin must be this server's own discovery document. A
server whose domain does not answer, or answers with anything else, keeps its people without
handles on other servers (they log "a peer's handle domain does not point back at it"), and its
own users still see their own handles. Changing `RIM_HANDLE_DOMAIN` later changes everyone's
handle; people's keys, contacts and memberships stay as they are. The protocol is in
`docs/specs/server-to-server-v1.md`, "Handles".

## Roles and server-to-server configuration

With nothing configured a server is what it always was: its own users' server and the host of its
workspaces (`RIM_ROLE=both`), with server-to-server traffic off.

| Variable | Default | Meaning |
|---|---|---|
| `RIM_PUBLIC_BASE_URL` | unset | The URL clients and peers reach this server at. Server-to-server traffic is on only when it is `https`. Its origin is sent as `Rim-Origin` and is the base every signed request is checked against, so it must be the URL peers actually use, also behind a TLS-terminating proxy |
| `RIM_HANDLE_DOMAIN` | the host of `RIM_PUBLIC_BASE_URL` | The domain of people's handles (`@<username>:<domain>`), a lowercase DNS name; the server refuses to start on anything else. Another domain delegates to this server ("Handles and delegation") |
| `RIM_ROLE` | `both` | `relay` (its users' server and their direct messages), `host` (workspaces only) or `both` |
| `RIM_SERVER_KEY_FILE` | `<RIM_DATA_DIR>/server-key.json` | The server key seed |
| `RIM_HOST_ALLOWED_RELAYS` | empty, any | Comma-separated server ids allowed to use this server's workspaces as a host |
| `RIM_S2S_DENIED_PEERS` | empty | Comma-separated server ids refused everywhere |
| `RIM_DM_INBOUND` | `requests` | Who outside a shared workspace may start a DM with people here: `open`, `requests`, `partners` (the servers `RIM_DM_PARTNERS` lists, and this one) or `none`. With `partners` or `none` other servers get no identity or key package of anyone here |
| `RIM_DM_OUTBOUND` | `allowed` | Whom on other servers people here may start a DM with: `allowed`, `partners` or `none` |
| `RIM_DM_PARTNERS` | empty | Comma-separated server ids the `partners` settings name |
| `RIM_S2S_PEER_RATE_PER_MIN` | 600 | Requests accepted per peer per minute, per node |
| `RIM_S2S_MAX_BODY_BYTES` | 8388608 | Largest request body from a peer |
| `RIM_S2S_ALLOW_HTTP` | false | Development and tests only: allow `http` |
| `RIM_S2S_ALLOW_PRIVATE_ADDRESSES` | false | Development and tests only: allow peers on loopback and private networks |

Server ids in the lists must be exact (`rims1_` and 43 characters); the server refuses to start on
anything else, so a typo cannot silently allow or deny the wrong server.

Behind a reverse proxy, pass `/.well-known/rim-server` and `/s2s/v1/` through unchanged; the
signature covers the method, the full URL and the body. A peer's clock must be within five minutes
of this server's.

## What a peer sees

A peer learns this server's id from a request, an invite or a profile card, fetches the discovery
document from the origin, checks that the log hashes to the id, and pins the log. From then on it
accepts only requests and answers signed by the pinned key (or the previous key for 24 hours after
a routine rotation). If the log it fetches later does not extend the pin, it refuses this server
until its operator resets the pin. It also checks this server's handle domain, as "Handles and
delegation" says, at the pin and at every refetch. Each peer appears in the log as
`s2s.peer_pinned`, `s2s.peer_rotated`, `s2s.peer_forked`, `s2s.peer_blocked`,
`s2s.peer_unblocked` and `s2s.peer_handle_domain` events (target `audit`), with server ids and
the checked domain only.
