# The relay

A relay is the server a person's app talks to. It keeps their account, devices and profile card,
queues what other servers send them, files conversations from strangers in a request inbox, and
forwards their requests to the servers that host the spaces they use. Every server is the relay of
its local users unless it runs with `RIM_ROLE=host`. This page explains what a relay and a host
do, for operators and for client authors. The contract is `docs/specs/server-to-server-v1.md`;
the server's key, id and transport settings are in `docs/server-identity.md`; deploying a public
relay is `deploy/README.md` and running one is `docs/operations/relay-public.md`.

Nothing here runs until server-to-server traffic is on (`RIM_PUBLIC_BASE_URL` is an `https` URL).
Before that, a server is the relay and host of its own users only, as it always was.

## For operators

### Configuration

| Variable | Default | Meaning |
|---|---|---|
| `RIM_ROLE` | `both` | `relay` creates no new workspaces; `host` refuses `/api/remote/…`, contacts, DM requests and deliveries |
| `RIM_RELAY_QUEUE_BYTES_PER_USER` | 268435456 | Bytes queued per local user; over it the oldest items are dropped |
| `RIM_RELAY_RETENTION_DAYS` | 30 | How long queued items and DM requests are kept |
| `RIM_S2S_DEVICE_RATE_PER_MIN` | 120 | Forwarded requests per device per minute: those a local user's device sends through `/api/remote/…`, and those a host accepts |
| `RIM_S2S_SUBSCRIPTIONS_PER_PEER` | 10000 | Active delegations one relay may hold on this host; past it new delegations are refused as `limited`, while renewals of active ones still go through |
| `RIM_HOST_ALLOWED_RELAYS` | empty = any | Server ids of the relays allowed to join, subscribe, forward, report presence and bring revocations here |
| `RIM_RELAY_FORWARDS_PER_USER_PER_DAY` | 50000 | Requests a local user sends through `/api/remote/…` per day |
| `RIM_RELAY_DM_REQUESTS_PER_DAY` | 20 | DM requests one sender principal files here per day, and invites a local user sends per day to people on other servers who are not their contacts |
| `RIM_RELAY_CARD_MAX_BYTES` | 131072 | Largest profile card a local user publishes |
| `RIM_RELAY_KEY_PACKAGES_PER_DEVICE` | 100 | Unclaimed one-time key packages per device and cipher suite |
| `RIM_RELAY_DEVICES_PER_USER` | 32 | Devices a local user keeps, revoked ones not counted |
| `RIM_REGISTRATION_POW_BITS` | unset | Turns on proof-of-work signups, "The public relay" |

### The direct space

Each server has one `server` row of kind `direct`, named "Direct messages", created at start. It
holds the DMs and groups its users start with people on other servers, and is never listed by
`servers` nor found by name. Everyone in it is a guest: nobody sees its member list, and nobody
creates channels, invites or plugins there. Local users join it on first use: a signed-in
`relayInfo`, or any read or write of the direct space that needs membership; people from
other servers join when a local user adds them. Custom emojis, plugins, workspace keybinds,
pipelines, workspace state and channels answer `VALIDATION` ("not available in the direct
space") there, reads and writes alike, and the refused call joins nobody.

A conversation's home is the relay whose direct space holds it. The home sequences it: it stores
the messages, Commits and Welcomes, and every member's app reads and writes there, through their
own relay when it is elsewhere.

### People from other servers

When a local user adds someone from another server, this server fetches their identity (key log,
devices with certificates and revocations, profile card) from their relay, checks every signature,
and stores them as a `user` row of kind `remote` named `r_` and the first 30 characters of their
principal id. Their relay pushes identity changes (`POST /s2s/v1/identity`) to the servers that
queued items or filed requests for them; `principalDevices` also refetches a remote person's
identity when the stored copy is over an hour old. If such a person later signs up here with
their key, the remote row becomes their local account and keeps its memberships.

Remote users never sign in here and never run account mutations here: sign-in, sign-up, device
and key changes, card publishing, profile, workspace creation, device links, instance invites
and the relay's own mutations are refused with `FORBIDDEN` ("not available to remote
members").

### Queues and the outbox

Every event of a space with remote members becomes a signed host event (`host-event`) on a stream:
`c:<conversationId>` for a conversation's events, `u:<principalId>` for one person's (their
notifications, mentions, read states, Welcomes, DM invites, and for a guest the directory events
about them), `w` for other space-wide events, `e` for typing and presence. Each stream's `seq`
counts from 1 without gaps; `e` has none. The host appends one outbox row per relay that should
get the event (for a workspace, the relays holding an active delegation of the person; for the
direct space, the relays the person's newest card lists, else the relay that served their
identity) and a sender delivers each relay's rows in order (`POST /s2s/v1/deliver`), backing off
from 1 second to 5 minutes. Outbox rows are kept 7 days. A relay that refuses a person as unknown
is not sent their events for 24 hours. Each server process keeps that refusal in memory: a restart
forgets it, and with several nodes each node learns it from the relay on its own.

A relay stores each envelope once and one item per recipient. Each device reads its user's items
above its cursor; a device added later starts at the newest item queued before it. An item is
deleted once every non-revoked device of its user has acknowledged it, and after the retention
period regardless. Over the per-user byte cap the oldest items go first and the devices online get
a `QueueOverflow` notice; a device offline at the time sees the gap in the stream's `seq`. A relay
pulls what a host still holds for it (`GET /s2s/v1/outbox`) every minute, so a delivery missed
while the relay was down arrives on the next pull.

### The request inbox

A relay files a conversation as a DM request when its invite (`DmInvite`) comes from someone who
is not the recipient's contact, was never accepted by them and shares no workspace this server
hosts with them. The request keeps the sender's card
and holds up to three items of the conversation; later ones are dropped until it is accepted.
Conversations from contacts are recorded as accepted at once. Items of a conversation the relay
has no invite for are refused (`not_subscribed`), so a host cannot skip the inbox.

The same rule covers conversations of this server's own direct space whose creator is someone
else: a person from another relay creating a DM with a local user through the forward path, or a
local stranger. The server files its own invite for the local member; until they accept, the
conversation is left out of their `inboxConversations`, its events out of their
`workspaceEvents`, and its notifications, mentions and unread counts out of the notification
list, the bell, `unreadCounts` and the email digest; its first events are held with
the request. Everything appears once they accept. This needs
server-to-server traffic to be on; with it off, conversations in the direct space are live at
once.

| Action | Effect |
|---|---|
| `acceptDmRequest` | Held items released; later items flow; the sender becomes a contact |
| `ignoreDmRequest` | Held items deleted but the invite, which stays with the request; later items dropped; the request expires after 30 days |
| `blockPrincipal` | Pending requests from the principal ignored as above; its invites (`blocked`) and key package claims refused |
| `reportDmRequest` | Filed in `relay_abuse_report` for the operator and audited as `relay.dm_request_reported`; a pending request is ignored |
| Nothing | Expires after the retention period |

Limits: 100 pending requests per recipient and `RIM_RELAY_DM_REQUESTS_PER_DAY` (20) per sender
principal per day on this relay; past them invites are refused as `limited`. A claim of a local user's key packages by a stranger
(not a contact, never accepted, sharing no workspace here) gets last-resort packages only, at
most 20 claims per stranger per day.

### Forwarded requests

A local user's app reaches a space hosted elsewhere at `/api/remote/<hostServerId>/<path>` with
its session cookie (and `X-CSRF-Token` for `POST`), a device-signed request statement in
`Rim-Device-Request` (unpadded base64url of the payload) and its signature in
`Rim-Device-Signature`. The relay checks that the statement names the session's principal and
device, that the device is not revoked and that the signature verifies, then forwards the body
unchanged to `<host>/s2s/v1/forward/<path>` and returns the host's answer unchanged. A host it
cannot reach is `502 {"error":"upstream_unavailable","hostServerId":…}`.

The host checks the S2S signature, the device request (a device of a user it holds, its own id,
the method, path and body hash, the `seq` window), and that the relay may carry the user's
traffic: the relay holds an active delegation of that device for a workspace the user is still a
member of, or the user is a member of its direct space and their newest card lists the relay's
origin (`403 not_delegated` otherwise). It then runs GraphQL (`POST /api`), attachment downloads and
uploads, avatar uploads (`POST /api/files/avatar`), stored-file downloads (`GET /api/files/<path>`)
or plugin bundle downloads as that user and device, with no session, and signs every
answer, refusals included, with `Rim-Host-Response` and `Rim-Host-Signature`. Per-IP limits key
on `relay:<serverId>:<principalId>`.

A device request's `seq` is accepted once: the host remembers accepted values for at least 10
minutes and until they fall below its window (5 minutes in microseconds under the device's
highest), and refuses a repeat or a lower value with `409 {"error":"replay","highestSeq":…}`.

### Workspaces with members from other servers

A person whose relay is another server joins a workspace here through that relay. Their app gets
a login challenge from this server (`POST /s2s/v1/join/challenge`, 120 seconds, single use),
with the workspace its invite names, and
signs it with this server's id as the audience, so the proof is good here only. It also signs a
**delegation**: its relay may receive this workspace's events for the person and forward that
device's signed requests here, for at most 24 hours. The relay presents both with the person's key
log, devices and card (`POST /s2s/v1/join`). This server checks everything, joins the person by
the invite exactly as `joinServerByInvite` does (bans included; a current member sends no invite),
stores them as a remote user and keeps the delegation.

A delegation is accepted when its device is a live device of the person, it names this server
and the presenting relay, its relay key hash names a key of the relay's log that no recovery
rotation replaced, the person is a current, unbanned member, and it runs at most 24 hours. It is
kept until its expiry or a day after acceptance, whichever is first, one per workspace, device and
relay. Relays renew them (`POST /s2s/v1/subscriptions`, at most 64 at a time, each accepted or
refused with a reason) and cancel them (`POST /s2s/v1/subscriptions/cancel`). A workspace's events
go to the relays holding an active delegation of a recipient; those relays may forward the
device's requests. A kick or a ban ends the person's delegations once their `MemberLeft` is queued;
a revoked device's delegations end with it.

Relays report whether their users are online (`POST /s2s/v1/presence`); this server shows a remote
member online in every workspace their relay holds a delegation for, until the relay reports them
offline.

A revocation of a remote member's device arrives from their relay (`POST /s2s/v1/revocations`) or
straight from their app (`applyRemoteRevocation`). This server checks it against its copy of the
person's key log, marks the device revoked, ends its delegations and sessions, removes it from
encrypted groups, and answers a signed acknowledgement; a repeat gets the same one.

When their relay is down, a remote member's desktop app may sign in here directly with its device
key (`loginWithKey`, audience this server's id). This server first tries to refresh the person's
identity from their relays, waiting up to 10 seconds, and issues a session of 24 hours. Account
mutations stay refused on it.

### Cleanup and audit

Every minute the server deletes acknowledged and expired items, envelopes no item uses, expired
requests, outbox rows over 7 days and accepted `seq` values out of the window, ends expired
delegations and those of revoked devices or departed members, and marks this relay's expired
subscriptions lapsed. Every 5 seconds it sends revocations due for another attempt. Audit events
(target `audit`, ids only): `relay.principal_blocked` (`scope` `user` for a user's block,
`server` for an operator's), `relay.principal_unblocked`, `relay.blocked_principal_refused`,
`relay.dm_request_reported`, `relay.abuse_report_filed`, `relay.abuse_report_resolved`,
`relay.registration_pow`, `relay.left`, `key_identity_adopted` and `host.revocation_applied`; `host.remote_join` and
`host.delegation_accepted` are also written to `command_audit_log` (initiator `s2s`).

### The public relay

A relay anyone may sign up on runs `RIM_ROLE=relay` with the settings of
`deploy/compose.relay.yml` on top of `deploy/compose.yml`. This section says how its
registration and blocks behave; `docs/operations/relay-public.md` has its quotas, the operator's
commands and the abuse policy.

**Registration.** `RIM_REGISTRATION_POW_BITS` (1 to 40; 24 on the public relay) turns on `pow`
registration, whatever `RIM_OPEN_REGISTRATION` says. `signupWithKey` then takes a registration
token (instance invite or workspace invite) or `proofOfWork`: an 8-byte nonce,
unpadded base64url, such that

```
SHA-256("rim.pow.v1\0" || this server's id || "\0" || inception payload || nonce)
```

starts with that many zero bits. The work is bound to this server and one principal, and
checking it is one hash. Proof-of-work signups also count against the open-signup limit per IP
and are audited as `relay.registration_pow`. Password signups take a token, as in invite-only
mode. The descriptor says `"registration":{"mode":"pow","powBits":24}` and `registrationMode`
answers `POW`. A signup link from `rim_server invite create` lets a person in without the work;
it needs no account or workspace, so it works with `RIM_ROLE=relay`.

**Quotas** per local user are the `RIM_RELAY_*` settings above. The per-minute and per-day
counters are in memory, per node.

**Blocking a principal.** `rim_server principal block <principalId> --reason <text>` stores the
block in `relay_principal_block`, ends the sessions of the account holding the principal here
and ignores its pending DM requests for every recipient. From then on, until `principal
unblock`: its logins, signups and enrollments are refused (`FORBIDDEN`); deliveries to it are
refused as `unknown_principal` and its invites as `blocked`; its key packages are not handed
out, and it claims none; `GET /s2s/v1/principals/<id>` answers `404`; forwarded requests it
signs are refused with `403`. `principal list` shows the blocks.

**Blocking a peer.** `rim_server peer block <serverId>` sets the pinned peer's status to
`blocked`, which the request check reads on every request: its requests get
`403 peer_not_allowed` and nothing is sent to it. `peer unblock` makes it `active` again. A
forked peer stays forked; a peer never pinned is refused with `RIM_S2S_DENIED_PEERS` instead.
Audited as `s2s.peer_blocked` and `s2s.peer_unblocked`.

**Abuse reports.** `reportDmRequest` in the app and reports that come by mail (`rim_server
abuse-reports add`) both end in `relay_abuse_report`, which `abuse-reports list` and
`abuse-reports resolve` work through.

## For clients

- `relayInfo { serverId roles directSpaceId }` (no session needed) names the direct space.
- `remoteServer(origin)` pins another server through the relay and returns its discovery
  document to verify and pin in the app.
- `remoteIdentity(principalId, relayUrl)` returns a principal's key log, devices and card, from
  this server or fetched from `relayUrl` and cached for 24 hours. Verify the log against the
  principal id, each certificate against the log and the card against the head.
- `contacts`, `addContact(principalId, relayUrls)`, `removeContact(principalId)`: the contact list
  the request inbox filters by. Relay URLs are 1 to 8 of 1 to 256 bytes of printable ASCII
  without `?`, `#` or `@`.
- `createConversation(input: { serverId: <directSpaceId>, remoteMembers: [{ principalId,
  relayUrls }], encrypted })` starts a DM (one remote member, no `otherUserId` or `memberIds`) or a
  group with people on other servers; people on this relay may be named the same way. A
  conversation with someone on another relay must be `encrypted` (otherwise `VALIDATION`,
  "conversations with people on other servers are encrypted"), and
  every member needs an MLS-active device. Any
  `memberIds`, or more than one remote member, makes a group. The caller needs a published profile
  card: their relays show it in the request inbox. `predecessor: { hostServerId, conversationId }`
  records the conversation a successor replaces, and `Conversation.predecessor` returns it.
- On a relay in `pow` registration mode (`registrationMode` is `POW`; the difficulty is the
  discovery document's `registration.powBits`), `signupWithKey` takes `proofOfWork`: count from
  0, hash as in "The public relay" with the counter as the 8-byte big-endian nonce, and send the
  first nonce that meets the difficulty, unpadded base64url. A registration token instead skips
  the work.
- `dmRequests(status)`, `acceptDmRequest`, `ignoreDmRequest`, `blockPrincipal`,
  `unblockPrincipal`, `reportDmRequest` work the request inbox. A request's `heldItems` are
  host-signed envelopes to read before deciding, the `DmInvite` first; verify it to bind the
  conversation's space to its home before reading or leaving it.
- `relayStream(after)` on the existing WebSocket yields the session device's items above `after`
  (its cursor when omitted), then live ones, as JSON strings:
  `{"type":"RelayItem","id","hostServerId","workspaceId","envelope":{"payload","signature"}}`, plus
  `{"type":"DmRequestReceived","requestId"}` and
  `{"type":"QueueOverflow","spaces":[{"hostServerId","workspaceId"}]}`. `relayItems(after, limit)`
  pulls the same items; `ackRelayItems(upTo)` moves the device's cursor. The session must be bound
  to a device.
- For every item, check the envelope's signature under the host key pinned for
  `hostServerId` at its `keySeq`, that `host` is that server, and that the stream matches the event
  (`c:` its conversation, `u:` your principal). Apply a stream's events in `seq` order; on a gap,
  reconcile through the host (its change feed and `streamHeads(serverId, streams)` through the
  forward path).
- `leaveRelay` cancels the caller's subscriptions at every host, then deletes their queue,
  requests, contacts and blocks on this relay.

### Joining a workspace on another host

1. Read the invite link `rim-invite:1:<hostServerId>:<hostKeyHash>:<inviteToken>:<hostOrigin>`
   and pin the host with `remoteServer(origin)`.
2. `remoteJoinChallenge(hostServerId, hostOrigin, inviteToken)` returns
   `{ challengeId nonce expiresAt workspaceId }`: `workspaceId` is the workspace the invite names
   (the invite is looked up, not used), null without one. Sign a `login` statement over the
   challenge with `audience` = the host's server id.
3. Sign a delegation with the device key over `rim.delegation.v1\0 || payload`:

   ```json
   {"v":2,"type":"delegation","principalId":"rimp1_…","deviceId":"<uuid>","relay":"<relay server id>",
    "relayKeyHash":"<hash of the relay's current key>","host":"<host server id>","workspaceId":"<uuid>",
    "scope":"workspace","nonce":"<16 random bytes, base64url>","issuedAt":"…","expiresAt":"…"}
   ```

   `expiresAt` is at most 24 hours after `issuedAt`. `relayInfo` names the relay; its key hash is
   in the relay's discovery document.
4. `joinRemoteWorkspace(input: { hostServerId, hostOrigin, inviteToken, proof, delegation })`
   returns `{ hostServerId workspaceId userId role }`; `inviteToken` is null for a current member.
   The workspace is then used through `/api/remote/<hostServerId>/api`, and its events arrive on
   `relayStream`.

### Keeping delegations fresh

- `remoteSubscriptions` lists `{ hostServerId workspaceId status expiresAt }` with `status`
  `active`, `lapsed` (every delegation expired) or `ended` (cancelled).
- When the current delegations have less than 6 hours left, an online device signs fresh ones and
  sends them with `renewDelegations(delegations)`. Each entry of the answer is `active` with its
  `expiresAt`, or `refused` with a `reason`: one of the host's (`not_member`, `expired`, …), or
  `host_unreachable` (try again later; the subscription is unchanged) or `host_refused` (the host
  turned the presentation down). The relay logs what lies behind the last two.
- While a device has `relayStream` open, the hosts see the person online; 30 seconds after the last
  stream closes they see them offline.

### Revoking a device

`revokeDeviceKey` on the relay also sends the revocation to every host and DM home holding the
person, retrying at 10 s, 30 s, 1 min, 2 min, 5 min and then every 15 minutes for a day.
`deviceRevocationStatus(deviceId)` lists `{ hostServerId status attempts ack }` per host, `status`
`PENDING`, `ACKED` or `GAVE_UP`; `relayStream` pushes
`{"type":"RevocationAcked","deviceId","hostServerId","ack":{"payload","signature"}}` as each host
acknowledges. Check each `ack` (a `revocation-ack` statement) under the host's pinned key and its
`revocationSha256` against the revocation. A host that has not acknowledged within 2 minutes gets
the revocation from the app directly: `applyRemoteRevocation(revocation)` on the host's own `/api`
needs no session and no CSRF header and returns the acknowledgement; an address may try it 20
times in 15 minutes.

### Signing in to a host directly

When the relay is unreachable, the desktop app calls `keyLoginChallenge` and `loginWithKey` on the
host's own `/api` with `audience` = the host's server id. The session lasts 24 hours; account
mutations are refused on it (`FORBIDDEN`), as through the relay.
