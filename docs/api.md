# Talking to a server over HTTP

The HTTP contract a client sees: the routes, how a session and its CSRF token work, which
origins a browser may call from, the error codes, pagination and how a client tells which
server version it talks to. The GraphQL types and fields themselves are in the schema
(`GET /api/sdl`, see "The full GraphQL reference"); the server-to-server transport is in
`specs/server-to-server-v1.md`.

## Endpoints

Everything except `/metrics` is served on `RIM_SERVER_HOST:RIM_SERVER_PORT`. "Session" means
the route needs the session cookie; "CSRF" means it also needs the `X-CSRF-Token` header (see
"Sessions and CSRF").

| Method | Path | Needs | What it does |
|---|---|---|---|
| `POST` | `/api` | Session for most fields; CSRF on mutations | GraphQL. The body is one GraphQL request or a JSON array of them (a batch). |
| `GET` | `/api` | Nothing | The GraphiQL explorer, only when `RIM_ENABLE_GRAPHIQL=true`. |
| `GET` | `/api/ws` | Session | GraphQL subscriptions over a WebSocket. |
| `GET` | `/api/sdl` | Nothing | The schema in SDL, as `text/plain`. |
| `GET` | `/.well-known/rim-server` | Nothing | The signed discovery document. |
| `GET` | `/healthz` | Nothing | Liveness. |
| `GET` | `/readyz` | Nothing | Readiness. |
| `GET` | `/metrics` | Nothing | Prometheus metrics, on a separate listener. |
| `POST` | `/api/files/upload` | Session, CSRF | Upload the bytes of an attachment. |
| `POST` | `/api/files/avatar` | Session, CSRF | Upload the caller's avatar. |
| `GET` | `/api/files/{path}` | Session | Download a stored avatar. |
| `GET` | `/api/attachments/{id}/download` | Session | Download an attachment. |
| `POST` | `/api/plugin-bundles/{server_id}` | Session, CSRF | Upload a web plugin bundle to a workspace. |
| `GET` | `/api/plugin-bundles/{server_id}/{sha256}` | Session | Download a web plugin bundle. |
| `GET` | `/api/reports/{report_id}/attachments/{blob_sha256}` | Session | A reported encrypted attachment, for moderators. |
| `GET`, `POST` | `/api/remote/{host_server_id}/{tail}` | Session; CSRF on `POST` | A request to a space hosted on another server, through this relay. |
| any | `/s2s/v1/...` | A server signature | The server-to-server API; not for clients. |

### GraphQL: `POST /api`

The request body is the usual GraphQL JSON (`query`, `variables`, `operationName`), or an array
of such objects, which the server runs as one batch and answers with an array. A batch holds
at most `RIM_API_MAX_BATCH` requests (default 20); a larger one is answered 400 with
`{"error":"batch_too_large"}` and none of it runs. Apart from that and the refusals under
"Limits", the answer is HTTP 200; errors are in the `errors` array of each response (see
"Errors"). The server
reads an `X-Request-Id` header, or makes one, and adds it to every error as
`extensions.request_id`, so a report can be matched to the server log.

A person whose relay is another server, signed in here directly, cannot run mutations that
change their account rather than a space; those answer `FORBIDDEN` and run at their relay.

### Subscriptions: `GET /api/ws`

A WebSocket upgrade carrying the session cookie. The subprotocol is chosen with
`Sec-WebSocket-Protocol`: `graphql-transport-ws` (the protocol of the `graphql-ws` library) or
`graphql-ws` (the older `subscriptions-transport-ws` protocol). An upgrade without one of them
is refused with 400, and an upgrade without a valid session with 401: there are no anonymous
subscriptions. A user holding as many connections as `RIM_RATE_WS_CONNECTIONS_PER_USER` allows
is refused with 429 and `{"error":"too_many_connections"}`. A subscription started past
`RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION` on one connection answers one error with the code
`TOO_MANY_SUBSCRIPTIONS` and ends; ending another subscription frees its place. The socket runs
subscriptions only: a query or a mutation sent on it answers one error with the code
`VALIDATION` and the message "only subscriptions over the WebSocket", runs nothing, and
completes; send those to `POST /api`. The subscription roots are `workspaceEvents` and
`relayStream`; the schema describes both. A `TypingStarted` event, sent for direct conversations
only, names the typing user by `user_id`, `username` and `display_name`, which is `null` when
they have none.

A connection lives no longer than the session it was opened with. When that session ends, the
server ends every stream on the connection and closes it with code `4401` and reason
`session ended`; nothing is sent after the close, and the connection's place under
`RIM_RATE_WS_CONNECTIONS_PER_USER` is free again. A client that sees `4401` signs in again
before it reconnects. The session ends this way:

- **At once** when this server process ends it: `logout`, `logoutOtherSessions`, a password
  reset, a requested account deletion (every other session), a device revocation or an identity
  recovery, and the account purge.
- **At its expiry**, read when the connection opens.
- **Within `RIM_WS_SESSION_RECHECK_SECS`** (default 30) when it ended where this process did
  not hear of it: in another server process, from the command line (`rim_server principal block`,
  `rim_server account purge`), or in the database. Each server process reads the sessions of
  its open connections again every interval, in one query per 500 sessions; `0` turns the
  recheck off.
- **Before the next operation**: every operation sent on the connection first checks the
  session again, before the subscriptions-only refusal. An ended session closes the connection
  with `4401` instead of answering the operation.

### The schema: `GET /api/sdl`

The whole GraphQL schema of the running build, in SDL, without a session.

### Discovery: `GET /.well-known/rim-server`

A JSON document, readable from any origin (`Access-Control-Allow-Origin: *`) and cacheable for
five minutes:

```json
{
  "rim": "server-discovery",
  "keyLog": [{ "payload": "...", "signature": "..." }],
  "descriptor": { "payload": "...", "signature": "..." },
  "features": { "attachments": { "state": "enabled" } }
}
```

`keyLog` and `descriptor` are signed by the server key; `features` is unsigned and advisory.
`specs/server-to-server-v1.md` ("Discovery document", "Pinning a peer") says how to verify and
pin them.

### Probes: `/healthz` and `/readyz`

Both are unauthenticated, outside the origin check, and sent with `Cache-Control: no-store`.

- `/healthz` answers 200 `{"status":"ok","version":"<version>"}` whenever the process serves
  requests.
- `/readyz` answers 200 when every check passes and 503 otherwise, with `status`, `version` and
  `checks`: `database`, `migrations`, `serverKey` and `frankingKey`, each
  `{"status":"ok"}` or `{"status":"fail","reason":"..."}`. A result is reused for one second.

### Metrics: `/metrics`

Served only on the listener `RIM_METRICS_ADDR` names, never on the API port, and off when it is
unset. It has no authentication and is meant for the operator's Prometheus, not for clients
(`operations/metrics.md`).

### Files

The file routes are plain HTTP, not GraphQL. They answer JSON errors of the form
`{"error":"<message>"}` with a status (see "Errors from the file routes").

- **`POST /api/files/upload`** completes an attachment. The client first calls the
  `createAttachment` mutation, which returns an `attachmentId` and an `uploadToken`, then sends a
  multipart body with the text fields `upload_token` and `attachment_id` and exactly one file.
  The file is at most 25 MiB and its type is `image/*`, `text/*`, `application/pdf`,
  `application/json`, `application/zip`, `application/gzip`, or `application/octet-stream` for
  the blob of an encrypted conversation. The answer is `{"size_bytes":<n>}`. Off when the
  `attachments` feature is not enabled.
- **`POST /api/files/avatar`** stores the caller's avatar: a multipart body with one file of
  type `image/png`, `image/jpeg`, `image/gif` or `image/webp`, at most 2 MiB. The answer is
  `{"path","size_bytes","url"}`, where `url` is the `/api/files/...` path to read it back.
- **`GET /api/files/{path}`** serves a stored avatar to any signed-in user. Any path outside
  `avatars/` is refused with 403.
- **`GET /api/attachments/{id}/download`** serves an attachment to a member of its conversation.
  The blob of an encrypted conversation is served as `application/octet-stream` named
  `encrypted.bin`; the client checks its hash and decrypts it. Off when the `attachments` feature
  is not enabled.
- **`POST /api/plugin-bundles/{server_id}`** stores a web plugin bundle for a workspace. The body
  is the bundle itself, not multipart, sent as `application/vnd.rim.plugin-bundle+json`, at most
  8 MiB. The answer is `{"sha256","size","url"}`, with 201 for a new bundle and 200 for one the
  workspace already had; a bundle the service refuses is 422. Off when the `workspace_plugins`
  feature is not enabled.
- **`GET /api/plugin-bundles/{server_id}/{sha256}`** serves a bundle to a member of the
  workspace, with `Cache-Control: private, max-age=31536000, immutable`, since a bundle is
  named by its hash.
- **`GET /api/reports/{report_id}/attachments/{blob_sha256}`** serves the encrypted blob of an
  attachment of a reported encrypted message to an Owner or Moderator of the report's
  workspace. `blob_sha256` is unpadded base64url. Every refusal of a signed-in caller is a bare
  404.

`specs/web-plugins-v1.md` and `specs/workspace-plugins-v1.md` describe the plugin bundle
contract; `specs/e2ee-mls-v1.md` describes encrypted attachments and reports.

### Spaces on another server: `/api/remote/{host_server_id}/{tail}`

A client talks only to its relay. A request for a space hosted on another server goes to
`/api/remote/<host server id>/<host-relative path>`, for example
`POST /api/remote/rims1_.../api` for GraphQL, with the `Rim-Device-Request` and
`Rim-Device-Signature` headers. The relay checks the session (and CSRF for `POST`), forwards
the body unchanged and returns the host's status, body, `Content-Type`, `Rim-Host-Response`,
`Rim-Host-Signature` and `Retry-After` unchanged; the client verifies the host's signature.
The relay's own refusals are `{"error":"<code>","message":"..."}`, and a host that gave no
verified answer is 502 with `"error":"upstream_unavailable"` and `hostServerId`. Bodies are
limited to 32 MiB. `specs/server-to-server-v1.md` ("Spaces hosted elsewhere: the client's
path") is the contract.

### Server to server: `/s2s/v1/...`

Requests between servers, signed with the RFC 9421 profile of `specs/server-to-server-v1.md`
("The S2S transport"). They carry no cookies and sit outside the origin check. A client never
calls them.

## Sessions and CSRF

### The session cookie

A session is the `rim_session` cookie: `HttpOnly`, `SameSite=Lax`, `Path=/`, and `Secure` when
`RIM_FORCE_SECURE_COOKIES=true` or when a trusted proxy (`RIM_TRUST_FORWARDED_HEADERS=true`)
reports `https`. These mutations set it, and need no session or CSRF header:

- **Key login.** `keyLoginChallenge` returns `{ challengeId, nonce, expiresAt }`. The client
  signs a `login` statement over the nonce with its device key and calls `loginWithKey(proof)`.
  The session is bound to that device. `specs/key-identity-v1.md` ("Challenge login") has the
  statement, the audiences and the limits.
- **Key-only signup.** `signupWithKey` creates the account and signs it in.
- **Password login.** `login(input: { username, password })`. An account without a password,
  such as a key-only account or one that enrolled an identity key, cannot sign in this way.
- **Invite signup.** `signupWithInvite` creates a password account and signs it in.

When the server has terms of service (`serverInfo.termsUrl` is set), both signups must accept
them: `acceptedTermsUrl` in the input carries that URL unchanged. A signup without it, or with
another URL, answers `VALIDATION`. The server records the acceptance and its time on the
account. Without terms on the server the field is ignored.

`logout` ends the session and clears both cookies; `logoutOtherSessions` ends every other
session of the account. Either also closes the WebSocket connections of the sessions it ends
(see "Subscriptions").

A session lasts `RIM_SESSION_TTL_DAYS` days (default 30) from sign-in: the cookie's `Max-Age`
and the server-side expiry are both set from it, and using the session does not extend it. A
missing, unknown, revoked or expired session cookie makes the request anonymous; a field that
needs a session then answers `UNAUTHENTICATED`.

### Handing over or deleting a workspace

A workspace has exactly one owner. `transferWorkspaceOwnership(input: { serverId, userId })`
makes another member the owner and the caller a moderator, in one step, and answers the
workspace; both changes are announced with `MemberRoleChanged`. Only the owner may call it, the
new owner must be a member and not a guest (`NOT_FOUND`, `VALIDATION`), and `setMemberRole` still
never grants the owner role.

`deleteWorkspace(input: { serverId, confirmName })` deletes the workspace for good, with its
members, channels and conversations, their messages, files, pins and threads, invites, workspace
state, keybinds, pipelines, macros, plugins, custom emoji and the notification rules scoped to it,
and answers `{ deleted }`. Only the owner may call it and `confirmName` must be the workspace's
name (`VALIDATION` otherwise). Every member gets `MemberLeft` for themself and their event stream
for the workspace ends; the workspace then answers like one they never joined. The direct space
can be neither handed over nor deleted (`VALIDATION`).

### Deleting an account

`requestAccountDeletion(input: { confirmUsername })` schedules the signed-in account for
deletion and answers `{ scheduledFor }`. The caller types their username again; a different one
answers `VALIDATION`, and so does a request from the owner of a workspace that has other
members, who hands it over or deletes it first (previous section; a workspace where the account
is the only member is deleted with the account). A request while one is pending answers `CONFLICT`, and a person from
another server, who deletes their account at their relay, gets `FORBIDDEN`. The request ends
every other session of the account.

The deletion waits out a grace period, `RIM_ACCOUNT_DELETION_GRACE_DAYS` (default 14). Until it
ends the account works as before, `me.deletionScheduledFor` holds the date (null when nothing is
pending, and on every other user), and `cancelAccountDeletion` keeps the account; it returns
whether a deletion was pending. Both mutations need a session and the CSRF header.

When the grace period ends, the hourly purge removes the account's sessions, devices, keys,
memberships, plaintext messages and files, settings and personal automations, and frees its
username. The user row stays as a tombstone: `deleted-` and the start of its id as the username,
"Deleted account" as the display name, no email, password or key. It cannot sign in and is in no
member list or search result. It stays the author of the account's encrypted messages, which are
kept so a conversation's history does not shift, and of a message others replied to, which shows
as deleted (`isDeleted: true`, no text) with the replies under it. The tombstone keeps the
account's key identity id, so the same key identity cannot sign up on this server again:
`signupWithKey` with it answers `VALIDATION`.
`operations/background-jobs.md` and `self-hosting.md` have the operator's side.

### The CSRF token

The mutations that set a session also set the `rim_csrf` cookie, with the same attributes except
that it is readable by scripts (not `HttpOnly`). Its value is an HMAC over the session token. The
client copies it into the `X-CSRF-Token` header. The server accepts a request when the header
equals the cookie and the token is bound to the request's session cookie.

Every mutation that starts a session (`login`, `loginWithKey`, `signupWithInvite` and
`signupWithKey`) also returns the same token as `csrfToken` in its payload. A web client reads the
cookie. A client that cannot read it, such as a packaged desktop app talking to a server on
another host, keeps `csrfToken` in memory and sends it in `X-CSRF-Token`. The cookie still has to
reach the server with the request: it carries the same attributes as the session cookie, so a
request that sends the one sends the other.

The header is required on:

- every GraphQL request whose operation is a mutation, with the exceptions below. The check
  runs once for the whole batch, before anything executes, on the operation the request's
  `operationName` selects. A query that cannot be parsed counts as needing it, and so does a
  document whose mutation the server cannot classify completely: an operation that cannot be
  selected, a fragment it does not define, fragments that spread each other, or fragments
  nested deeper than 32;
- `POST /api/files/upload`, `POST /api/files/avatar` and `POST /api/plugin-bundles/{server_id}`;
- `POST /api/remote/...`.

Queries and subscriptions never need it. The mutations used before a session exists are exempt:
`login`, `logout`, `signupWithInvite`, `requestPasswordReset`, `resetPassword`,
`keyLoginChallenge`, `loginWithKey`, `signupWithKey`, `sendDeviceLinkOffer` and
`applyRemoteRevocation`. `rotateIdentityKey` and `cancelDeviceLink` are exempt only when the
request carries no session cookie. A request is exempt only when every root field of its
mutation is, counting the fields of named and inline fragments at any depth and fields under
`@skip` or `@include` whatever their condition.

A GraphQL request refused by the check gets an error with a message such as
`csrf validation failed` and no `extensions.code`; a file route answers 403.

## Origins

A request without an `Origin` header passes. A request whose `Origin` is not allowed is refused
with 400 on every route under `/api`. The allowed origins are:

- **The server's own origin.** The origin of `RIM_PUBLIC_BASE_URL` when it is set. Otherwise the
  origin the request addressed: its scheme and `Host`, or `X-Forwarded-Proto` and
  `X-Forwarded-Host` when `RIM_TRUST_FORWARDED_HEADERS=true`.
- **One configured extra origin**, `RIM_CORS_ALLOWED_ORIGIN`, such as a development server of
  the web client on another port.
- **The packaged desktop app**, from any server without configuration: `tauri://localhost`,
  `http://tauri.localhost` and `https://tauri.localhost`.

Origins compare without case and without a default port. The browser may send `Content-Type`,
`Authorization`, `X-CSRF-Token` and `X-Request-Id`, with credentials, on `GET`, `POST` and
`OPTIONS`; a preflight is cached for an hour, and the browser may read the `Retry-After` header
of an answer. The discovery document, the probes and `/s2s/v1/...` are outside this check.

## Limits

An operator can limit what one client sends. Every limit is off unless its variable is set to
a number above 0, so a private server limits nothing but the batch size.

| Variable | Limits | Over the limit |
|---|---|---|
| `RIM_RATE_REQUESTS_PER_MIN_PER_IP` | Requests to routes under `/api` per client address per minute, with a burst of the same size. IPv6 addresses count per /64. | HTTP 429, `Retry-After` and `{"error":"rate_limited","retryAfterSeconds":n}` |
| `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION` | Mutation fields per session per minute, each field, alias and fragment expansion counted, a fragment spread twice counting twice; a request without a session counts against its address. | `RATE_LIMITED` with `retryAfterSeconds`; nothing in the request runs |
| `RIM_RATE_MESSAGES_PER_MIN_PER_USER`, `RIM_RATE_MESSAGES_BURST` | Messages per user per minute, sent, replied, forwarded or encrypted, with a burst defaulting to the per-minute number. | `RATE_LIMITED` with `retryAfterSeconds` on the mutation |
| `RIM_RATE_WS_CONNECTIONS_PER_USER` | Open WebSocket connections per user. | HTTP 429 on the upgrade |
| `RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION` | Active subscriptions per WebSocket connection. | `TOO_MANY_SUBSCRIPTIONS` on the subscription |
| `RIM_API_MAX_BATCH` | GraphQL requests in one batch, default 20. | HTTP 400 and `{"error":"batch_too_large"}` |

The client address is the TCP peer, or with `RIM_TRUST_FORWARDED_HEADERS=true` the client a
`Forwarded` or `X-Forwarded-For` header names. The limits are kept in memory by each server
process, so a restart forgets them and two processes behind one proxy each allow the full
amount. `/healthz`, `/readyz`, discovery and `/s2s/v1/...` are outside the per-address limit;
server-to-server traffic has its own, `RIM_S2S_PEER_RATE_PER_MIN`.

An operator can also cap how much of each thing the instance holds. Every cap is off unless its
variable is set to a number above 0; `serverInfo.limits` reports each one, so a client can show
"5 of 5" before it tries. Creating one more past a cap answers `VALIDATION`, with a message
naming the cap, such as "this instance allows 5 workspaces per user".

| Variable | `serverInfo.limits` | Caps |
|---|---|---|
| `RIM_LIMIT_WORKSPACES_PER_USER` | `workspacesPerUser` | Workspaces one user owns. |
| `RIM_LIMIT_MEMBERS_PER_WORKSPACE` | `membersPerWorkspace` | Members of a workspace, by invite or signup; the direct space has none. |
| `RIM_LIMIT_CHANNELS_PER_WORKSPACE` | `channelsPerWorkspace` | Channels of a workspace, public and private; a new workspace gets only the default channels the cap allows. |
| `RIM_LIMIT_DEVICES_PER_USER` | `devicesPerUser` | Devices on one account. |
| `RIM_LIMIT_DM_REQUESTS_PER_DAY` | `dmRequestsPerDay` | DM requests one user sends to strangers per day. |
| `RIM_LIMIT_MACROS_PER_WORKSPACE` | `macrosPerWorkspace` | Macros of a workspace. |
| `RIM_LIMIT_PIPELINES_PER_WORKSPACE` | `pipelinesPerWorkspace` | Pipelines of a workspace. |
| `RIM_LIMIT_PLUGINS_PER_WORKSPACE` | `pluginsPerWorkspace` | Plugins of a workspace's registry, at most the built-in 100. |
| `RIM_LIMIT_INVITES_PER_USER_PER_DAY` | `invitesPerUserPerDay` | Workspace invites one user creates in the last 24 hours. |
| `RIM_LIMIT_INVITE_MAX_USES` | `inviteMaxUses` | The uses one invite may have. |
| `RIM_LIMIT_MESSAGE_BYTES` | `messageBytes` | A message's text in UTF-8 bytes, at most 65536. An encrypted message's ciphertext may exceed it by 4096 bytes of framing. |

Devices and DM requests also have a relay quota, `RIM_RELAY_DEVICES_PER_USER` (default 32) and
`RIM_RELAY_DM_REQUESTS_PER_DAY` (default 20). The stricter of the cap and the quota applies,
and `serverInfo.limits` reports that number, so `devicesPerUser` and `dmRequestsPerDay` are
never null. A DM request past the daily number stays `RATE_LIMITED` with `retryAfterSeconds`,
as the quota answered before.

`RIM_PROBATION_HOURS` puts new accounts on probation for that many hours after their creation;
unset or 0 turns it off, and an account older than the setting is judged by its age like any
other. During probation an account cannot create a workspace or send a DM request to a stranger,
someone neither its contact nor in a workspace with it, and both answer `VALIDATION` saying when
it can, in minutes rounded up ("a new account can create a workspace in 42 minutes"). Its
messages follow `RIM_RATE_PROBATION_MESSAGES_PER_MIN`, with a burst of the same size, instead
of `RIM_RATE_MESSAGES_PER_MIN_PER_USER`; unset, the normal rate applies. The `probationHours`
of `serverInfo.limits` reports the setting, null when off, and `me { probationEndsAt }` tells
the signed-in caller when theirs ends, null once it has.

An operator can also block an address or range (`rim_server ip block`). A blocked address is
answered 403 with `{"error":"blocked"}` and nothing else on every route but `/healthz`: the API,
the readiness probe, discovery and `/s2s/v1/...`. The address is found the same way as for the
limits.

## Instance moderation

An instance operator runs the server's moderation queue from the app: the abuse reports, the
principals the server refuses and the IP deny list. The server's operator makes an account an
instance operator with `rim_server operator grant <username>` and takes the role back with
`operator revoke`; a blocked account or one scheduled for deletion cannot be granted it.
`me { instanceOperator }` tells the signed-in caller whether they hold it (false on every other
user).

| Root | Does what the command line does |
|---|---|
| `instanceReports(openOnly: true)` | `abuse-reports list [--open]`, oldest first |
| `instanceResolveReport(id)` | `abuse-reports resolve`; `NOT_FOUND` for no such report |
| `instancePrincipalBlocks` | `principal list`, newest first |
| `instanceBlockPrincipal(principalId, reason, cascade: false)` | `principal block [--cascade]` |
| `instanceUnblockPrincipal(principalId)` | `principal unblock` |
| `instanceIpBlocks` | `ip list` |
| `instanceBlockIp(cidr, reason, expiresInDays)` | `ip block [--expires-in-days]` |
| `instanceUnblockIp(cidr)` | `ip unblock` |

Reports and blocks name the local account holding a principal (`subjectUsername`, `username`),
null when no account here holds it. Each root needs a session and the mutations the CSRF
header. Anyone but an instance operator gets `FORBIDDEN`, an app or bot acting for one too.
Each change writes the same audit line as its command, with the operator's user id as
`actor_id` (`cli` for the command line).

## Errors

### GraphQL error codes

Every error a resolver returns carries `extensions.code`. A client branches on the code, never
on the message.

| Code | Meaning | Extra extensions |
|---|---|---|
| `UNAUTHENTICATED` | No valid session, or a sign-in attempt that failed. | |
| `FORBIDDEN` | The caller may not do this. Also: a session not bound to a device asking for device-only data, a relay-only field on a server that is not a relay, a remote member's account mutation. | |
| `NOT_FOUND` | The object does not exist, or is hidden from the caller (`messaging.md` says where). | |
| `VALIDATION` | The input is malformed or out of range, or the feature does not exist in the direct space; the message says what. | |
| `CONFLICT` | The change conflicts with the current state; the message says how. | |
| `EMAIL_TAKEN` | The signup email address is already in use. | |
| `INVALID_TOKEN` | An invite token is unknown, used up or revoked. | |
| `EXPIRED_TOKEN` | An invite token has expired. | |
| `RATE_LIMITED` | Too many attempts, mutations or messages; wait before retrying (see "Limits"). | `retryAfterSeconds` |
| `TOO_MANY_SUBSCRIPTIONS` | The WebSocket connection holds as many subscriptions as it may. | `limit` |
| `CONFIRMATION_STALE` | A command confirmation expired or was already used; ask again. | |
| `CONFIRMATION_FAILED` | A command confirmation failed verification: it was made for another command, context or caller. | |
| `CONVERSATION_ARCHIVED` | The conversation is archived and read-only. | |
| `TIMED_OUT` | The caller is timed out in this workspace. | `until` (RFC 3339) |
| `STALE_EPOCH` | An MLS message or Commit for an epoch that is not current; fetch the Commits and retry. | `incarnation`, `epoch` |
| `MLS_COMMIT_REQUIRED` | An encrypted message needs a Commit first. | `reason`: `pendingRemovals` or `noGroup` |
| `FEATURE_DISABLED` | The feature is off or unavailable on this instance; `serverInfo.features` says why. | `feature` |
| `UPSTREAM_UNAVAILABLE` | Another server this request needed could not be reached or gave no verified answer. | `hostServerId` (or null), `origin` when known |
| `UNAVAILABLE` | A dependency of the server is unavailable. | |
| `TIMEOUT` | A dependency of the server timed out. | |
| `INTERNAL` | A server fault; the log has the detail under the error's `request_id`. | |

Errors raised by GraphQL itself, before a resolver runs (a query that does not parse or does not
match the schema), carry no `code`, and neither does the CSRF refusal.

### Refusals before a route runs

The limits and the IP deny list under "Limits" refuse a request before its route runs, with a
JSON body naming the refusal:

| Status | Body | When |
|---|---|---|
| 400 | `{"error":"batch_too_large"}` | A GraphQL batch over `RIM_API_MAX_BATCH`. |
| 429 | `{"error":"rate_limited","retryAfterSeconds":n}`, with `Retry-After: n` | The client address is over `RIM_RATE_REQUESTS_PER_MIN_PER_IP`. |
| 429 | `{"error":"too_many_connections"}` | A WebSocket upgrade over `RIM_RATE_WS_CONNECTIONS_PER_USER`. |
| 403 | `{"error":"blocked"}` | The client address is on the operator's IP deny list. |

### Errors from the file routes

The file and plugin bundle routes answer `{"error":"<message>"}` with:

| Status | When |
|---|---|
| 400 | A malformed request or a validation failure. |
| 401 | No valid session. |
| 403 | No permission, a failed CSRF check, a timeout (with `until`), or a feature that is not enabled (with `"code":"FEATURE_DISABLED"` and `feature`). |
| 404 | Not found. |
| 409 | A conflict, or an archived conversation. |
| 413 | The body is over the route's limit. |
| 415 | A file type the route does not take. |
| 422 | A plugin bundle the service refused. |
| 429 | Rate limited, with a `Retry-After` header and `retryAfterSeconds`. |
| 500 | A server fault. |

## Pagination

Lists that can grow without bound use cursor pagination over a `(timestamp, id)` pair: each list
has a fixed sort order, a request names a cursor and a direction (`Before` or `After` the cursor
in that order), and a page returns its items with a `next` cursor to continue in the same
direction and a `prev` cursor to go back. `pagination.md` is the contract and lists the order of
each list.

## Telling which server you talk to

`serverInfo`, readable without a session, returns:

- **`version`**: the server's release version, such as `0.1.0`.
- **`apiVersion`**: the major version of the GraphQL API, an integer. It goes up when a field
  or argument is removed or changes meaning, so a client built for a lower or higher number
  knows it may misread the server. Additions do not change it.
- **`features`**: every optional feature with `key`, `state` (`ENABLED`, `DISABLED` or
  `UNAVAILABLE`) and, for an unavailable one, `reason`. A client hides the surfaces of a feature
  that is not enabled and ignores a `key` it does not know (`operations/features.md`).
- **`auditRetentionDays`**: how many days a command audit row is kept, or null when rows are
  kept forever (`operations/audit-retention.md`).
- **`instanceNotice`**: a short text from the operator, at most 500 characters, such as who
  runs the server; null when there is none. A client shows it where people sign up or sign in.
- **`termsUrl`**: the terms of service a signup accepts, an absolute http(s) URL; null when the
  server has none. A client links it at signup and sends it back as `acceptedTermsUrl`.
- **`limits`**: the instance's caps ("Limits"), each an integer or null when there is none,
  and `probationHours`, the probation of new accounts.

The discovery document carries the same `features` for a client that has not connected yet.

## The full GraphQL reference

The schema the server runs is the reference: `GET /api/sdl` returns it in SDL, with every
type, field and argument and their descriptions. The `rim-ui` clients generate their TypeScript
types from it:

```sh
curl -s http://localhost:8080/api/sdl > ../rim-ui/schema.graphql
cd ../rim-ui && npm run codegen
```

Enum values appear in `SCREAMING_SNAKE` and fields in `camelCase`. The models in `messaging.md`,
`search.md`, `message-change-feed.md`, `command_model.md` and the specs in `specs/` explain what
the fields mean.
