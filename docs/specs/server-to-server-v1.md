# Server-to-server v1 (Stage 5, S5-0)

_Spec written 2026-10-09 for Stage 5 (`docs/plans/stage-5-2026-10-09.md`, `ROADMAP.md` Track 7.4)
under `docs/decisions/decentralization-2026-10-08.md` and
`docs/decisions/relay-proxied-workspaces-2026-10-09.md`. Read `key-identity-v1.md`,
`device-linking-v1.md` and `e2ee-mls-v1.md` first. This document is the contract between the server
PRs (S5-1, S5-2, S5-4, S5-6) and the client PRs (S5-3, S5-5): server identity, the signed HTTP API
between servers, relays, hosts, and what the client verifies._

## Summary

1. Every rim-server has an Ed25519 **server key** in a signed key log whose inception fixes a
   `rims1_` **server id**, published with a signed descriptor at `GET /.well-known/rim-server`.
2. Servers talk over HTTPS with **HTTP Message Signatures** (RFC 9421) over a fixed component list
   with `Content-Digest` (RFC 9530), a 5-minute window and a nonce store. Responses are signed too.
   A minimal, vector-tested profile lives in a new `rim_s2s` crate; no published crate fits.
3. `RIM_ROLE=relay|host|both`, default `both`: with no new configuration a server behaves as today,
   and server-to-server traffic stays off until `RIM_PUBLIC_BASE_URL` is an `https` URL.
4. Each relay has one **direct space**, a `server` row of kind `direct` holding the DMs and groups
   its users start with anyone. The relay **hosts** them: sequencer, MLS delivery service, change
   feed. A DM's home is its creator's relay.
5. Hosts know people from elsewhere as `user` rows of kind `remote`, keyed by principal id, with
   key log, devices and card copied from the person's relays and kept current.
6. A client talks only to its relay. Requests for a space hosted elsewhere go to
   `/api/remote/<host server id>/…` with a **device-signed request** statement; the relay forwards
   them unchanged; the host answers with a **host-signed response** the client checks.
7. Hosts sign every event they send to relays (`host-event`, per-stream sequence numbers). Relays
   queue events per user with per-device cursors (30 days at most) and push them over the client's
   existing WebSocket; the client verifies them against the host key pinned from the invite.
8. Workspace traffic reaches a relay only under a **delegation** a device signed (relay, host,
   workspace, at most 24 hours); direct-space traffic goes to the relays the person's card lists.
9. A device revocation fans out from the relay to every host with a signed acknowledgement; the app
   sends it directly to a host that has not acknowledged within 2 minutes.
10. When the relay is down the desktop app logs in to a host directly with its device key. A public
    relay gates registration with proof-of-work or an invite.

## Parties and terms

| Term | Meaning |
|---|---|
| Server | One rim-server deployment: one server key, one database, one or more nodes |
| Relay | The server a person's app talks to; their `user` row there is `kind = local` |
| Host | The authority for a space: a workspace, or a relay's direct space |
| Space | A `server` row: `kind = workspace` or `kind = direct` |
| Peer | Another server this one has talked to |
| Remote user | A `user` row of `kind = remote` on a host: a person whose relay is elsewhere |

A server running `both` hosts its own workspaces and direct space and is the relay of its local
users. Today's single server is exactly that and stays that with nothing configured.

## Server identity

### Server key log

The rules of a person's key log apply (`key-identity-v1.md`, "Signed statements": exact signed
text, unknown and duplicate fields refused, at most 256 entries), in **version 2 only**. Context
bytes `rim.server-key.v1\0`, payload at most 16 KiB.

```json
{"v":2,"type":"server-inception","seq":0,"serverKey":"ed25519:…","recoveryKey":"ed25519:…","createdAt":"2026-10-09T00:00:00Z"}
{"v":2,"type":"server-rotate","serverId":"rims1_…","seq":1,"prev":"…","serverKey":"ed25519:…","recoveryKey":"ed25519:…","signer":"server","createdAt":"…"}
```

```
serverId = "rims1_" + base64url(SHA-256("rim.server.v1\0" || inception payload bytes))
keyHash  = base64url(SHA-256(tagged key text))          (as device_key.key_hash)
```

- The inception is signed by its `serverKey`. `"signer": "server"`: signed by the current server
  key, recovery key unchanged. `"signer": "recovery"`: signed by the current recovery key, which may
  change. `seq`, `prev` and "the key must change" are as for a person's log.
- **Private keys** never touch the database: the server key's 32-byte seed is in
  `RIM_SERVER_KEY_FILE` (default `<RIM_DATA_DIR>/server-key.json`, `{"v":1,"seed":"<base64url>"}`);
  the server refuses to start if the file is readable by group or others, or if file and log head
  disagree. The log is in `server_key_log_entry`.
- **First boot** (S5-1) with neither file nor log: the server creates both keys, the inception and
  the file, and writes the recovery seed to `<RIM_DATA_DIR>/server-recovery-key.txt` (0600) as
  `rim-server-recovery:1:<base64url>`, logging a warning at every start while it exists ("move the
  server recovery key offline").
- **CLI** (S5-1, beside `bootstrap_cmd.rs`): `rim_server server-key show`, `server-key rotate` (a
  `signer: server` rotation; the old seed is overwritten after the entry commits),
  `server-key recover --recovery-key-file F [--new-recovery-key]`. The new seed is written to
  `<key file>.next` (0600) before its entry commits and moved over the key file after; a start
  that finds the log head naming the `.next` seed moves it into place. A running server checks the
  log every minute and switches to the new key, re-signing its descriptor. `recover` works without
  the key file; with `--new-recovery-key` it writes the new recovery key where first boot does,
  refusing to overwrite a file there.

### Discovery document

`GET /.well-known/rim-server`, unauthenticated, `Cache-Control: max-age=300`,
`Access-Control-Allow-Origin: *`:

```json
{"rim":"server-discovery","keyLog":[{"payload":"…","signature":"…"}],"descriptor":{"payload":"…","signature":"…"}}
```

The descriptor is signed by the head server key over `rim.server-descriptor.v1\0 || payload`
(at most 16 KiB):

```json
{"v":2,"type":"server-descriptor","serverId":"rims1_…","keySeq":0,"version":1,"roles":["host","relay"],
 "endpoints":{"graphql":"https://host.example/api","websocket":"wss://host.example/api/ws","s2s":"https://host.example/s2s/v1"},
 "s2sVersions":[1],"mlsCipherSuites":[79],"limits":{"maxBodyBytes":8388608,"clockSkewSeconds":300,"maxDeliverItems":256},
 "registration":{"mode":"invite","powBits":null},"issuedAt":"…"}
```

| Field | Rule |
|---|---|
| `keySeq` | The log head; a descriptor signed by another key is refused |
| `version` | 1 to 2³¹−1; readers keep only a higher one. Re-signed with `version + 1` on any change and after every rotation |
| `roles` | Non-empty subset of `host`, `relay`, in that order |
| `endpoints` | One origin in v1; `s2s` null while server-to-server traffic is off |
| `s2sVersions` | `[1]`; a later version keeps serving the previous one (`ROADMAP.md`, "Releases and updates") |
| `mlsCipherSuites` | What `uploadMlsKeyPackages` accepts |
| `registration` | `mode` `invite`, `open` or `pow`; `powBits` only in `pow` mode |

The decision record wrote `/.well-known/rim/server` as an illustration; one well-known name is
simpler. Rejected: an unsigned descriptor (anyone proxying the fetch, a relay included, could change
endpoints), and signing the whole document (the log is self-certifying; one verifier per statement).

### Pinning a peer

A server learns a peer's id from a request's `keyid`, a delegation, a card's relay URL or an
invite. To pin it: fetch `<origin>/.well-known/rim-server` over HTTPS (5 s, 64 KiB, no redirects;
origins resolving to loopback, link-local, private or unique-local addresses are refused unless
`RIM_S2S_ALLOW_PRIVATE_ADDRESSES=true`), replay the log, check the inception against the expected
id, verify the descriptor (its `endpoints.s2s` must start with the origin), and store everything in
`peer_server`. Refresh every 6 hours on use, on a `keyid` above the pinned head, and on a `401` from
the peer; at most one fetch per peer per minute and 60 in total.

| Fetched log | Action |
|---|---|
| Equal to the pin | Keep; take the descriptor if its version is higher |
| Extends the pin with valid rotations | Accept. After a `signer: server` rotation the previous key stays valid 24 hours from when this server accepted the extension; after a `signer: recovery` rotation it is refused at once |
| Does not extend it (a fork) | Status `forked`: requests either way fail `403 peer_forked`, ERROR log, audit `s2s.peer_forked`, until `rim_server peer reset <id>` after an out-of-band check |
| A prefix of the pin | Keep the pin (the peer serves an older view, say a lagging node) |

A first pin keeps the key before the head but gives it no grace: the grace counts from an
extension this server saw. A refetch whose inception hashes to another id (the origin now serves
another server) fails and keeps the pin; only a log from the same inception can fork it.

### Host keys in clients

- **From an invite** (`rim-invite:1:<hostServerId>:<hostKeyHash>:<inviteToken>:<hostOrigin>`): the
  app fetches the discovery document through its relay (`remoteServer(origin)`), replays the log,
  checks the inception hashes to `hostServerId` and that `hostKeyHash` names a key of the log
  followed only by `signer: server` rotations, then pins id and log.
- **Trust on first use** for a contact's relay (the home of a DM someone else started). DM content
  is MLS-protected either way; the pin protects metadata events.
- A signature at a `keySeq` above the pin makes the app refetch (at most once a minute) and accept
  only an extension. The key before the head keeps verifying 24 hours after the app accepted a
  `signer: server` rotation, never after a `signer: recovery` one, and a first pin gives it no
  grace, as for servers ("Pinning a peer"). A fork stops the app using that server: "This
  server's identity changed in a way RIM cannot verify".

The invite carries the host's existing invite token, not one the host signed: the server id is a
hash of the host's inception, so id plus key hash already expose a substituted host, and a
signature over the token would add nothing.

## The S2S transport

### Signature profile

Requests go over HTTPS (`http` only with `RIM_S2S_ALLOW_HTTP=true`) to paths under the peer's
`endpoints.s2s`, signed per RFC 9421 under the label `sig1`:

| Request | Covered components, in order |
|---|---|
| With a body (`POST`) | `"@method" "@target-uri" "content-digest" "content-type" "rim-origin"` |
| Without (`GET`) | `"@method" "@target-uri" "rim-origin"` |

Parameters, in order: `created`; `expires` (at most `created + 300`); `nonce` (16 random bytes,
unpadded base64url); `keyid` (`"<serverId>#<seq>"`, the signing key's log entry); `alg` (the key's
algorithm name, `"ed25519"`); `tag` (`"rim-s2s-v1"`).

- `Content-Digest: sha-256=:<base64 SHA-256 of the body>:` (RFC 9530, padded base64); `sha-256`
  must be present and match, other algorithms are ignored. `Content-Type: application/json`, except
  forwarded requests, which keep the client's.
- `Rim-Origin`: the sender's origin, where its discovery document lives.
- `@target-uri` is the absolute URI the sender used. The receiver rebuilds it from the origin of its
  `RIM_PUBLIC_BASE_URL` plus path and query, never from `Host` or forwarded headers, so a
  TLS-terminating proxy does not change it.
- The base is RFC 9421 §2.5: `"<component>": <value>` lines, then `"@signature-params"`, joined by
  `\n`, no trailing newline (A.4). `Signature-Input` and `Signature` are RFC 8941 dictionaries;
  other labels are ignored.

The `nonce` parameter is RFC 9421's own replay field and is covered by `@signature-params`.
Rejected: a separate `rim-nonce` header (a second field for one purpose) and `Date` (second
resolution; `created` is covered already).

### Verifying a request

In order, stopping at the first failure:

1. Body at most `RIM_S2S_MAX_BODY_BYTES` (8 MiB; 32 MiB on `/forward/`) → `413 too_large`.
2. Exactly the profile's components and parameters; `created ≤ now + 300`, `expires > now`,
   `expires − created ≤ 300` → `401 bad_signature`.
3. Peer: denied, `blocked` or `forked` → `403`; an unknown id is pinned from `Rim-Origin` first
   (`403 unknown_peer` on failure). The host allow-list applies here ("Configuration").
4. `keyid`'s seq is the pinned head, or the previous key in its grace period; a higher seq
   refreshes the pin once. Digest matches; signature verifies (`verify_strict`) → else `401`.
5. Insert `(peer, nonce, expires_at = created + 600 s)` into `s2s_nonce`; a conflict is
   `409 replay`. Expired rows are deleted every minute.
6. Rate limits per peer → `429` with `Retry-After`.

### Responses

Every response from the RIM layer, errors included, is signed by the responder, covering
`"@status" "content-digest" "content-type" "rim-request-nonce" "@method";req "@target-uri";req`
plus `"content-digest";req` when the request had a body, with `created`, `keyid`, `alg`, `tag`.
`Rim-Request-Nonce` echoes the request's nonce and binds the answer to it. RFC 9421 §2.4 advises
against covering the request's `Signature` field, hence the echo. A caller treats an unsigned or
badly signed response as a transport failure (retry), never as an answer, and refuses a response
whose `created` is more than 300 s from its clock. A refusal of a request whose nonce cannot be
read echoes an empty `Rim-Request-Nonce`; no caller can match it. While server-to-server traffic
is off, `/s2s/v1/…` answers an unsigned `404`.

### Errors

Bodies are `{"error":"<code>","message":"…"}`.

| Status | `error` | When |
|---|---|---|
| 400 | `bad_request` | Malformed JSON or statement |
| 401 | `bad_signature` | Profile, clock, key, digest or signature |
| 403 | `unknown_peer`, `peer_not_allowed`, `peer_forked`, `not_delegated`, `forbidden` | Peer not pinnable; denied or off the allow-list; forked; no delegation or card names the relay; action refused |
| 404 | `not_found` | Unknown principal, space or conversation |
| 409 | `replay` | Nonce seen; device request `seq` refused (`highestSeq` in the body) |
| 413, 429 | `too_large`, `rate_limited` | Caps; `Retry-After` |
| 500 | `internal` | The responder failed (its database, say); retry |
| 503 | `role_disabled` | The endpoint belongs to a role this server does not run |

A request in a method other than `GET` or `POST` fails the profile (`401`). A peer with status
`blocked` gets `403 peer_not_allowed`.

### Ping

`GET /s2s/v1/ping` → `200 {"serverId":"rims1_…","roles":["host","relay"]}`, served by every role.
It runs the whole request check and signs its answer, so it tests a pin and the transport both
ways.

### Implementation

New crate **`rim_s2s`** (no database, no Actix): signature bases, signing and verifying this
profile, `Content-Digest`, and the server-signed statements. Structured fields through **`sfv`
0.15.0** (2026-06-16; RFC 8941/9651); Ed25519 through `rim_identity`'s key encodings. Its tests
check RFC 9421 appendix B.2.6 ("Signing a Request Using ed25519") and appendix A here byte for
byte. Outgoing requests use `reqwest` with rustls behind a signing client in `rim_infra_services`.

Considered on crates.io, 2026-10-09: **`httpsig`** 0.0.26 (2026-07-22, MIT; Ed25519,
`Content-Digest`, response binding, but its README says work in progress and hyper-only
integration); **`web-bot-auth`** 0.7.0 (2026-04-22, Apache-2.0, Cloudflare; built for the Web Bot
Auth profile, generic use undocumented); `actpub-httpsig` (ActivityPub-shaped). A fixed profile
with one algorithm is about 400 lines against vectors, a smaller surface than any of them, and fits
Actix's request types.

## Roles and configuration

| `RIM_ROLE` | Client API | S2S endpoints |
|---|---|---|
| `both` (default) | Everything | All |
| `relay` | All but `createServer` (`FORBIDDEN`); existing workspaces keep working for local users | Discovery, `ping`, `principals`, `identity`, `key-packages/claim`, `deliver`, `presence`, and host endpoints (`identity`, `outbox`, `forward`) for its direct space only |
| `host` | All but `/api/remote/…`, contacts, DM requests and remote spaces; local users DM inside workspaces | Discovery, `ping` and host endpoints; `deliver` answers `503` |

| Variable | Default | Meaning |
|---|---|---|
| `RIM_PUBLIC_BASE_URL` | unset | S2S is on only when it is `https` (or `http` with `RIM_S2S_ALLOW_HTTP`); its origin is `Rim-Origin` and the base of `@target-uri` |
| `RIM_SERVER_KEY_FILE` | `<RIM_DATA_DIR>/server-key.json` | The server key seed |
| `RIM_HOST_ALLOWED_RELAYS` | empty = any | Server ids allowed on host endpoints (`join`, `subscriptions`, `forward`, `revocations`, `presence`) |
| `RIM_S2S_DENIED_PEERS` | empty | Server ids refused everywhere |
| `RIM_S2S_PEER_RATE_PER_MIN` | 600 | Requests per peer (in memory, per node) |
| `RIM_S2S_SUBSCRIPTIONS_PER_PEER` | 10000 | Active delegations per relay on a host |
| `RIM_S2S_DEVICE_RATE_PER_MIN` | 120 | Forwarded requests per device |
| `RIM_S2S_MAX_BODY_BYTES` | 8388608 | Body cap (forward path: 32 MiB) |
| `RIM_RELAY_QUEUE_BYTES_PER_USER` | 268435456 | Queued bytes per local user |
| `RIM_RELAY_RETENTION_DAYS` | 30 | Queue and DM request retention |
| `RIM_REGISTRATION_POW_BITS` | unset | Turns on `pow` registration |

The existing `config` loader (`Environment::with_prefix("RIM")`) maps `RIM_ROLE` to the field
`role`. From S5-1 on a login proof's audience may also be this server's id, compared exactly
(base64url is case-sensitive), beside `RIM_KEY_LOGIN_AUDIENCES`.

### Remote users on a host

Created the first time a host accepts a person from elsewhere (a join, a DM a local user starts, a
delegation):

| Column | Value |
|---|---|
| `kind`, `principal_id`, `identity_key` | `remote`; from the verified key log |
| `username` | `r_` + the first 30 characters after `rimp1_`. From S5-1 on local signups refuse the `r_` prefix, so nobody squats a principal's name; existing local names are left alone |
| `display_name` | The card's `displayName`, updated with each newer card |
| `password_hash`, `email` | Null (allowed: `principal_id` is set) |
| `home_relay` | Server id of the relay that most recently presented a valid delegation for the user, else the first relay of their newest card that pins, else the relay that served their identity; null for local users |

Key log, devices (with certificates and revocations) and card live in the existing
`key_log_entry`, `device_key` and `profile_card` tables, as for an enrolled local user. A key-only
signup for a principal held here as `remote` turns the row `local` and keeps its memberships.
Usernames stay unique across the table; rejected: a partial unique index on local users only,
since login and member lookups assume one row per name.

Remote users may not run mutations that change the account rather than a space (`FORBIDDEN`, `not
available to remote members`): `login`, `logout`, `signup*`, `enrollIdentityKey`, `addDeviceKey`,
`revokeDeviceKey`, `rotateIdentityKey`, `publishProfileCard`, `logoutOtherSessions`, password and
email mutations, `updateProfile`, `createServer`, ingest token and bot mutations. Identity changes
arrive by `POST /identity` and `POST /revocations`.

## Statements added by this document

All follow key-identity's statement rules, version 2 only. Timestamps are informational, except a
delegation's `expiresAt`, which the host enforces against its own clock.

| Statement | Context bytes | Signed by | Max payload | PR |
|---|---|---|---|---|
| Server key log entry | `rim.server-key.v1\0` | server key or server recovery key | 16 KiB | S5-1 |
| Server descriptor | `rim.server-descriptor.v1\0` | head server key | 16 KiB | S5-1 |
| Device request | `rim.device-request.v1\0` | device key | 16 KiB | S5-2 |
| Host response | `rim.host-response.v1\0` | host server key | 16 KiB | S5-2 |
| Host event | `rim.host-event.v1\0` | host server key | 512 KiB | S5-2 |
| Delegation | `rim.delegation.v1\0` | device key | 16 KiB | S5-4 |
| Revocation acknowledgement | `rim.revocation-ack.v1\0` | host server key | 16 KiB | S5-4 |

Device-signed statements are verified in `rim_identity` against a key log, server-signed ones in
`rim_s2s`. Both take the algorithm from the tagged key, so a hybrid key is added in one place.

## Spaces hosted elsewhere: the client's path

### Device-signed requests

A request to a space hosted elsewhere carries `Rim-Device-Request` (unpadded base64url of the exact
payload text) and `Rim-Device-Signature` (the tagged signature over
`rim.device-request.v1\0 || payload`):

```json
{"v":2,"type":"device-request","principalId":"rimp1_…","deviceId":"<uuid>","host":"rims1_…",
 "method":"POST","path":"/api","seq":1791504000000000,"bodySha256":"<base64url>","issuedAt":"…"}
```

| Field | Rule |
|---|---|
| `host` | The host's server id |
| `method`, `path` | The host-relative request (`POST /api`, `GET /api/attachments/<id>/download`, `POST /api/files/upload`, `GET /api/plugin-bundles/<server>/<sha256>`); must equal the forwarded one |
| `seq` | Below 2⁵³, greater than every `seq` this device used with this host; clients use `max(previous + 1, Unix time in µs)` and persist it |
| `bodySha256` | base64url(SHA-256(exact body bytes)); the empty body's hash for `GET` |

The host accepts a `seq` it has not accepted from this device in the last 10 minutes and that is
above `highestSeq − 300,000,000` (5 minutes in µs), then raises `highestSeq`. An accepted `seq` is
remembered for 10 minutes and, after that, until it falls below the window: forgetting it at 10
minutes would let a relay replay a device's latest request once the device went quiet. Otherwise
`409 {"error":"replay","highestSeq":"<decimal>"}`; the client moves its counter above it and signs
once more. The window tolerates the relay reordering concurrent requests and needs no server clock.
Rejected: a strict `+1` counter (a crash between signing and persisting locks the device out;
concurrent requests fail).

### Forwarding

The relay serves `GET|POST /api/remote/<hostServerId>/<host-relative path>` (a non-GraphQL
transport in `rim_server`, in the `file_handlers.rs` pattern); GraphQL is
`POST /api/remote/rims1_…/api`. It authenticates the session cookie (and CSRF for `POST`) as today,
refuses a device request naming another principal or device than the session's or a revoked
device (`FORBIDDEN`), and sends `<host s2s>/forward<path>` with the body byte for byte, the client's
`Content-Type`, both device headers and its own signature. It returns the host's status, body,
`Content-Type`, `Rim-Host-Response` and `Rim-Host-Signature` unchanged.

The host's `/s2s/v1/forward/…` checks the S2S signature, then the device request (a non-revoked
device of a user here; `host` is its own id; method, path, body hash; `seq`), then that the relay
may carry this user's traffic: it holds an active delegation of this device for a workspace here,
or the user is a member of this host's direct space and the relay's origin is in their newest card
(`403 not_delegated` otherwise). It runs the normal handler (GraphQL, upload, download) as that user
and device, with no session; CSRF does not apply without cookies. Per-IP limiters key on
`relay:<serverId>:<principalId>` for forwarded requests.

Every forwarded response carries `Rim-Host-Response` (base64url payload) and `Rim-Host-Signature`:

```json
{"v":2,"type":"host-response","host":"rims1_…","keySeq":0,"requestSha256":"<base64url>","status":200,"bodySha256":"<base64url>"}
```

`requestSha256` hashes the device-request payload text, so a relay cannot answer a new request with
an old response. The app checks signature, pin, request hash, status and body hash; a failure counts
as a relay failure ("Direct connection"). The host buffers bodies to hash them (downloads ≤ 25 MB).

**Coexistence.** Spaces this server hosts are used exactly as today (`/api` with cookie and
`X-CSRF-Token`, `workspaceEvents` on `/api/ws`); only spaces hosted elsewhere use `/api/remote/…`.
The request authenticator gains a second path (device request via a pinned peer) yielding the same
`RequestAuthResultDto` with no session id; `AuthActorCtx` already carries the device. Rejected: one
S2S endpoint per client operation (hundreds of endpoints drifting from the schema, while the
decision requires writes to reach the host unchanged).

## Host events and relay queues

### Host events

```json
{"v":2,"type":"host-event","host":"rims1_…","keySeq":0,"workspaceId":"<uuid>","stream":"c:<uuid>","seq":42,
 "event":{"type":"MessageDeleted","workspace_id":"…","conversation_id":"…",…},"issuedAt":"…"}
```

`event` is the `WorkspaceEvent` JSON clients parse today. Its ids are the host's; clients key every
entity of a remote space by (host server id, id).

| `stream` | Events | `seq` |
|---|---|---|
| `c:<conversationId>` | Every event with a `conversation_id` (messages, reactions, pins, threads, conversation membership, link previews, incidents, `MlsCommitAccepted`, `MlsMembershipChanged`) | 1, 2, … |
| `u:<principalId>` | Events for one user (`NotificationDue`, `UserMentioned`, `ReadStateUpdated`, `MlsWelcomeAvailable`, `DmInvite`); for a guest, the directory events about them | 1, 2, … |
| `w` | Other space-wide events (`MemberJoined`, `MemberLeft`, `MemberRoleChanged`, `WorkspacePluginChanged`, pipeline and workspace-state events), to non-guests | 1, 2, … |
| `e` | `TypingStarted`, `PresenceChanged` | `null` |

`seq` is assigned per `(workspace, stream)` in `host_event_stream`, in the transaction that writes
the outbox rows. Recipients follow today's `user_can_see_event_as`, which S5-2 moves from `rim_api`
to `rim_services` for the outbox writer. `streamHeads(serverId, streams)` (host GraphQL, through
the forward path) answers the current `seq` of each stream the caller can see.

### Outbox and `deliver`

For each event the host appends one `s2s_outbox` row per receiving relay: relays holding an active
delegation of a recipient, or for a direct space the relays in each remote recipient's newest card.
A task per peer sends them in id order:

```
POST /s2s/v1/deliver
{"items":[{"id":"1234","envelope":{"payload":"…","signature":"…"},"recipients":["rimp1_…"],"deviceId":null}]}
→ 200 {"ackedThrough":"1234","refused":[{"id":"1230","reason":"unknown_principal"}]}
```

- At most 256 items and 8 MiB per request; `recipients` are the relay's users who may see the event;
  `deviceId` targets one device (`MlsWelcomeAvailable`).
- The relay stores items before answering and ignores ids at or below its `received_through` for
  the peer, so redelivery is idempotent. It refuses items for non-local principals
  (`unknown_principal`), items of a conversation it holds no invite or request for and workspace
  items without an active subscription (`not_subscribed`), invites from a principal the recipient
  blocked (`blocked`) or past the request limits (`limited`), a device-targeted item for an unknown
  device (`unknown_device`), and envelopes that do not verify under the delivering host's key or
  name another host (`invalid_envelope`). A refusal for one recipient carries `principalId`. The
  host skips a (relay, principal) for 24 hours after `unknown_principal` only; the other refusals
  are about one conversation or item.
- Failures back off from 1 s doubling to 5 minutes. Outbox rows live 7 days; a relay down longer
  gets nothing and its clients catch up from the change feed. A relay may pull what it missed:
  `GET /s2s/v1/outbox?after=<id>&limit=256` → `{"items":[…],"hasMore":false}`.

### Relay queues

The relay stores each envelope once (`relay_envelope`), one `relay_item` per recipient user, and a
cursor per device (`relay_device_cursor`): each device's queue is its user's items above its cursor.

- **Push**: a subscription `relayStream(after: ID)` on the existing `/api/ws` yields items above
  `after`, then live ones: `{"type":"RelayItem","id":"981","hostServerId":"rims1_…","workspaceId":"…","envelope":{…}}`,
  plus notices: `{"type":"DmRequestReceived","requestId":"<uuid>"}`, `RevocationAcked` and
  `{"type":"QueueOverflow","spaces":[{"hostServerId":"rims1_…","workspaceId":"<uuid>"}]}`.
- **Pull** `relayItems(after, limit)`; **ack** `ackRelayItems(upTo)` moves the session device's
  cursor. A newly linked device starts at the newest item and reads history from the host's feed
  and the archive.
- **Retention**: an item goes once every non-revoked device of its user acked it, and after
  `RIM_RELAY_RETENTION_DAYS` (30) regardless. Over the per-user byte cap the oldest go first and a
  `QueueOverflow` notice names the affected spaces. The notice is pushed to the devices online and
  not queued (a queued item is a host-signed envelope); a device offline then sees the gap in the
  streams' `seq`. Plaintext channel events are readable by the relay
  while queued, as the relay decision accepts.

### What the client checks

For every `RelayItem`: the signature under the pinned key at `keySeq` (refetching on a newer seq);
`host` is the item's; `workspaceId` is a space the app has from that host; the stream matches the
event (`c:` its conversation, `u:` the app's principal). Then per stream:

- `seq ≤ last`: duplicate, dropped. `seq = last + 1`: applied.
- `seq > last + 1`: held up to 30 seconds or 256 events for the missing ones, then the app
  reconciles through host-signed responses (change feed, pins and reactions of the open
  conversation, members, `streamHeads`) and sets `last` to the head. Only if that fails does it show
  "Messages may be missing from <space>" with "Connect directly".
- A stream's baseline is its `streamHeads` value when the space opens, or the first event seen.
  Events arriving by relay and by direct connection are deduplicated the same way.

## Profile cards and contacts (7.4a)

- **Publishing.** `relays[]` is the person's own list, preferred first. The app signs a card
  (version + 1) and calls `publishProfileCard` on each listed relay it has an account on; each relay
  pushes it to every host that holds the person as remote (`POST /s2s/v1/identity`): the hosts it
  queued items or filed requests from for the person, and in S5-4 those holding their
  subscriptions. Device additions and revocations are pushed the same way. The answer is
  `{"applied":true|false}`.
- **Identity endpoint** (relays, for local users): `GET /s2s/v1/principals/<principalId>` →
  `{"principalId","keyLog":[…],"devices":[{"deviceId","certificate","revocation","mlsActive"}],"card"}`,
  `404` for others. `POST /s2s/v1/identity` takes the same JSON; the receiver applies it only for
  principals it holds as remote: a log extending its copy, new devices with valid certificates,
  revocations, a card of higher version.
- **Fetching.** `remoteIdentity(principalId, relayUrl)` answers local principals directly and
  otherwise fetches from `relayUrl` (pinning it), cached 24 hours. The app verifies the log against
  the principal id, each certificate against the log and the card against the head, as
  `e2ee-mls-v1.md` requires for leaves. `principalDevices` answers remote users from the stored
  copy, refreshed from their relay when older than an hour.
- **Contacts.** A contact link is `rim-contact:1:<principalId>:<relayUrl>` (relay URL rules as a
  device-link endpoint: 1 to 256 bytes of printable ASCII, no `?`, `#`, `@`), shown as a byte-mode
  QR code. The principal id authenticates everything fetched with it. Rejected: the card in the link
  (up to 128 KiB, too large for a QR code, stale after a relay switch). The app reads a shared
  workspace member's card with `remoteIdentity(principalId)` (the relay answers its own principals
  directly) to offer "Add as contact". The contact list answers open decision 9 (readable names):
  the app shows the card's name or the person's alias for it; aliases stay on the device. The relay
  keeps the list (`relay_contact`) because it filters DM requests by it.

## The direct space

Each relay has one `server` row with `kind = 'direct'`, created at start if missing, named "Direct
messages", not listed by `servers`, with no owner, channels, invites or plugin registry;
`relayInfo { serverId roles directSpaceId }` gives its id. Every member is a `guest`, so today's
guest rules already hide the member list and directory events; the conversation service lets
guests create groups in a direct space and refuses channels there. Local users join on first use,
remote users when a local user adds them to a conversation.

A DM or group there has `conversation.server_id` = the direct space, so "a DM in no workspace"
belongs to its home relay's direct space and every table, event and query keyed by `server_id`
works unchanged. Rejected: a nullable `conversation.server_id` (every conversation query, every
event's `workspace_id` and the per-space subscription would change). DMs inside workspaces stay
possible; the app starts new DMs in the direct space when every member has a principal id.

## DMs across relays (7.4a)

Alice's relay R_A is the home; Bob's relay is R_B. Groups work the same way.

1. **Create.** Alice calls `createConversation(serverId: <directSpaceId>, encrypted: true,
   remoteMembers: [{ principalId, relayUrls }])` on R_A. R_A fetches Bob's identity from the first
   relay that answers, creates or refreshes his remote user row and direct-space membership, and
   checks readiness from `mlsActive`. A pair with a DM there gets it back, as today. One entry in
   `remoteMembers` and no `memberIds` makes a DM, more make a group; the app names every member
   this way, those on R_A included. The app creates a conversation with anyone on another relay
   only encrypted: when a member or Alice's device is not ready it creates nothing and says who.
   R_A refuses such a conversation unencrypted (`VALIDATION`), and an encrypted one when a member
   has no MLS-active device. A person from another relay may create one too, through the forward
   path, naming R_A's members the same way.
2. **Key packages.** Claims for Bob's devices go to R_B: `POST /s2s/v1/key-packages/claim`
   `{"principalId","devices":["<uuid>"],"conversation":{"host","conversationId","incarnation"},"claimer":{"principalId","deviceId"},"cipherSuite":79}`
   (`cipherSuite` optional, the preferred suite when absent; `conversation.host` must be the
   calling peer)
   → `{"keyPackages":[{"deviceId","keyPackage","keyPackageRef","lastResort"}]}`, by today's claim
   rules. A claimer who is not Bob's contact, was never accepted by him, and shares no workspace R_B
   holds a subscription for, is a **stranger**: last-resort packages only, 20 claims per claimer
   principal per day. R_A stores the packages as claimed `mls_key_package` rows of Bob's device rows,
   so `submitMlsCommit`'s claim check runs unchanged.
3. **Invite.** When a remote member is added to a conversation of the direct space (at creation or
   by `addConversationMembers`), R_A emits `DmInvite` on the member's `u:` stream,
   `{"type":"DmInvite","workspace_id","conversation_id","from_principal_id","from_card","member_principal_ids"}`,
   before the conversation's own events. `from_*` name the conversation's creator; `from_card` is
   their published card as `{"payload","signature"}` (a conversation with remote members needs one),
   and its relays tell Bob's app where to pin R_A from. The Commit that adds the member's devices
   then emits `MlsWelcomeAvailable` per added device. That is how R_B and Bob's devices learn of it;
   a plaintext conversation has an invite and no Commit.
4. **Traffic.** Bob's devices fetch Welcomes and Commits, read the change feed, send, edit, react
   and mark read through `/api/remote/<R_A>/api`; R_A runs them as Bob's device, and its events reach
   Bob's devices through R_B's queue. The envelopes are exactly the `message.encryption` R_A stores.
5. **Sequencer.** R_A alone: one Commit per epoch, the change feed's `version`, read states. R_B
   holds no conversation row. R_A keeps the conversation like any server today (Commits 90 days,
   Welcomes 30 days); R_B's items follow "Relay queues".

Rejected: a sequencer that moves between relays or a vote between them (the state resolution the
2026-10-08 decision rejected), and storing the conversation on both relays (two truths for epochs).

**When the home is gone**: unreachable directly and through the member's relay for 7 days, or
absent from the creator's newest card and not answering. Any member's app may then create a
**successor** in its own relay's direct space with the same members and
`predecessor: { hostServerId, conversationId }`; apps show both as one timeline from the cache and
archive. Of two successors the one with the smaller id is canonical (ids are UUID v7, so this is
the earlier) and members leave the other. Members learn a conversation is a successor from
`Conversation.predecessor`. The creator's app also makes successors when its person leaves the relay.

### The request inbox

R_B files a `DmInvite` from a principal outside Bob's contacts, never accepted and sharing no
workspace R_B hosts with him as a **DM request** (`relay_dm_request`): the sender's card, the conversation, and up to 3 held items of it,
the `DmInvite` first (later ones dropped until accepted; the app reads them from the host). The
app verifies the held `DmInvite` like any relay item to bind the conversation's space to its home
before it reads or leaves it; ignoring or blocking keeps it with the request until the row
expires. Only `DmRequestReceived` is pushed. An invite from a contact or a principal accepted
before is recorded as a row accepted at once (`decided_at = created_at`), not listed by
`dmRequests` and not counted by the limits, so every conversation R_B delivers for has a row and
items of any other conversation are refused.

When the home and the recipient's relay are one server (Bob's relay hosts the conversation, and
its creator is someone else, from another relay or not), the home files its own `DmInvite` for
Bob through the same rule instead of delivering it. The invite and the conversation's events are
held as above, and until Bob accepts, the conversation is left out of his `inboxConversations` and
its events out of his `workspaceEvents`; ignoring keeps it out, and so does blocking its creator.
Once accepted it is an ordinary conversation of the space he uses directly.

| Action | Effect |
|---|---|
| `acceptDmRequest(id)` | Held items released; later items flow; the sender becomes a contact |
| `ignoreDmRequest(id)` | Held items deleted but the `DmInvite`; later items of that conversation dropped; row expires in 30 days |
| `blockPrincipal(principalId)` | Pending requests from it ignored; its `DmInvite`s refused (`blocked`) and its claims refused; the app leaves the conversation through the home |
| Nothing | Expires after 30 days |

The app shows the card's name (verified against the sender's key log) and a fingerprint of the
principal id, `base32(first 10 bytes of SHA-256("rim.principal-fingerprint.v1\0" || principal id))`
in four groups of four, the same on every device, and may process the Welcome to read the held
messages before deciding. It accepts on its own when the sender shares a workspace
with the person (checked on host-signed member lists): workspace members are never strangers.
Limits: 100 pending per recipient, 20 per sender principal per day per relay;
`reportDmRequest(id, reason)` reports abuse: until `relay_abuse_report` exists (S5-6) it is the
audit event `relay.dm_request_reported`, and a pending request is ignored.

## Remote workspace members (7.4b)

### Joining

```
app ─────────────────────► relay R ─────────────────────────► host H
remoteServer(origin)              GET /.well-known/rim-server               (pin H from the invite)
remoteJoinChallenge(H, origin)    POST /s2s/v1/join/challenge → {challengeId, nonce, expiresAt}
sign a login proof (audience = H's server id) and a delegation
joinRemoteWorkspace(input)        POST /s2s/v1/join           → {workspaceId, userId, role, streamHeads}
relayStream                  ◄─── POST /s2s/v1/deliver         (events)
```

1. The invite link is read like a device-link code: whitespace removed, at most 400 characters,
   split at the first four `:` after the scheme (the origin keeps its colons). The app pins H.
2. The challenge is today's `keyLoginChallenge` (120 s, single use) over S2S. The device signs a
   `login` statement with `audience` = H's server id, so the proof is good only at H and the relay
   never vouches for the person.
3. The device signs a delegation. R sends H `{"inviteToken","keyLog":[…],"devices":[{"certificate","revocation"}],"card","proof","delegation"}`.
4. H verifies log, certificates, proof (claiming the challenge) and invite token (the rules of
   `joinServerByInvite`, bans included), creates or updates the remote user, adds the membership
   with the invite's role, stores devices and card, accepts the delegation, and answers
   `{"workspaceId","userId","role","streamHeads":{"w":17,"u:rimp1_…":0}}`. R records the
   subscription; the app loads the space through the forward path.

A current member (an earlier join, or an existing local account on H) sends `inviteToken: null`.

### Delegation

```json
{"v":2,"type":"delegation","principalId":"rimp1_…","deviceId":"<uuid>","relay":"rims1_…","relayKeyHash":"…",
 "host":"rims1_…","workspaceId":"<uuid>","scope":"workspace","nonce":"<16 bytes>","issuedAt":"…","expiresAt":"…"}
```

Signed by the device key over `rim.delegation.v1\0 || payload`: "relay `relay` may receive this
workspace's events for me and forward my device-signed requests to `host` until `expiresAt`". It
never lets the relay act as the person; writes still need device signatures.

A host accepts it (on `join` or `POST /s2s/v1/subscriptions`) when the signature verifies under a
non-revoked device of the principal (an unknown device is fetched once from the presenting relay's
`principals`); `host` is its own id and `relay` the presenting peer; `relayKeyHash` names a key of
that relay's log with no `signer: recovery` rotation after it (an ordinary rotation keeps
delegations, a recovery kills them); the principal is a current, unbanned member of `workspaceId`;
`scope` is `workspace`; `expiresAt − issuedAt ≤ 24 h` and `expiresAt` is ahead. It keeps it until
`min(expiresAt, accepted_at + 24 h)` by its own clock, so a device clock cannot extend it. One row
per (workspace, device, relay); a newer one replaces it.

**Subscriptions.** `POST /s2s/v1/subscriptions {"delegations":[…]}` (at most 64) →
`{"subscriptions":[{"workspaceId","status","expiresAt"}]}`, one entry per delegation in order,
`status` `active` or `refused` with a `reason`. A relay's subscription for (user, workspace) lives
while one of the user's devices has an active delegation there.
`POST /s2s/v1/subscriptions/cancel {"subscriptions":[{"workspaceId","principalId"}]}` ends it.
Leaving, a kick or a ban ends the user's delegations for the workspace after `MemberLeft` is sent.

**Renewal.** An online device signs fresh delegations when the current ones have less than 6 hours
left and sends them with `renewDelegations`; the relay presents them. When every device has been
offline 24 hours the subscription lapses, the host stops queuing for it, and on return the app
renews and catches up from the change feed and `streamHeads`. Short delegations bound a hidden
revocation, as the relay decision requires. Rejected: a delegation for the whole host (a stolen
relay key would reach every workspace there), and long-lived delegations with explicit revocation
only (a hidden revocation would never end).

**Presence.** The relay reports `POST /s2s/v1/presence {"updates":[{"principalId","online"}]}` for
subscribed workspaces when a person's first WebSocket opens or the last closes (30 s debounce).

Members, roles, bans, timeouts, private channels, archive state, the plugin registry and bundles,
search, notifications, the change feed and MLS sequencing stay the host's, checked by the same
services for remote members. Bundles and attachments come through the forward path, so the web app
reads them same-origin from its relay.

## Revocation propagation

```
t = 0     app → relay: revokeDeviceKey(revocation)            the relay commits it locally
t = 0     relay → every host and DM home holding the person: POST /s2s/v1/revocations
          retries at 10 s, 30 s, 1 m, 2 m, 5 m, then every 15 m until 24 h
t ≤ 2 m   app polls deviceRevocationStatus(deviceId), or gets RevocationAcked on relayStream
t = 2 m   app → each host without a verified ack, directly: applyRemoteRevocation(revocation)
t = 24 h  every delegation the revoked device signed has expired regardless
```

`POST /s2s/v1/revocations {"principalId","revocation":{"payload","signature"}}` → `{"ack":{…}}`:

```json
{"v":2,"type":"revocation-ack","host":"rims1_…","keySeq":0,"principalId":"rimp1_…","deviceId":"<uuid>","revocationSha256":"<base64url>","appliedAt":"…"}
```

The host verifies the revocation against its copy of the log (identity key, or the device itself),
marks the device revoked, ends its delegations and direct sessions, raises `MlsMembershipChanged`
where it is a group device, and acknowledges; a repeat gets the same acknowledgement.
`applyRemoteRevocation` is the same on the host's GraphQL, without a session and exempt from CSRF
(it acts only on a fresh identity or device signature, like `loginWithKey`). The app verifies each
acknowledgement against the pinned key and its revocation's hash, and names any host it could not
reach: "<host> has not confirmed; the device keeps access to <workspace> until <time>", the
delegation's expiry at most. The relay stops forwarding anything the revoked device signs as soon as
it commits.

## Direct connection

The fallback when the relay is unreachable; desktop only in v1 (a browser cannot hold cookies for
another origin).

- **When.** After 60 seconds of failed relay requests (network errors, 5xx, bad host signatures on
  forwarded responses), or when "messages may be missing" persists after reconciling. The app asks
  once per host ("Connect directly to <host>? It will see your IP address") and remembers it.
- **How.** It fetches the host's discovery document directly and checks it against the pin, then
  calls `keyLoginChallenge` and `loginWithKey` on the host with `audience` = the host's server id.
  The host finds the remote user and device (refreshing its copy from the person's relays first,
  carrying on if they are down) and issues a device-bound session valid **24 hours** for a remote
  user. The app then uses the host's `/api` and `/api/ws` as a local client would, under a banner
  "Connected directly to <host>".
- **Back.** It probes `relayInfo` every 60 seconds; after two successes it renews delegations,
  reads `relayStream` from its last item, deduplicates by `(host, stream, seq)` and closes the
  direct connections.

## Switching relays

1. `signupWithKey` on the new relay R2 with the same identity.
2. Publish a card (version + 1) with `relays: [R2, R1]` to both; direct-space hosts deliver to both.
3. Sign a delegation for R2 for every subscribed workspace, including those R1 hosts (a delegation
   may name a local member too), and `renewDelegations` on R2.
4. Drain R1's `relayStream`; nothing new reaches R1 that does not also reach R2.
5. If R1 will go away, create successors on R2 for conversations homed in R1's direct space.
6. Publish `relays: [R2]` and call `leaveRelay` on R1, which cancels its subscriptions at every host
   and deletes the person's queue, contacts and requests there (the `user` row stays: it may still
   be a member of spaces R1 hosts).

History is on the devices and the drive, keys on the devices, spaces on their hosts: nothing is lost.

## Public relay (7.4c)

The public relay (`relay.rim.chat` is its working name; the domain is not registered yet, and
every mention of it here and in the runbook is a placeholder) runs the stock image with
`RIM_ROLE=relay`. In `pow` mode `signupWithKey` takes an
instance invite token or `proofOfWork`: an 8-byte nonce (unpadded base64url) such that

```
SHA-256("rim.pow.v1\0" || relay server id (ASCII) || "\0" || inception payload bytes || nonce)
```

starts with `powBits` zero bits (the descriptor's, from `RIM_REGISTRATION_POW_BITS`; 24 to start,
about 16.7 million hashes on average). It binds the work to this relay and this principal; the
server checks one hash; the open-signup per-IP limit applies too. Rejected: a server-issued puzzle
(a round trip and stored state for no gain) and memory-hard functions (no WebCrypto support).

| Quota per local user | Public relay |
|---|---|
| Queued bytes | 64 MiB |
| Card / key packages / devices | 1 card of 128 KiB / 100 unclaimed per device / 32 |
| Forwarded requests | 120 per device per minute, 50,000 per user per day |
| DM requests sent | 20 per sender principal per day |

**Abuse hooks** (S5-6), each audited: `rim_server principal block <principalId> --reason …`
(sessions, signups and deliveries refused; `relay_principal_block`); `rim_server peer block|unblock
<serverId>` (like `RIM_S2S_DENIED_PEERS` without a restart); `reportDmRequest` and the abuse
address feed `relay_abuse_report`, listed by `rim_server abuse-reports list`.

**Runbook** (`docs/oncall/relay-rim-chat.md`, S5-6): who holds the server recovery key and where,
yearly `server-key rotate` and `recover` on compromise, database and key file backups with a restore
drill, quotas and how to change them, monitoring (queue bytes, outbox depth per peer, deliver
failures, `401`/`409`/`429` rates, PoW signups), incident response for spam waves, a hostile peer, a
stolen server key and a database leak, and the abuse policy text.

**Compose** (`deploy/compose.relay.yml`): rim-server with `RIM_ROLE=relay`,
`RIM_PUBLIC_BASE_URL=https://relay.rim.chat`, `RIM_REGISTRATION_POW_BITS=24`,
`RIM_RELAY_QUEUE_BYTES_PER_USER=67108864`, `RIM_TRUST_FORWARDED_HEADERS=true`, `RIM_DATA_DIR=/data`
on a volume; Postgres 17 on its own volume; a TLS proxy passing `/api`, `/api/ws`, `/s2s/v1` and
`/.well-known/rim-server` with bodies up to 32 MiB.

## Migration of single-server deployments

The migrations are additive and the key is created at first boot. With `RIM_PUBLIC_BASE_URL` unset
the discovery document has `endpoints.s2s` null and nothing else changes. Existing accounts are
`local`, existing workspaces hosted here; the desktop app's card already lists this server as its
relay. Old invite links keep working; `createInvite` also answers the `rim-invite:1:` form when S2S
is on. `down` migrations refuse to run while remote users, delegations or queued items exist.

## Storage

One additive migration per PR (`sea migrate generate …`).

```sql
-- S5-1 add_server_identity
CREATE TABLE server_key_log_entry (seq integer PRIMARY KEY CHECK (seq >= 0),
    entry_type text NOT NULL CHECK (entry_type IN ('inception', 'rotate')),
    payload text NOT NULL, signature text NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE server_descriptor (id smallint PRIMARY KEY CHECK (id = 1), version integer NOT NULL,
    payload text NOT NULL, signature text NOT NULL, updated_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE peer_server (server_id text PRIMARY KEY, origin text NOT NULL,
    head_seq integer NOT NULL, head_key text NOT NULL, previous_key text NULL,
    rotated_at timestamptz NULL, rotation_signer text NULL CHECK (rotation_signer IN ('server', 'recovery')),
    descriptor_version integer NOT NULL, descriptor_payload text NOT NULL, descriptor_signature text NOT NULL,
    status text NOT NULL DEFAULT 'active' CHECK (status IN ('active', 'forked', 'blocked')),
    first_seen_at timestamptz NOT NULL DEFAULT now(), fetched_at timestamptz NOT NULL,
    last_request_at timestamptz NULL);
CREATE INDEX idx_peer_server_origin ON peer_server (origin);
CREATE TABLE peer_key_log_entry (server_id text NOT NULL REFERENCES peer_server ON DELETE CASCADE,
    seq integer NOT NULL, payload text NOT NULL, signature text NOT NULL, PRIMARY KEY (server_id, seq));
CREATE TABLE s2s_nonce (peer_server_id text NOT NULL, nonce text NOT NULL,
    expires_at timestamptz NOT NULL, PRIMARY KEY (peer_server_id, nonce));
CREATE INDEX idx_s2s_nonce_expires ON s2s_nonce (expires_at);

-- S5-2 add_relay_delivery
ALTER TABLE server ADD COLUMN kind text NOT NULL DEFAULT 'workspace',
    ADD CONSTRAINT chk_server_kind CHECK (kind IN ('workspace', 'direct'));
CREATE UNIQUE INDEX uq_server_direct ON server (kind) WHERE kind = 'direct';
ALTER TABLE conversation ADD COLUMN predecessor_host text NULL,
    ADD COLUMN predecessor_conversation_id uuid NULL;
ALTER TABLE device_key ADD COLUMN request_highest_seq bigint NULL;
CREATE TABLE device_request_seen (device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE,
    seq bigint NOT NULL, expires_at timestamptz NOT NULL, PRIMARY KEY (device_key_id, seq));
CREATE TABLE host_event_stream (workspace_id uuid NOT NULL REFERENCES server ON DELETE CASCADE,
    stream text NOT NULL, seq bigint NOT NULL, PRIMARY KEY (workspace_id, stream));
CREATE TABLE s2s_outbox (id bigserial PRIMARY KEY, peer_server_id text NOT NULL, item text NOT NULL,
    size_bytes integer NOT NULL, created_at timestamptz NOT NULL DEFAULT now());
CREATE INDEX idx_s2s_outbox_peer ON s2s_outbox (peer_server_id, id);
CREATE TABLE s2s_peer_delivery (peer_server_id text PRIMARY KEY, delivered_through bigint NOT NULL DEFAULT 0,
    failures integer NOT NULL DEFAULT 0, next_attempt_at timestamptz NOT NULL DEFAULT now(),
    last_success_at timestamptz NULL);
CREATE TABLE s2s_peer_inbound (peer_server_id text PRIMARY KEY, received_through bigint NOT NULL DEFAULT 0);
CREATE TABLE relay_envelope (id bigserial PRIMARY KEY, host_server_id text NOT NULL,
    workspace_id uuid NOT NULL, stream text NOT NULL, seq bigint NULL, payload text NOT NULL,
    signature text NOT NULL, received_at timestamptz NOT NULL DEFAULT now());
CREATE UNIQUE INDEX uq_relay_envelope_seq ON relay_envelope (host_server_id, workspace_id, stream, seq)
    WHERE seq IS NOT NULL;
CREATE TABLE relay_dm_request (id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE, from_principal_id text NOT NULL,
    card_payload text NOT NULL, card_signature text NOT NULL, host_server_id text NOT NULL,
    conversation_id uuid NOT NULL,
    status text NOT NULL DEFAULT 'pending' CHECK (status IN ('pending', 'accepted', 'ignored')),
    created_at timestamptz NOT NULL DEFAULT now(), decided_at timestamptz NULL,
    expires_at timestamptz NOT NULL, UNIQUE (user_id, host_server_id, conversation_id));
CREATE TABLE relay_item (id bigserial PRIMARY KEY, user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
    envelope_id bigint NOT NULL REFERENCES relay_envelope ON DELETE CASCADE,
    device_key_id uuid NULL REFERENCES device_key ON DELETE CASCADE,          -- one device only
    request_id uuid NULL REFERENCES relay_dm_request ON DELETE CASCADE,       -- held by a request
    size_bytes integer NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    expires_at timestamptz NOT NULL);
CREATE INDEX idx_relay_item_user ON relay_item (user_id, id);
CREATE INDEX idx_relay_item_expires ON relay_item (expires_at);
CREATE TABLE relay_device_cursor (device_key_id uuid PRIMARY KEY REFERENCES device_key ON DELETE CASCADE,
    acked_item_id bigint NOT NULL, updated_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE relay_contact (user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
    principal_id text NOT NULL, relay_urls text[] NOT NULL,
    added_at timestamptz NOT NULL DEFAULT now(), PRIMARY KEY (user_id, principal_id));
CREATE TABLE relay_block (user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
    principal_id text NOT NULL, created_at timestamptz NOT NULL DEFAULT now(),
    PRIMARY KEY (user_id, principal_id));
CREATE TABLE remote_identity_cache (principal_id text PRIMARY KEY, served_by text NOT NULL,
    identity text NOT NULL, fetched_at timestamptz NOT NULL);           -- the principals JSON

-- S5-4 add_remote_workspaces
CREATE TABLE host_delegation (id uuid PRIMARY KEY,
    workspace_id uuid NOT NULL REFERENCES server ON DELETE CASCADE,
    user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
    device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE,
    relay_server_id text NOT NULL, payload text NOT NULL, signature text NOT NULL,
    accepted_at timestamptz NOT NULL DEFAULT now(), expires_at timestamptz NOT NULL,
    ended_at timestamptz NULL, UNIQUE (workspace_id, device_key_id, relay_server_id));
CREATE INDEX idx_host_delegation_active ON host_delegation (workspace_id, relay_server_id, user_id)
    WHERE ended_at IS NULL;
CREATE TABLE relay_subscription (id uuid PRIMARY KEY,
    user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
    host_server_id text NOT NULL, workspace_id uuid NOT NULL,
    status text NOT NULL CHECK (status IN ('active', 'lapsed', 'ended')), expires_at timestamptz NULL,
    updated_at timestamptz NOT NULL DEFAULT now(), UNIQUE (user_id, host_server_id, workspace_id));
CREATE TABLE relay_revocation_fanout (device_key_id uuid NOT NULL REFERENCES device_key ON DELETE CASCADE,
    host_server_id text NOT NULL, status text NOT NULL CHECK (status IN ('pending', 'acked', 'gave_up')),
    attempts integer NOT NULL DEFAULT 0, next_attempt_at timestamptz NOT NULL,
    ack_payload text NULL, ack_signature text NULL, PRIMARY KEY (device_key_id, host_server_id));

-- S5-6 add_relay_abuse
CREATE TABLE relay_principal_block (principal_id text PRIMARY KEY, reason text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now());
CREATE TABLE relay_abuse_report (id uuid PRIMARY KEY,
    reporter_user_id uuid NULL REFERENCES "user" ON DELETE SET NULL,
    subject_principal_id text NULL, subject_server_id text NULL,
    request_id uuid NULL REFERENCES relay_dm_request ON DELETE SET NULL, reason text NOT NULL,
    created_at timestamptz NOT NULL DEFAULT now(), resolved_at timestamptz NULL);
```

A per-minute cleanup (like the key challenge cleanup) deletes expired nonces, `device_request_seen`
rows, relay items and orphaned envelopes, expired requests and outbox rows older than 7 days, and
ends expired delegations. Code follows the layers: repository traits in `rim_core` (`s2s`, `relay`),
`S2sService`, `RelayService` and `HostService` traits in `rim_services`, implementations in
`rim_infra_services`, handlers in `rim_server` (`s2s_handlers.rs`, `remote_handlers.rs`), wired in
`app.rs`.

## GraphQL

Additive, on the client's relay unless marked "host"; new objects append to the last groups.

```graphql
type RelayInfo { serverId: String!  roles: [String!]!  directSpaceId: ID! }
type RemoteServer { serverId: String!  origin: String!  discovery: String! }           # the raw document
type RemotePrincipalDevice { deviceId: ID!  certificate: SignedStatement!  revocation: SignedStatement
                             mlsActive: Boolean! }
type RemoteIdentity { principalId: String!  keyLog: [SignedStatement!]!  devices: [RemotePrincipalDevice!]!
                      card: SignedStatement  servedBy: String! }
type Contact { principalId: String!  relayUrls: [String!]!  addedAt: DateTime! }
enum DmRequestStatus { PENDING  ACCEPTED  IGNORED }
type RelayItem { id: ID!  hostServerId: String!  workspaceId: ID!  envelope: SignedStatement! }
type RelayItemPage { items: [RelayItem!]!  hasMore: Boolean! }
type DmRequest { id: ID!  fromPrincipalId: String!  card: SignedStatement!  hostServerId: String!
                 conversationId: ID!  heldItems: [RelayItem!]!  status: DmRequestStatus!
                 createdAt: DateTime!  expiresAt: DateTime! }
type RemoteSubscription { hostServerId: String!  workspaceId: ID!  status: String!  expiresAt: DateTime }
enum RevocationFanoutStatus { PENDING  ACKED  GAVE_UP }
type RevocationFanout { hostServerId: String!  status: RevocationFanoutStatus!  ack: SignedStatement
                        attempts: Int! }
type RemoteJoinChallenge { challengeId: ID!  nonce: String!  expiresAt: DateTime! }
type RemoteJoinResult { hostServerId: String!  workspaceId: ID!  userId: ID!  role: String! }
type StreamHead { stream: String!  seq: String! }                                    # host; decimal
type ConversationPredecessor { hostServerId: String!  conversationId: ID! }
type Conversation { predecessor: ConversationPredecessor }                            # additive field

input RemoteMemberInput { principalId: String!  relayUrls: [String!]! }
input PredecessorInput { hostServerId: String!  conversationId: ID! }
input CreateConversationInput { remoteMembers: [RemoteMemberInput!]  predecessor: PredecessorInput }   # additive fields
input SignupWithKeyInput { proofOfWork: String }   # additive field (S5-6)
input JoinRemoteWorkspaceInput { hostServerId: String!  hostOrigin: String!  inviteToken: String
                                 proof: SignedStatementInput!  delegation: SignedStatementInput! }

type Query {
  relayInfo: RelayInfo!
  remoteServer(origin: String!): RemoteServer!
  remoteIdentity(principalId: String!, relayUrl: String): RemoteIdentity!
  contacts: [Contact!]!
  dmRequests(status: DmRequestStatus): [DmRequest!]!
  relayItems(after: ID, limit: Int): RelayItemPage!                      # limit 1..500, default 200
  remoteSubscriptions: [RemoteSubscription!]!
  deviceRevocationStatus(deviceId: ID!): [RevocationFanout!]!
  streamHeads(serverId: ID!, streams: [String!]!): [StreamHead!]!        # host
}
type Mutation {
  addContact(principalId: String!, relayUrls: [String!]!): Contact!
  removeContact(principalId: String!): Boolean!
  acceptDmRequest(id: ID!): DmRequest!
  ignoreDmRequest(id: ID!): DmRequest!
  blockPrincipal(principalId: String!): Boolean!
  unblockPrincipal(principalId: String!): Boolean!
  reportDmRequest(id: ID!, reason: String!): Boolean!
  ackRelayItems(upTo: ID!): Boolean!
  remoteJoinChallenge(hostServerId: String!, hostOrigin: String!): RemoteJoinChallenge!
  joinRemoteWorkspace(input: JoinRemoteWorkspaceInput!): RemoteJoinResult!
  renewDelegations(delegations: [SignedStatementInput!]!): [RemoteSubscription!]!
  leaveRelay: Boolean!
  applyRemoteRevocation(revocation: SignedStatementInput!): SignedStatement!  # host; no session, CSRF-exempt
}
type Subscription { relayStream(after: ID): String! }                     # JSON strings, as workspaceEvents
```

Every new mutation but `applyRemoteRevocation` needs the CSRF header. A peer that cannot be reached
is the new code `UPSTREAM_UNAVAILABLE` (with `hostServerId`); a peer's S2S error keeps its meaning
(`FORBIDDEN`, `NOT_FOUND`, `CONFLICT`, `RATE_LIMITED`).

## Limits and audit

| What | Limit |
|---|---|
| S2S body / forward body / discovery fetch | 8 MiB / 32 MiB / 64 KiB |
| Clock window; nonce retention | `created` ≤ 300 s ahead, `expires − created ≤ 300`; 600 s |
| Deliver; outbox | 256 items and 8 MiB per request; 7 days |
| Relay queue | 30 days; 256 MiB per user (64 MiB on the public relay) |
| Delegations | ≤ 24 h; 64 per request; 10,000 per relay per host |
| Device requests | `seq` window 5 minutes; 120 per device per minute |
| DM requests | 100 pending per recipient; 20 per sender per day; 3 held items; 30 days |
| Stranger claims | Last-resort packages only; 20 per claimer per day |
| Remote identity | Log ≤ 256 entries; card 128 KiB; cache 24 h |
| Remote user's direct session | 24 h |

Rate limits are the in-memory per-node limiters; nonces, `seq` windows and cursors are in the
database, so correctness never depends on one node. Audit (ids only):
`s2s.peer_pinned`, `s2s.peer_rotated`, `s2s.peer_forked`, `s2s.peer_blocked`, `host.remote_join`,
`host.delegation_accepted`, `host.revocation_applied`, `relay.principal_blocked`,
`relay.registration_pow`, `server_key.created`, `server_key.rotated`, `server_key.recovered`,
`s2s.peer_reset`. Events with an acting user and workspace go to `command_audit_log`; the server
key and peer events have neither (its columns are `NOT NULL`), so they go to the audit log stream
(`target = "audit"`, as key logins do). Logs carry server, principal and row ids, never payloads. Metrics: outbox depth and oldest age per peer, deliver failures, relay
queue bytes, S2S responses by status, signature failures by reason.

## Threat model

| Adversary | Can | Cannot |
|---|---|---|
| Hostile relay | Read plaintext channel events and DM metadata it queues; drop, delay, reorder; refuse to forward; see which spaces its users use and when | Forge a write (no device key); forge or alter host events or responses (pinned host key); replay (`seq`, nonces); use a delegation for another workspace, host or relay; hide a revocation past 24 hours once the app reaches the host directly; read DMs (MLS) |
| Hostile host | What a server can for its own spaces today, plus remote members' principals, devices and activity there | Sign as a device; use a delegation at another host; read DMs homed elsewhere; see a remote member's IP outside a direct connection |
| Hostile peer | Send signed junk up to its limits; claim last-resort packages and send DM requests as a stranger | Impersonate another server (self-certifying id); replay; push events for spaces it does not host (envelopes name their host; relays accept only subscribed spaces); pass an allow-list |
| Stolen server key | Sign as that server until recovery: receive delegated events as a relay, sign false events to its own members as a host | Sign as users; extend delegations; outlive a `signer: recovery` rotation (peers drop the old key at once, delegations naming it die) |
| Stolen device | Whatever that device can do, through any relay or directly | Act after a host applies the revocation |
| Network attacker | See TLS metadata between servers | Alter bodies (digest, signature), replay, substitute a server at a pinned id |
| DNS or TLS compromise of a relay URL | Answer as that URL for card-based delivery and first-use pins | Present a pinned server id; take delegations (they name the id) |

**Metadata.** A relay learns its users' spaces, contacts, DM partners, activity and the size and
timing of all it queues; a host learns its remote members' principals, devices, activity and relays.
This is the exposure the relay decision accepts. Signatures are classical Ed25519 until server and
device keys move to hybrid.

## Where the existing code forces a compromise

| Today | Compromise | PR |
|---|---|---|
| `conversation.server_id` is non-null; DMs live in a workspace | The direct space: one `server` row of kind `direct` per relay, guests only | S5-2 |
| GraphQL authenticates by session cookie and CSRF | A second, device-signed path on `/s2s/v1/forward/…`; the cookie path unchanged | S5-2, S5-4 |
| One WebSocket, `workspaceEvents(workspaceId)` per space | `relayStream` on the same socket for everything hosted elsewhere | S5-2 |
| Events go through a lossy broadcast (capacity 1024; lagging receivers skip) | A separate, non-lossy observer feeds the outbox; events still fire after commit, so a crash in between leaves a gap the client reconciles | S5-2 |
| Visibility rules live in `rim_api/src/subscription.rs` | Moved to `rim_services` for the outbox writer | S5-2 |
| Audiences are lowercased `host[:port]` | The server id is accepted too, compared exactly | S5-1 |
| Usernames are unique across `user` | Remote rows get `r_…`; local signup refuses the prefix | S5-1 |
| `principalDevices` answers only principals sharing a workspace | Also remote users, and `remoteIdentity` | S5-2 |
| Per-IP rate limits | Keyed by relay and principal for forwarded requests | S5-2 |
| Sessions last 30 days | 24 hours for a remote user's direct session | S5-4 |

## Test plan

- **Unit.** `rim_s2s`: RFC 9421 B.2.6 and appendix A byte for byte; each refusal of "Verifying a
  request" (missing or reordered component, `expires` too far, `created` ahead, wrong `alg` or
  `tag`, digest mismatch, only another label); a response for another nonce fails. `rim_identity`:
  the new statements' vectors and refusals (v1 form, unknown fields, delegation over 24 h, wrong
  `host`, another body hash). Services with mock repositories: pinning (equal, extension, fork,
  recovery, grace), the `seq` window, delegation rules, stranger rules, request transitions, queue
  retention and overflow, the revocation schedule, the role table.
- **db-tests.** One nonce inserted twice concurrently, one wins; per-stream `seq` under concurrency
  without gaps or repeats; outbox order per peer; `received_through` idempotency; items deleted only
  once every device acked; cascades; each migration up and down, and the refusing `down`s.
- **e2e with two servers** (`rim_api/tests/s2s_e2e.rs`, `e2e-tests`): two Actix servers in one
  process with two databases, `RIM_S2S_ALLOW_HTTP` and `RIM_S2S_ALLOW_PRIVATE_ADDRESSES`. Flows:
  pinning and rotation; an encrypted DM across relays from creation to edit and read state
  (`rim_mls` fixtures); a DM request accepted, ignored, blocked; a remote join; channel traffic both
  ways with host events verified by a test client; a gap and its reconciliation; revocation with
  acknowledgement; the direct login; a relay switch; the allow-list; `relay` and `host` refusals;
  replayed and tampered requests. A two-server compose test in CI when CI is back (`ROADMAP.md`).
- **Clients (S5-3, S5-5).** Appendix A in the Tauri module and Vitest; pinning from an invite
  (current key, rotated key, fork); event verification and gaps; contacts by link and QR; a DM
  across relays on two desktops; the request inbox; a remote join; the direct fallback and its
  notice; a relay switch without losing a message.

## Deferred

- Hybrid Ed25519 + ML-DSA-44 server keys and S2S signatures (tagged keys, `alg` from the key and
  caps are ready).
- Matrix interoperability; encrypted private channels; relay discovery by DNS; multi-relay fan-out
  for reads; moving a workspace to another host.
- Proof-of-work on first contact (a `pow` field reserved for `DmInvite`); mobile push through
  relays; server ids in profile cards; `https://` invite and contact links.
- Deleting queued messages once the drive has them (open decision).

## Appendix A: known-answer vectors

Computed with Python `cryptography` 50.0.2; server id and request signature cross-checked with
Node 26 `crypto`. Keys are `SigningKey::from_bytes([seed; 32])`. The device is
`key-identity-v1.md` appendix B's (seed 20, `rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0`,
device `00000000-0000-0000-0000-000000000001`). Workspace `0199d1a0-0000-7000-8000-000000000002`,
conversation `…000000000003`, message `…000000000004`. JSON has fields in the order shown and no
spaces. Never use these values for real keys.

### A.1 Server inceptions

| Item | Relay (seed 30, recovery 130) | Host (seed 40, recovery 140) |
|---|---|---|
| Server key | `ed25519:rNsOKXQ_DMuGhtChBMuW4Fq-_sFTh2XnWVhp99yMSao` | `ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws` |
| Recovery key | `ed25519:-gE0epkeBK4PGm2B2yml6k7rWWX1d6ye-0TnsfL7sCg` | `ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw` |
| Server id | `rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c` | `rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw` |
| Key hash | `8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM` | `qkSWxxJEL_11l8_dTCGhHc_1Q1pAc2pMirMBGnz7388` |
| `prev` for seq 1 | `dMszB0YhzGkVSf4B-3elynzitpFsvVYojOEW8hXiNKg` | `oUvCbfqOS1bUTL5gAtene_faqghv8PaPgrRDIJpXLOM` |
| Inception signature | `ed25519:_oQe2QcbA37vNosXrUVHFf8iIbnaesyNMmPtj827Wx5Q6vt77tyaOYR6JA9e3Z2FjHDRs0uR0XQEj9TMwoIODA` | `ed25519:Dll_XFAqcMn9Pejfpge4oAGiEH16dAEu8KLjLjSEDPd8UFk3-tmRgzINC2G89uyREl1aDYI5bA-O8XOF7oftAg` |

Host inception (the relay's has its own keys in the same fields):

```
{"v":2,"type":"server-inception","seq":0,"serverKey":"ed25519:6S62BU_pvGgqG887dZ9lqzikz72BxNHzNC5Myc3tiws","recoveryKey":"ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw","createdAt":"2026-10-09T00:00:00Z"}
```

### A.2 Host descriptor

```
{"v":2,"type":"server-descriptor","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"version":1,"roles":["host","relay"],"endpoints":{"graphql":"https://host.example/api","websocket":"wss://host.example/api/ws","s2s":"https://host.example/s2s/v1"},"s2sVersions":[1],"mlsCipherSuites":[79],"limits":{"maxBodyBytes":8388608,"clockSkewSeconds":300,"maxDeliverItems":256},"registration":{"mode":"invite","powBits":null},"issuedAt":"2026-10-09T00:00:00Z"}
ed25519:ZdBDMoUEFhLQMnX7dK_r8xUPa0dNoSHOZqfAHelFegW0PUpluRUX84gzBzp2TtMKku19aCrf4wMXjbemA8b_Bw
```

### A.3 Delegation

Device seed 20, to the relay of A.1, for the host of A.1; nonce bytes `70`..`7f`.

```
{"v":2,"type":"delegation","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","relay":"rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c","relayKeyHash":"8UGftl4tBoxZjUo1UP_-zwUlobQJPwj5kZ0NC1oaAaM","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","workspaceId":"0199d1a0-0000-7000-8000-000000000002","scope":"workspace","nonce":"cHFyc3R1dnd4eXp7fH1-fw","issuedAt":"2026-10-09T00:00:00Z","expiresAt":"2026-10-10T00:00:00Z"}
ed25519:aT8j0zeSdSqWySreIIeRokrjVoYuO-3z1k4IcsFN898M2x_in0B9UlaL5hCUAdw-gCisynSQi3b51QQiZdHlCQ
```

### A.4 Signed S2S request and response

The relay presents A.3: `POST https://host.example/s2s/v1/subscriptions`,
`Rim-Origin: https://relay.example`, `created` 1791504000 (2026-10-09T00:00:00Z), nonce bytes
`60`..`6f`, body (685 bytes) `{"delegations":[{"payload":<A.3 payload as a JSON string>,"signature":<A.3 signature>}]}`.

```
"@method": POST
"@target-uri": https://host.example/s2s/v1/subscriptions
"content-digest": sha-256=:NLGvVDM7TsvDtemcqNyHnp4tVM9bTzAjg1MbseDLrOc=:
"content-type": application/json
"rim-origin": https://relay.example
"@signature-params": ("@method" "@target-uri" "content-digest" "content-type" "rim-origin");created=1791504000;expires=1791504300;nonce="YGFiY2RlZmdoaWprbG1ubw";keyid="rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c#0";alg="ed25519";tag="rim-s2s-v1"
```

That is the signature base (lines joined by `\n`, no trailing newline). `Signature-Input` is `sig1=`
followed by the `@signature-params` value, and:

```
Signature: sig1=:N1zyrK5mP9qxsSJOKAriCI38lQHL2H8FiKprKMqOay1+XFgIaldQw7J5zILaG6/hWcw6ESUC4HGY/bJ1O+QMDg==:
```

The host's response: status 200, `created` 1791504001, `Rim-Request-Nonce: YGFiY2RlZmdoaWprbG1ubw`,
body `{"subscriptions":[{"workspaceId":"0199d1a0-0000-7000-8000-000000000002","status":"active","expiresAt":"2026-10-10T00:00:00Z"}]}`;
its signature base, then its `Signature` field:

```
"@status": 200
"content-digest": sha-256=:BBeCXY4Vnpnqik7XJ8TNUUC/Y392lt0Zhhcz9uwijB8=:
"content-type": application/json
"rim-request-nonce": YGFiY2RlZmdoaWprbG1ubw
"@method";req: POST
"@target-uri";req: https://host.example/s2s/v1/subscriptions
"content-digest";req: sha-256=:NLGvVDM7TsvDtemcqNyHnp4tVM9bTzAjg1MbseDLrOc=:
"@signature-params": ("@status" "content-digest" "content-type" "rim-request-nonce" "@method";req "@target-uri";req "content-digest";req);created=1791504001;keyid="rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw#0";alg="ed25519";tag="rim-s2s-v1"

Signature: sig1=:rGIqqwBn079G9OciB/u4fSIro9i6S0P97QNxFSo8UxPvXJwywOS4z5NrmPIYNnr2MKxK2dSzsBl0eU97wQFGAw==:
```

### A.5 Host event

```
{"v":2,"type":"host-event","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"workspaceId":"0199d1a0-0000-7000-8000-000000000002","stream":"c:0199d1a0-0000-7000-8000-000000000003","seq":42,"event":{"type":"MessageDeleted","workspace_id":"0199d1a0-0000-7000-8000-000000000002","conversation_id":"0199d1a0-0000-7000-8000-000000000003","conversation_type":"Channel","message_id":"0199d1a0-0000-7000-8000-000000000004","thread_id":null},"issuedAt":"2026-10-09T00:00:00Z"}
ed25519:CPSwWGrkjbauuToACdrAuJL4L6mv1XpGTPy_XKjkymGwzqx6pyW-j6V52HkhpPlXx82tXRbQTI-GsH9N4N-zBg
```

### A.6 Device-signed write and host-signed response

GraphQL body (117 bytes):
`{"query":"mutation($id:ID!){deleteMessage(messageId:$id)}","variables":{"id":"0199d1a0-0000-7000-8000-000000000004"}}`

```
{"v":2,"type":"device-request","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","method":"POST","path":"/api","seq":1791504000000000,"bodySha256":"a2PtmXdQU8dMqDduw8cQymbwEmFEilShfZfDDArTcwU","issuedAt":"2026-10-09T00:00:00Z"}
ed25519:IoWInJdss89VdGgCEnWvLRCAHZ--5mrIzRiDV0thfCwLOVOgyEZzaGG7CGBfgKqnnJiJvzHMxaCZYFiEMiSbCg
```

`Rim-Device-Request` (base64url of that payload):
`eyJ2IjoyLCJ0eXBlIjoiZGV2aWNlLXJlcXVlc3QiLCJwcmluY2lwYWxJZCI6InJpbXAxX3R4RExibmlwaTVpM2tDSnRKd0JPVWl4enZfTVk4T25VMG1jeS0tVHlqZTAiLCJkZXZpY2VJZCI6IjAwMDAwMDAwLTAwMDAtMDAwMC0wMDAwLTAwMDAwMDAwMDAwMSIsImhvc3QiOiJyaW1zMV9PRlBETlhUVjFzcGxDY0tjZ3oxdldZajZNZy1sYlVvbGxIUXdYVGtZRUJ3IiwibWV0aG9kIjoiUE9TVCIsInBhdGgiOiIvYXBpIiwic2VxIjoxNzkxNTA0MDAwMDAwMDAwLCJib2R5U2hhMjU2IjoiYTJQdG1YZFFVOGRNcURkdXc4Y1F5bWJ3RW1GRWlsU2hmWmZEREFyVGN3VSIsImlzc3VlZEF0IjoiMjAyNi0xMC0wOVQwMDowMDowMFoifQ`

The host's answer to body `{"data":{"deleteMessage":true}}`:

```
{"v":2,"type":"host-response","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"requestSha256":"USMHYKhT_TDTgBiA_mrWvOz3-RZLlohDKUQREkVxMCY","status":200,"bodySha256":"6lPwX_33fnLq0QTkKknG-F-tK81icmIjQKnN0eS8JA0"}
ed25519:f-vulCqj3GgD3dI4zNAeCMs1JGyDwkzbkcLhk0uXXMz4NCJ-az1ZVKWxgpbqNMWTzMhSKtnFPhwMHmATk69zDg
```

### A.7 Revocation and acknowledgement

The device revokes itself (`"signer":"device"`); the host acknowledges.

```
{"v":2,"type":"device-revoke","principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","signer":"device","issuedAt":"2026-10-09T00:00:00Z"}
ed25519:kCcxKVwjdFvYSC3xx3yHWgO5gWwwq9wpC8bzQs1NPn9bKhAUJSBdLawiI1bWkNDDOdeW_x7AEm0fafPf38D3Bw
{"v":2,"type":"revocation-ack","host":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":0,"principalId":"rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0","deviceId":"00000000-0000-0000-0000-000000000001","revocationSha256":"VYCvacXofNMIe3q3-owfpopw8oLdXAOc6KvNKsEtis4","appliedAt":"2026-10-09T00:00:00Z"}
ed25519:waKbDNMuQ460M8gX_H1NKzAMDXF3x4P8hbHoJFPr-pSduvfvQSbXvOcRmK5VcF5F9-sICQmF-pBnACBu9Z-mDg
```

### A.8 Proof-of-work

At the relay of A.1, for `key-identity-v1.md` appendix B's inception payload, with test difficulty
`powBits` 16: the smallest working counter is 37283, nonce `AAAAAAAAkaM` (8 bytes big-endian), hash
`00002f63d137749fda27201ccf9d282a4ed846287d49c55fea89a47617e78ed6`.

### Filled by S5-1

**RFC 9421 appendix B.2.6** through `rim_s2s` (`rim_s2s/src/vectors.rs`): the RFC's
`test-key-ed25519` (seed: the last 32 bytes of its PKCS#8 key), the components `"date" "@method"
"@path" "@authority" "content-type" "content-length"` with `created=1618884473` and
`keyid="test-key-ed25519"` serialize to the RFC's `Signature-Input`, the crate builds the RFC's
signature base, and signing it gives the RFC's
`sig-b26=:wqcAqbmYJ2ji2glfAMaRy4gruYYnx2nEFN2HN6jrnDnQCK1u02Gb04v9EDgwUPiu4A0w6vuQv5lIp5WPpBKRCw==:`.

**Rotation.** The host of A.1 rotates from seed 40 to seed 41 (`signer: server`, recovery key
unchanged, `createdAt` 2026-10-10T00:00:00Z), signed by the seed 40 key:

```
{"v":2,"type":"server-rotate","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","seq":1,"prev":"oUvCbfqOS1bUTL5gAtene_faqghv8PaPgrRDIJpXLOM","serverKey":"ed25519:-kg0FH9uaQw2k-_2EzYEZAPNiuKhTzGzxAc1hWkjlWU","recoveryKey":"ed25519:_L44YyQXu1uHXUmjwCJwYzkXxgk_0B67Wf9kFuN6_gw","signer":"server","createdAt":"2026-10-10T00:00:00Z"}
ed25519:cysmxfZAsbKJKvFGkftqHCqAzySvf4MMD9PTimeNavN_Oi91Zt-lfcvCswkb08SnVDrS-iMWK1WT1n7iY_fZAA
```

| Item | Value |
|---|---|
| Key hash of the seed 41 key | `fvKh1r_va0lqRvlpGojY2V1JzJMQeSESlNpO4OXw6Ak` |
| `prev` for seq 2 | `sU02HLfXJNvkpkHn28DO3JdBxQpmTr3Dl4y94zIY9hU` |

The descriptor of A.2 re-signed by the seed 41 key at `keySeq` 1, `version` 2, `issuedAt`
2026-10-10T00:00:00Z (the A.2 descriptor no longer verifies against this log):

```
{"v":2,"type":"server-descriptor","serverId":"rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw","keySeq":1,"version":2,"roles":["host","relay"],"endpoints":{"graphql":"https://host.example/api","websocket":"wss://host.example/api/ws","s2s":"https://host.example/s2s/v1"},"s2sVersions":[1],"mlsCipherSuites":[79],"limits":{"maxBodyBytes":8388608,"clockSkewSeconds":300,"maxDeliverItems":256},"registration":{"mode":"invite","powBits":null},"issuedAt":"2026-10-10T00:00:00Z"}
ed25519:Yuy9VdOS78e8fL4sMDT9PHmBkZcuzeJEvLlfdD8dRhDjJeJiivU4FcatDrlwEKaB3l9Beado7Qi6_33D7UN0AA
```

Both computed by `rim_s2s` and cross-checked with Python `cryptography` 50.0.2. Every value of
A.1 to A.8 reproduces byte for byte in `rim_s2s`'s tests.

**Without `RIM_PUBLIC_BASE_URL`** the descriptor's `graphql` and `websocket` endpoints use
`http://localhost:<RIM_SERVER_PORT>`; `s2s` is null.
