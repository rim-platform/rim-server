# The relay

A relay is the server a person's app talks to. It keeps their account, devices and profile card,
queues what other servers send them, files conversations from strangers in a request inbox, and
forwards their requests to the servers that host the spaces they use. Every server is the relay of
its local users unless it runs with `RIM_ROLE=host`. The contract is
`docs/specs/server-to-server-v1.md`; server identity and the signed transport are in
`docs/server-identity.md`.

Nothing here runs until server-to-server traffic is on (`RIM_PUBLIC_BASE_URL` is an `https` URL).
Before that, a server is the relay and host of its own users only, as it always was.

## For operators

### Configuration

| Variable | Default | Meaning |
|---|---|---|
| `RIM_ROLE` | `both` | `relay` creates no new workspaces; `host` refuses `/api/remote/…`, contacts, DM requests and deliveries |
| `RIM_RELAY_QUEUE_BYTES_PER_USER` | 268435456 | Bytes queued per local user; over it the oldest items are dropped |
| `RIM_RELAY_RETENTION_DAYS` | 30 | How long queued items and DM requests are kept |
| `RIM_S2S_DEVICE_RATE_PER_MIN` | 120 | Forwarded requests a host accepts per device per minute |

### The direct space

Each server has one `server` row of kind `direct`, named "Direct messages", created at start. It
holds the DMs and groups its users start with people on other servers, and is never listed by
`servers` nor found by name. Everyone in it is a guest: nobody sees its member list, and nobody
creates channels, invites or plugins there. Local users join it the first time they start a
conversation there; people from other servers join when a local user adds them.

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
and key changes, card publishing, profile, workspace creation, ingest tokens, device links, instance
invites and the relay's own mutations are refused with `FORBIDDEN` ("not available to remote
members").

### Queues and the outbox

Every event of a space with remote members becomes a signed host event (`host-event`) on a stream:
`c:<conversationId>` for a conversation's events, `u:<principalId>` for one person's (their
notifications, mentions, read states, Welcomes, DM invites, and for a guest the directory events
about them), `w` for other space-wide events, `e` for typing and presence. Each stream's `seq`
counts from 1 without gaps; `e` has none. The host appends one outbox row per relay that should
get the event (the relays the person's newest card lists, else the relay that served their
identity) and a sender delivers each relay's rows in order (`POST /s2s/v1/deliver`), backing off
from 1 second to 5 minutes. Outbox rows are kept 7 days. A relay that refuses a person as unknown
is not sent their events for 24 hours.

A relay stores each envelope once and one item per recipient. Each device reads its user's items
above its cursor; a device added later starts at the newest item queued before it. An item is
deleted once every non-revoked device of its user has acknowledged it, and after the retention
period regardless. Over the per-user byte cap the oldest items go first and the devices online get
a `QueueOverflow` notice; a device offline at the time sees the gap in the stream's `seq`. A relay
pulls what a host still holds for it (`GET /s2s/v1/outbox`) every minute, so a delivery missed
while the relay was down arrives on the next pull.

### The request inbox

A relay files a conversation as a DM request when its invite (`DmInvite`) comes from someone who
is not the recipient's contact and was never accepted by them. The request keeps the sender's card
and holds up to three items of the conversation; later ones are dropped until it is accepted.
Conversations from contacts are recorded as accepted at once. Items of a conversation the relay
has no invite for are refused (`not_subscribed`), so a host cannot skip the inbox.

| Action | Effect |
|---|---|
| `acceptDmRequest` | Held items released; later items flow; the sender becomes a contact |
| `ignoreDmRequest` | Held items deleted but the invite, which stays with the request; later items dropped; the request expires after 30 days |
| `blockPrincipal` | Pending requests from the principal ignored as above; its invites (`blocked`) and key package claims refused |
| `reportDmRequest` | Audited as `relay.dm_request_reported`; a pending request is ignored |
| Nothing | Expires after the retention period |

Limits: 100 pending requests per recipient and 20 per sender principal per day on this relay;
past them invites are refused as `limited`. A claim of a local user's key packages by a stranger
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
traffic: the user is a member of its direct space and their newest card lists the relay's origin
(`403 not_delegated` otherwise). It then runs GraphQL (`POST /api`), attachment downloads and
uploads, or plugin bundle downloads as that user and device, with no session, and signs every
answer, refusals included, with `Rim-Host-Response` and `Rim-Host-Signature`. Per-IP limits key
on `relay:<serverId>:<principalId>`.

A device request's `seq` is accepted once: the host remembers accepted values for at least 10
minutes and until they fall below its window (5 minutes in microseconds under the device's
highest), and refuses a repeat or a lower value with `409 {"error":"replay","highestSeq":…}`.

### Cleanup and audit

Every minute the server deletes acknowledged and expired items, envelopes no item uses, expired
requests, outbox rows over 7 days and accepted `seq` values out of the window. Audit events (target
`audit`, ids only): `relay.principal_blocked`, `relay.dm_request_reported`, `relay.left` and
`key_identity_adopted`.

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
  group with people on other servers; people on this relay may be named the same way. Any
  `memberIds`, or more than one remote member, makes a group. The caller needs a published profile
  card: their relays show it in the request inbox. `predecessor: { hostServerId, conversationId }`
  records the conversation a successor replaces, and `Conversation.predecessor` returns it.
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
- `leaveRelay` deletes the caller's queue, requests, contacts and blocks on this relay.
