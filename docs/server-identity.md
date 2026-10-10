# Server identity and server-to-server traffic

Every rim-server has its own **server key**, an Ed25519 key whose signed history (the **key log**)
fixes the server's id, `rims1_…`. Other servers use the id and key to tell this server apart from
any other, to check the requests it sends them and the answers it gives. The format and protocol
are in `docs/specs/server-to-server-v1.md`; this page is the operator's view.

## The key file

| What | Where |
|---|---|
| Server key seed | `RIM_SERVER_KEY_FILE`, default `<RIM_DATA_DIR>/server-key.json` (`RIM_DATA_DIR` defaults to `./data`) |
| Key log, descriptor, pinned peers | The database (`server_key_log_entry`, `server_descriptor`, `peer_server`) |
| Recovery key, after first start | `<RIM_DATA_DIR>/server-recovery-key.txt` until you move it |

The key file is `{"v":1,"seed":"<base64url>"}`, mode `0600`. The server refuses to start when:

- the file is readable by group or others (`chmod 600` it);
- the file exists but the key log is empty (a new database next to an old key), or the log exists
  but the file is missing (a lost key: restore the file or run `server-key recover`);
- the file holds another key than the one the log's head names.

Back the key file up with the database: the database without the file cannot sign, the file
without the database is a key nobody can prove belongs to this server id. Never copy the key file
to another deployment; two servers with one key are one server to everyone else.

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
  out of band; its next request pins it afresh.

## The franking key file

Reports of encrypted messages are checked with the server's **franking keys**
(`docs/specs/e2ee-mls-v1.md`, "Franking"). They live in their own file, next to the server key
and with the same rules:

| What | Where |
|---|---|
| Franking keys | `RIM_FRANKING_KEY_FILE`, default `<RIM_DATA_DIR>/franking-key.json` |

The file is `{"v":1,"keys":[{"id":1,"key":"<base64url>","createdAt":"…"}]}`, mode `0600`; the
server refuses to start when the group or others can read it. The first start creates it with
one key. A server upgraded from a version that kept the key in the database moves it into the
file at that start and rotates at once: the moved key, which the database and its backups also
hold, only verifies from then on, and a new key signs. The log line says so. The server refuses
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
MLS cipher suites, limits and registration mode. The server signs a new descriptor with a higher
`version` whenever its configuration changes it and after every rotation.

## Roles and server-to-server configuration

With nothing configured a server is what it always was: its own users' server and the host of its
workspaces (`RIM_ROLE=both`), with server-to-server traffic off.

| Variable | Default | Meaning |
|---|---|---|
| `RIM_PUBLIC_BASE_URL` | unset | The URL clients and peers reach this server at. Server-to-server traffic is on only when it is `https`. Its origin is sent as `Rim-Origin` and is the base every signed request is checked against, so it must be the URL peers actually use, also behind a TLS-terminating proxy |
| `RIM_ROLE` | `both` | `relay` (its users' server and their direct messages), `host` (workspaces only) or `both` |
| `RIM_SERVER_KEY_FILE` | `<RIM_DATA_DIR>/server-key.json` | The server key seed |
| `RIM_HOST_ALLOWED_RELAYS` | empty, any | Comma-separated server ids allowed to use this server's workspaces as a host |
| `RIM_S2S_DENIED_PEERS` | empty | Comma-separated server ids refused everywhere |
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
until its operator resets the pin. Each peer appears in the log as `s2s.peer_pinned`,
`s2s.peer_rotated` and `s2s.peer_forked` events (target `audit`), with server ids only.
