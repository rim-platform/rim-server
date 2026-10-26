# Talking to a server over HTTP

The HTTP contract a client sees: the routes, how a session and its CSRF token work, which
origins a browser may call from, the error codes, pagination and how a client tells which
server version it talks to. The GraphQL types and fields themselves are in the schema
(`GET /api/sdl`, see "The full GraphQL reference"); the server-to-server transport is in
`specs/server-to-server-v1.md`.

## Endpoints

Everything except `/metrics` is served on `RIM_SERVER_HOST:RIM_SERVER_PORT`. "Session" means
the route needs the session cookie; "with its secret" that a session signed in without "Keep
me signed in" also needs its secret in the `X-RIM-Session-Secret` header; "CSRF" means it also
needs the `X-CSRF-Token` header (see "Sessions and CSRF").

| Method | Path | Needs | What it does |
|---|---|---|---|
| `POST` | `/api` | Session (with its secret) for most fields; CSRF on mutations | GraphQL. The body is one GraphQL request or a JSON array of them (a batch). |
| `GET` | `/api` | Nothing | The GraphiQL explorer, only when `RIM_ENABLE_GRAPHIQL=true`. |
| `GET` | `/api/ws` | Session | GraphQL subscriptions over a WebSocket. |
| `GET` | `/api/sdl` | Nothing | The schema in SDL, as `text/plain`. |
| `GET` | `/.well-known/rim-server` | Nothing | The signed discovery document. |
| `GET` | `/healthz` | Nothing | Liveness. |
| `GET` | `/readyz` | Nothing | Readiness. |
| `GET` | `/metrics` | Nothing | Prometheus metrics, on a separate listener. |
| `POST` | `/api/files/upload` | Session (with its secret), CSRF | Upload the bytes of an attachment. |
| `POST` | `/api/files/avatar` | Session (with its secret), CSRF | Upload the caller's avatar. |
| `GET` | `/api/files/{path}` | Session | Download a stored avatar. |
| `GET` | `/api/attachments/{id}/download` | Session | Download an attachment. |
| `POST` | `/api/plugin-bundles/{server_id}` | Session (with its secret), CSRF | Upload a web plugin bundle to a workspace. |
| `GET` | `/api/plugin-bundles/{server_id}/{sha256}` | Session | Download a web plugin bundle. |
| `GET` | `/api/reports/{report_id}/attachments/{blob_sha256}` | Session | A reported encrypted attachment, for moderators. |
| `GET`, `POST` | `/api/remote/{host_server_id}/{tail}` | Session (with its secret); CSRF on `POST` | A request to a space hosted on another server, through this relay. |
| any | `/s2s/v1/...` | A server signature | The server-to-server API; not for clients. |

### GraphQL: `POST /api`

The request body is the usual GraphQL JSON (`query`, `variables`, `operationName`), or an array
of such objects, which the server runs as one batch and answers with an array. The body is read
as JSON whatever its `Content-Type`; a `multipart/*` body (the GraphQL multipart upload format)
is answered 415 with `{"error":"unsupported_media_type","message":"..."}` before it is read, as
the schema takes no file uploads (files go through the routes under `/api/files`). A batch holds
at most `RIM_API_MAX_BATCH` requests (default 20); a larger one is answered 400 with
`{"error":"batch_too_large"}` and none of it runs. The body is at most 12 MiB, enough for
`submitMlsCommit` with a Commit and a Welcome at their 4 MiB caps in base64; it holds at most
262,144 of the bytes `,`, `[` and `{`, counted anywhere in it, strings included, and its GraphQL
documents (every `query` of a batch together) are at most 128 KiB. A body past any of these is
answered 413 with `{"error":"too_large","message":"..."}` before it is parsed, and none of it
runs. Apart from these and the refusals under "Limits", the answer is HTTP 200; errors are in the `errors` array of each response (see
"Errors"). The server
reads an `X-Request-Id` header, or makes one, and adds it to every error as
`extensions.request_id`, so a report can be matched to the server log.

A person whose relay is another server, signed in here directly, cannot run mutations that
change their account rather than a space; those answer `FORBIDDEN` and run at their relay.

An encrypted DM or group (`createConversation(input: { encrypted: true })`) needs a creator with a
device set up for encryption, else `VALIDATION`. Every other member without such a device is
invited instead: `Conversation.pendingMemberIds` lists them, apart from `memberIds` and
`memberCount`. An invitee hears of it through `MemberAddedToConversation` and finds it in
`inboxConversations` with `isMember: false` and `mlsGroup` null, but reading, sending or opening
it answers `FORBIDDEN`, as for a non-member, it has no unread count and nothing notifies them.
`acceptConversationInvite(serverId, conversationId)` makes them a member once a device of theirs
is set up (`VALIDATION` before), emits `MemberAddedToConversation` and `MlsMembershipChanged`, and
a member's device adds them to the group; they read nothing from before. Accepting again changes
nothing. `declineConversationInvite` deletes the invitation; both answer `NOT_FOUND` without one.
`addConversationMembers` invites such users too, with the same authorization as adding members.
An invitation not accepted within 30 days expires. `specs/e2ee-mls-v1.md` is the contract.

### Subscriptions: `GET /api/ws`

A WebSocket upgrade carrying the session cookie. The subprotocol is chosen with
`Sec-WebSocket-Protocol`: `graphql-transport-ws` (the protocol of the `graphql-ws` library) or
`graphql-ws` (the older `subscriptions-transport-ws` protocol). An upgrade without one of them
is refused with 400, and an upgrade without a valid session with 401: there are no anonymous
subscriptions. A user holding as many connections as `RIM_RATE_WS_CONNECTIONS_PER_USER` allows
is refused with 429 and `{"error":"too_many_connections"}`. A subscription started past
`RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION` on one connection answers one error with the code
`TOO_MANY_SUBSCRIPTIONS` and ends; ending another subscription frees its place. A subscription
operation selects exactly one root field: one with more, aliases, repeated fields and fragment
expansions each counted, answers one error with the code `VALIDATION` and the message "a
subscription selects exactly one root field", over the WebSocket or anywhere else, and starts
nothing. A client
message over 256 KiB, whole or in fragments, closes the connection with code `1009` ("message
too large"), and more than 32 messages waiting for the server close it with `1008` ("too many
messages"). A connection starts at most 10 subscriptions a second, in bursts of up to 50: a
`subscribe` (or `start`) message past that closes it with `1008` ("too many subscriptions
started"). The socket runs subscriptions only: a query or a mutation sent on it answers one
error with the code `VALIDATION` and the message "only subscriptions over the WebSocket", runs
nothing, and completes; send those to `POST /api`. The subscription roots are `workspaceEvents` and
`relayStream`; the schema describes both. A `TypingStarted` event, sent for direct conversations
only, names the typing user by `user_id`, `username` and `display_name`, which is `null` when
they have none.

A session signed in without "Keep me signed in" opens its connection with the cookie alone,
and its first message must be `connection_init` with the session secret as
`payload.sessionSecret`. Any other first message, or a wrong secret, closes the connection with
code `4401` and reason `session secret required`. The server notes when such a session's last
connection closed, for the reload window ("Keep me signed in" below).

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
  "features": { "attachments": { "state": "enabled" } },
  "registration": { "full": false }
}
```

`keyLog` and `descriptor` are signed by the server key; `features` and `registration` are
unsigned and advisory. `registration.full` is true while the server holds as many accounts as
`RIM_LIMIT_ACCOUNTS` allows ("Limits"), so a client can say so before anyone tries to sign up.
`specs/server-to-server-v1.md` ("Discovery document", "Pinning a peer") says how to verify and
pin them. The descriptor's `handleDomain` is the domain of the server's handles ("Handles"); it
is optional, so a client accepts a descriptor with or without it. At a handle domain that is not
a server's own host the document is instead `{"delegate": "<origin>"}`, the origin of the
server holding that domain's handles (`specs/server-to-server-v1.md`, "Handles").

### Probes: `/healthz` and `/readyz`

Both are unauthenticated, outside the origin check, and sent with `Cache-Control: no-store`.

- `/healthz` answers 200 `{"status":"ok","version":"<version>"}` whenever the process serves
  requests.
- `/readyz` answers 200 when every check passes and 503 otherwise, with `status`, `version` and
  `checks`: `database`, `migrations`, `serverKey`, `frankingKey` and `linkKey`, each
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
  The file is at most 25 MiB and its type is `image/*`, `text/*`, `audio/*`, `video/*`,
  `application/pdf`, `application/json`, `application/zip`, `application/gzip`, or
  `application/octet-stream` for the blob of an encrypted conversation. The answer is `{"size_bytes":<n>}`. Off when the
  `attachments` feature is not enabled. The ticket rejects unsupported plaintext media types
  before accepting bytes. After uploading, call `finalizeAttachment` to mark the file `READY`;
  finalizing an already ready file succeeds again. Attachment creation, finalization and deletion
  emit `MESSAGE_UPDATED`; attachment writes also advance the parent message's change-feed version.
- **`POST /api/files/avatar`** stores the caller's avatar: a multipart body with one file of
  type `image/png`, `image/jpeg`, `image/gif` or `image/webp`, at most 1 MiB (413 past it),
  whose bytes start with that format's signature (PNG `89 50 4E 47 0D 0A 1A 0A`, JPEG
  `FF D8 FF`, GIF `GIF87a` or `GIF89a`, WebP `RIFF` with `WEBP` at byte 8); another type, or
  bytes that do not match the declared one, is 415. The server decodes the file as its declared
  type and re-encodes it: the first frame of an animated GIF or WebP, centre-cropped to a
  square, is stored as a static 256×256 PNG with no metadata. Bytes that do not decode are 415
  (`avatar could not be read as an image`), an image wider or taller than 4096 pixels 413
  (`avatar image must be at most 4096×4096 pixels`). When the server runs the profile picture
  check (`RIM_AVATAR_CHECK_URL`), the re-encoded picture is then scored for sexual content, gore
  and violence: at the block threshold the upload answers 422 (`this image isn't allowed as a
  profile picture`), at the report threshold it is stored and reported to the server's
  operators, and while the check cannot run it answers 503 (`profile pictures can't be checked
  right now; try again later`); neither refusal stores anything. It is stored as
  `avatars/<user id>/avatar-<16 hex>.png`, named by the first eight bytes of the stored PNG's
  SHA-256, and the caller's other avatar files are deleted. Before the body is read the upload
  is refused with 403 while this server's operators lock the caller's profile, with 400
  (`a new account can set a profile picture in … minutes`) during probation, and with 429 and
  `Retry-After` past `RIM_AVATAR_UPLOADS_PER_HOUR` or `RIM_AVATAR_UPLOADS_PER_DAY` (see
  "Limits" below); every attempt counts against those. The answer is
  `{"path","size_bytes","url"}`, where `url` is the `/api/files/...` path to read it back and
  the only value `updateMyProfile` takes as `avatarUrl` besides an empty one. Off when the
  `avatars` feature is not enabled.
- **`GET /api/files/{path}`** serves a stored avatar to any signed-in user, as an attachment
  with the type its extension implies and `X-Content-Type-Options: nosniff`. Any path outside
  `avatars/` is refused with 403. Off when the `avatars` feature is not enabled.
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
  The session is bound to that device. An account of this server also proves its password
  first: `passwordLoginStart` and `passwordLoginFinish`, whose ticket goes to
  `loginWithKey(proof, passwordTicket)` ("Account password" below). `specs/key-identity-v1.md`
  ("Challenge login") has the statement, the audiences and the limits.
- **Key signup.** `signupWithKey` creates the account with its password
  (`passwordRegistration`, after `passwordRegistrationStart`) and signs it in. An instance
  invite bound to an email address gives the account that address (`EMAIL_TAKEN` when another
  account has it); otherwise the account has no email.

The signup stores the username trimmed and in lowercase, so `Alice` signs up as `alice`, and a
username that differs from an existing account's only in case answers `CONFLICT`.

A display name, from a signup or `updateMyProfile`, is stored without control characters,
format characters and other default-ignorable ones (zero-width spaces, word joiners, soft
hyphens, byte order marks, bidirectional overrides and isolates, variation selectors, the
combining grapheme joiner, Hangul fillers, tag characters), with each run of whitespace made one
space and no space at either end. Kept are the invisible characters emoji and scripts need: the
zero-width joiner and non-joiner (emoji sequences, Persian, Arabic), except at either end; the
text and emoji presentation selectors U+FE0E and U+FE0F right after a visible character (`❤️`);
a flag's tag characters, a run of U+E0020 to U+E007E ended by U+E007F right after the black
flag U+1F3F4 (the flags of Scotland, England and Wales); and the Mongolian free variation
selectors and vowel separator, U+180B to U+180F, right after a Mongolian letter.
What is left is at most 64 characters, or the request answers `VALIDATION`; a signup with
nothing visible left takes the username, and `updateMyProfile` clears the name. A name the
server reserves (`RIM_RESERVED_NAMES`), followed by a separator or not, answers `VALIDATION`
`this name is reserved`, also when it is written with lookalike letters from another script
(the Unicode confusable skeleton) or hidden behind invisible characters, the kept ones
included. The signup through the owner claim link of a defined default workspace may take a
reserved username and display name. A person from another
server keeps the display name of their profile card, cleaned the same way and cut to 64
characters; a reserved one is replaced by their username here.

When the server has terms of service (`serverInfo.termsUrl` is set), both signups must accept
them: `acceptedTermsUrl` in the input carries that URL unchanged. A signup without it, or with
another URL, answers `VALIDATION`. The server records the acceptance and its time on the
account. Without terms on the server the field is ignored. Terms the server serves itself
(`RIM_TERMS_FILE`) are at `/api/terms?v=<hash of the file>`: a page in HTML, and the URL
changes whenever the operator edits them, so a form that showed old terms is refused and reads
`serverInfo` again.

When the server has a minimum age (`serverInfo.minimumAge` is set), both signups must confirm
it: once the person ticks "I am at least N", `confirmedMinimumAge` in the input carries
`minimumAge` unchanged. A signup without it, or with another number, answers `VALIDATION` with
a message about the minimum age (not the terms). The age is self-declared; the server records
on the account only the age confirmed. Without a minimum age on the server the field is
ignored.

When the server names a default workspace (`serverInfo.defaultWorkspace` is set), every signup
makes the new account a Member of it, with its public channels, before the signup answers. The
payload's `serverId` names only the workspace of a workspace invite, and the workspace an owner
claim link created (an instance invite the operator's server issues for a default workspace
described in a file, "A default workspace" in `self-hosting.md`); otherwise a client opens the
default workspace from `serverInfo`.

`logout` ends the session and clears both cookies; `logoutOtherSessions` ends every other
session of the account. Each also closes the WebSocket connections of the sessions it ends (see
"Subscriptions").

A session lasts `RIM_SESSION_TTL_DAYS` days (default 30) from sign-in: the server-side expiry
is set from it, and so is the cookie's `Max-Age` for a kept session ("Keep me signed in"
below). Using the session does not extend it. A missing, unknown, revoked or expired session
cookie makes the request anonymous; a field that needs a session then answers
`UNAUTHENTICATED`.

### Keep me signed in

`loginWithKey` and `signupWithKey` take `keepSignedIn` and `keepKeyHeld` (both `Boolean!`).

- **Kept** (`keepSignedIn: true`): both cookies carry `Max-Age`, and the session needs nothing
  besides the cookie. For a web client (`keepKeyHeld: false`) the server makes a keep key, 32
  random bytes, keeps it sealed under the password key and returns it as `keepKey`; the
  client opens its data key with it. `query { sessionKeepKey }` returns it again for the same
  session, for the web client at start. A client that holds its own keep key (`keepKeyHeld:
  true`, the desktop app) gets none, and `sessionKeepKey` is `null` for it.
- **Unkept** (`keepSignedIn: false`; `keepKeyHeld` is ignored): both cookies have no `Max-Age`,
  so the browser drops them when it closes. The sign-in returns `sessionSecret`, 32 random
  bytes as unpadded base64url, of which the server keeps only the SHA-256. The client keeps it
  in memory and sends it in the `X-RIM-Session-Secret` header of every request and in the
  WebSocket's `connection_init`. A request with the cookie and without the right secret is
  anonymous. Three kinds of request need no secret: a batch whose every operation is a
  mutation of only `reloadSession` and `logout`, the WebSocket upgrade (the secret comes in
  `connection_init`), and the plain `GET` downloads (`/api/files/...`,
  `/api/attachments/{id}/download`, `/api/plugin-bundles/...`, `/api/reports/...`), which an
  `<img>` tag or a link makes without headers. Those downloads therefore still answer to the
  cookie alone.

`keepKey` and `sessionSecret` travel in the clear, protected by TLS; each is `null` when the
other case applies.

**The reload window.** A tab of an unkept session that reloads loses the secret it held in
memory. Before that, it seals what it keeps in `sessionStorage` under a reload key that only
the server holds: `setReloadKey` (with the secret) makes a new one, stores it in place of the
previous one and returns it, right after signing in and after each reload. After the reload
the tab calls `reloadSession` with the cookie and the CSRF header but without the secret. It
returns `{ reloadKey, nextReloadKey }`, the key the data is sealed under and a new one the
server now holds in its place, only when:

- the session is unkept and live, and holds a reload key;
- its last WebSocket connection closed less than 60 seconds ago, and none opened since.

The answer is given once: it clears the closing time, so a second call answers
`UNAUTHENTICATED` until a connection of the session opens and closes again. Every refusal is
`UNAUTHENTICATED`. `setReloadKey` and `reloadSession` answer `UNAUTHENTICATED` for a kept
session. Signing out or any other end of the session ends its keep key and reload key with
it.

### Handing over or deleting a workspace

A workspace has exactly one owner. `transferWorkspaceOwnership(input: { serverId, userId })`
makes another member the owner and the caller a moderator, in one step, and answers the
workspace; both changes are announced with `MemberRoleChanged`. Only the owner may call it, the
new owner must be a member and not a guest (`NOT_FOUND`, `VALIDATION`), and `setMemberRole` still
never grants the owner role. Under `RIM_LIMIT_WORKSPACES_PER_USER` a new owner who already owns as
many workspaces as the cap is refused (`VALIDATION`).

`deleteWorkspace(input: { serverId, confirmName })` deletes the workspace for good, with its
members, channels and conversations, their messages, files, pins and threads, invites, workspace
state, keybinds, pipelines, macros, plugins, custom emoji and the notification rules scoped to it,
and answers `{ deleted }`. Only the owner may call it and `confirmName` must be the workspace's
name (`VALIDATION` otherwise). Every member gets `MemberLeft` for themself and their event stream
for the workspace ends; the workspace then answers like one they never joined. The direct space
can be neither handed over nor deleted (`VALIDATION`).

### Deleting an account

`requestAccountDeletion(input: { confirmUsername })` schedules the signed-in account for
deletion and answers `{ scheduledFor }`. The caller types their username again, matched
without regard to case since usernames are stored lowercase; a different one answers
`VALIDATION`, and so does a request from the owner of a workspace that has other
members, who hands it over or deletes it first (previous section; a workspace where the account
is the only member is deleted with the account). A request while one is pending answers `CONFLICT`, and a person from
another server, who deletes their account at their relay, gets `FORBIDDEN`. The request ends
every other session of the account.

The deletion waits out a grace period, `RIM_ACCOUNT_DELETION_GRACE_DAYS` (default 14). Until it
ends the account works as before, `me.deletionScheduledFor` holds the date (null when nothing is
pending, and on every other user), and `cancelAccountDeletion` keeps the account; it returns
whether a deletion was pending. Both mutations need a session and the CSRF header.

When the grace period ends, the hourly purge removes the account's sessions, devices, keys,
memberships and join requests, plaintext messages and files, every avatar file it stored, settings and personal
automations, and frees its username. The user row stays as a tombstone: `deleted-` and the start of its id as the username,
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

Every mutation that starts a session (`loginWithKey` and `signupWithKey`) also returns the same token as `csrfToken` in its payload. A web client reads the
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
`logout`, `keyLoginChallenge`, `loginWithKey`, `signupWithKey`, `passwordRegistrationStart`,
`passwordLoginStart`, `passwordLoginFinish`, `passwordResetChallenge`, `passwordResetStart`,
`passwordResetFinish`, `sendDeviceLinkOffer` and `applyRemoteRevocation`. `rotateIdentityKey` and `cancelDeviceLink` are exempt only when the
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
`Authorization`, `X-CSRF-Token`, `X-Request-Id` and `X-RIM-Session-Secret`, with credentials, on
`GET`, `POST` and `OPTIONS`; a preflight is cached for an hour, and the browser may read the
`Retry-After` header of an answer. The discovery document, the probes and `/s2s/v1/...` are outside this check.

## Limits

An operator can limit what one client sends. Every limit is off unless its variable is set to
a number above 0, so a private server limits nothing but the batch size.

| Variable | Limits | Over the limit |
|---|---|---|
| `RIM_RATE_REQUESTS_PER_MIN_PER_IP` | Requests to routes under `/api` per client address per minute, with a burst of the same size. IPv6 addresses count per /64. | HTTP 429, `Retry-After` and `{"error":"rate_limited","retryAfterSeconds":n}` |
| `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION` | Mutation fields per session per minute, each field, alias and fragment expansion counted, a fragment spread twice counting twice; a request another server forwards for one of its users counts against that user's device, any other request without a session against its address. | `RATE_LIMITED` with `retryAfterSeconds`; nothing in the request runs |
| `RIM_RATE_MESSAGES_PER_MIN_PER_USER`, `RIM_RATE_MESSAGES_BURST` | Messages per user per minute, sent, replied, forwarded or edited, plaintext or encrypted, with a burst defaulting to the per-minute number. Resending an encrypted edit that is already stored does not count. Slow mode does not apply to edits. | `RATE_LIMITED` with `retryAfterSeconds` on the mutation |
| `RIM_RATE_WS_CONNECTIONS_PER_USER` | Open WebSocket connections per user. | HTTP 429 on the upgrade |
| `RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION` | Active subscriptions per WebSocket connection. | `TOO_MANY_SUBSCRIPTIONS` on the subscription |
| `RIM_API_MAX_BATCH` | GraphQL requests in one batch, default 20, whether sent here or forwarded by another server. | HTTP 400 and `{"error":"batch_too_large"}` |
| none (fixed) | Account password tries: every `passwordLoginStart` and `passwordChangeStart` counts against the account until a login finishes. Tries 1 to 3 are free; the 4th makes the next wait 1 minute, the 5th 5 minutes, the 6th 15 minutes, the 7th 1 hour, the 8th 4 hours, the 9th 12 hours; the 10th locks the password until a reset with the recovery key. Each `passwordLoginStart` also counts against the address, sharing the 20 failed key sign-ins per 15 minutes. | `RATE_LIMITED` with `retryAfterSeconds` during a wait; `PASSWORD_LOCKED` once locked |
| none (fixed) | Failed public join link lookups and joins (`publicJoinLinkPreview`, `joinByPublicLink` with an invalid link) per client address, and those a relay makes for its users (`remoteJoinChallenge`, `joinRemoteWorkspace` with a link) per relay server id: 20 within ten minutes cut the address or relay off for an hour. Successful lookups do not count. | `RATE_LIMITED` with `retryAfterSeconds` on every link lookup and join from the address or relay, valid links included |

The client address is the TCP peer, or with `RIM_TRUST_FORWARDED_HEADERS=true` the first
address in `X-Forwarded-For` (the peer when that header is missing or does not parse).
`Forwarded` is never read, so a proxy that replaces `X-Forwarded-For` but passes `Forwarded`
through cannot be used to choose an address. The limits are kept in memory by each server
process, so a restart forgets them and two processes behind one proxy each allow the full
amount. `/healthz`, `/readyz`, discovery and `/s2s/v1/...` are outside the per-address limit;
server-to-server traffic has its own, `RIM_S2S_PEER_RATE_PER_MIN`.

An operator can also cap how much of each thing the instance holds. Every cap but two is off
unless its variable is set to a number above 0; `serverInfo.limits` reports each one, so a client can show
"5 of 5" before it tries. Creating one more past a cap answers `VALIDATION`, with a message
naming the cap, such as "this instance allows 5 workspaces per user".

| Variable | `serverInfo.limits` | Caps |
|---|---|---|
| `RIM_LIMIT_WORKSPACES_PER_USER` | `workspacesPerUser` | Workspaces one user owns, created or handed over to them: `transferWorkspaceOwnership` to someone at the cap answers `VALIDATION`. |
| `RIM_LIMIT_MEMBERS_PER_WORKSPACE` | `membersPerWorkspace` | Members of a workspace, by invite, signup or public link (a join request is refused too while the workspace is full); the direct space has none. |
| `RIM_LIMIT_CHANNELS_PER_WORKSPACE` | `channelsPerWorkspace` | Unarchived channels of a workspace, public and private; a new workspace gets only the default channels the cap allows. Archiving a channel frees its place, and unarchiving one needs room. All channels, archived ones included, are capped at five times this. |
| `RIM_LIMIT_DEVICES_PER_USER` | `devicesPerUser` | Devices on one account. |
| `RIM_LIMIT_DM_REQUESTS_PER_DAY` | `dmRequestsPerDay` | DM requests one user sends to strangers per day. |
| `RIM_LIMIT_MACROS_PER_WORKSPACE` | `macrosPerWorkspace` | Macros of a workspace. A member who is neither Owner nor Moderator holds at most a fifth of them (at least one, and never more than the 50 everyone may hold). The default workspace has no per-workspace cap, only that share. |
| `RIM_LIMIT_PIPELINES_PER_WORKSPACE` | `pipelinesPerWorkspace` | Pipelines of a workspace, with the same fifth for a member who is neither Owner nor Moderator, and the same exception for the default workspace. |
| `RIM_LIMIT_PLUGINS_PER_WORKSPACE` | `pluginsPerWorkspace` | Plugins of a workspace's registry, at most the built-in 100. |
| `RIM_LIMIT_INVITES_PER_USER_PER_DAY` | `invitesPerUserPerDay` | Workspace invites one user creates in the last 24 hours, and invite emails (`recipientEmail`) one user sends in the last 24 hours. Unlike most caps it defaults to 50; 0 turns it off. |
| `RIM_LIMIT_INVITE_MAX_USES` | `inviteMaxUses` | The uses one invite may have. |
| `RIM_LIMIT_MESSAGE_BYTES` | `messageBytes` | A message's text in UTF-8 bytes, at most 65536. An encrypted message's ciphertext may exceed it by 4096 bytes of framing. |
| `RIM_LIMIT_PENDING_INVITES_PER_USER` | `pendingInvitesPerUser` | Pending invitations to encrypted conversations one user holds. Unlike the others it defaults to 50; 0 turns it off. Inviting someone who holds that many refuses the whole create or add. |
| `RIM_LIMIT_ACCOUNTS` | `accounts` | Local accounts the instance holds, those waiting out their deletion grace period included; people from other servers held here and purged accounts do not count. At the cap every `signupWithKey`, whatever token or proof-of-work it carries, answers `REGISTRATION_FULL` before validating terms, credentials or the combination of tokens. GraphQL input coercion still runs before resolvers. When full, `serverInfo.registrationFull` is true. |

The server's default workspace, which every account joins, has caps of its own that no
setting changes: a group there is started with at most 50 people per call and a plaintext one
holds at most 50 members, and `addConversationMembers` names at most 50 people per call; past
either the call answers `VALIDATION`.

Devices and DM requests also have a relay quota, `RIM_RELAY_DEVICES_PER_USER` (default 32) and
`RIM_RELAY_DM_REQUESTS_PER_DAY` (default 20). The stricter of the cap and the quota applies,
and `serverInfo.limits` reports that number, so `devicesPerUser` and `dmRequestsPerDay` are
never null. A DM request past the daily number stays `RATE_LIMITED` with `retryAfterSeconds`,
as the quota answered before.

`RIM_PROBATION_HOURS` puts new accounts on probation for that many hours after their creation;
unset or 0 turns it off, and an account older than the setting is judged by its age like any
other. The instance operator is never on probation: the role (from the default workspace's owner
claim with `claimGrantsOperator`, or `rim_server operator grant`) is read at each request, so an
account is out of probation from the moment it holds it. During probation an account cannot
create a workspace, write a workspace's shared state, or start a DM with a stranger, in the
direct space or in a workspace, someone it does not know (a contact who lists it too, or someone
either of them accepted a DM request from: "Who May Start A DM" in `docs/messaging.md`) and
shares no workspace with but the server's default workspace, which every new account joins
(`serverInfo.defaultWorkspace`); an account without a key is known only through a workspace.
It cannot set a profile picture, status or bio either (`updateMyProfile` and
`POST /api/files/avatar`); it may set its display name and clear any of them. Each answers
`VALIDATION` saying when it can, in minutes rounded up ("a new account can create a workspace in
42 minutes", "a new account can set a profile picture, bio or status in 42 minutes"). Its
messages follow `RIM_RATE_PROBATION_MESSAGES_PER_MIN`, with a
burst of the same size, instead of `RIM_RATE_MESSAGES_PER_MIN_PER_USER`; unset, the normal rate
applies. The `probationHours` of
`serverInfo.limits` reports the setting, null when off, and `me { probationEndsAt }` tells the
signed-in caller when theirs ends, null once it has and for an instance operator.

Profile picture uploads have two limits per account, on by default: `RIM_AVATAR_UPLOADS_PER_HOUR`
(5) and `RIM_AVATAR_UPLOADS_PER_DAY` (20); 0 turns either off. Every upload attempt counts, one
refused for its content too; removing a picture or setting `avatarUrl` to an uploaded one does
not, and an instance operator is exempt. Past either, `POST /api/files/avatar` answers 429 with
`Retry-After` and `retryAfterSeconds`. The limits are counted in the server's memory, so a
restart starts them afresh. `serverInfo.limits` does not report them.

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
| `instanceReports(openOnly: true, after, limit: 100)` | `abuse-reports list [--open] [--after] [--limit]`, oldest first, at most 500 a page |
| `instanceResolveReport(id)` | `abuse-reports resolve`; `NOT_FOUND` for no such report |
| `instancePrincipalBlocks` | `principal list`, newest first |
| `instanceBlockPrincipal(principalId, reason, cascade: false)` | `principal block [--cascade]` |
| `instanceUnblockPrincipal(principalId)` | `principal unblock` |
| `instanceIpBlocks` | `ip list` |
| `instanceBlockIp(cidr, reason, expiresInDays)` | `ip block [--expires-in-days]` |
| `instanceUnblockIp(cidr)` | `ip unblock` |
| `instanceClearProfile(principalId, reason, lockDays)` | `principal clear-profile [--lock-days]`; `NOT_FOUND` when no account here holds the principal |

Reports and blocks name the local account holding a principal (`subjectUsername`, `username`),
null when no account here holds it. Each root needs a session and the mutations the CSRF
header. Anyone but an instance operator gets `FORBIDDEN`, an app or bot acting for one too.
Each change writes the same audit line as its command, with the operator's user id as
`actor_id` (`cli` for the command line).

`instanceClearProfile` clears the display name, picture, status and bio of the account holding
a principal here and deletes its stored pictures; its workspaces see the change at once. With
`lockDays` (1 to 365) the account cannot set any of the four again until the lock ends:
`updateMyProfile` setting one answers `FORBIDDEN` ("this server's operators locked your profile
until …"), clearing one still works, and `POST /api/files/avatar` answers 403, an operator's
too. `me { profileLockedUntil }` tells the account when its lock ends, null while none holds
and on every other user. The outcome names the principal, the account (`InstanceAccount`) and
`lockedUntil`, and the audit line is `relay.profile_cleared` with the reason and the lock's
end. It clears what this server shows: a person whose relay is another server keeps their
display name in the signed profile card that server serves, and other servers keep showing the
copy of the card they hold.

`instanceReports` pages from the oldest report: the next page passes the last report's `id` as
`after`. A user reports a DM request once (`reportDmRequest` answers `CONFLICT` the second
time).

Reports of messages in the direct space (DMs and groups, where everyone is a guest) reach the
instance operators, not a workspace's moderators. An operator handles them with the workspace
moderation roots and the direct space's id (`relayInfo.directSpaceId`): `messageReports`,
`messageReport` (with `frankedContent`, audited as `e2ee.report_viewed` with ids only),
`resolveReport`, the report's attachment route and `deleteMessage` of a message with a verified
report, or of a reported unencrypted message (`docs/messaging.md`, "Reports"). A report of an
unencrypted message shows the operator that message's text, author and send time, and a report
whose author is shown carries the author's principal id (`messageAuthorPrincipalId`), which
`instanceBlockPrincipal` takes. Resolving a report starts the 90 days after which its verified
content is deleted.

## Errors

### GraphQL error codes

Every error a resolver returns carries `extensions.code`. A client branches on the code, never
on the message.

| Code | Meaning | Extra extensions |
|---|---|---|
| `UNAUTHENTICATED` | No valid session, or a sign-in attempt that failed. | |
| `FORBIDDEN` | The caller may not do this. Also: a session not bound to a device asking for device-only data, a relay-only field on a server that is not a relay, and a remote member's account mutation. | |
| `NOT_FOUND` | The object does not exist, or is hidden from the caller (`messaging.md` says where). Every invalid public join link answers it with the same message, "not found". | |
| `VALIDATION` | The input is malformed or out of range, or the feature does not exist in the direct space; the message says what. | |
| `CONFLICT` | The change conflicts with the current state; the message says how. | |
| `EMAIL_TAKEN` | The signup email address is already in use. | |
| `INVALID_TOKEN` | An invite token is unknown, used up or revoked. A public join link token is never a signup token and answers this too. | |
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
| `REGISTRATION_FULL` | The server holds as many accounts as it accepts (`RIM_LIMIT_ACCOUNTS`) and takes no new one. The message says so and points to running a server or signing up on another one. | |
| `PASSWORD_LOCKED` | The account's password locked after ten tries without a success; only a reset with the recovery phrase (`passwordResetStart`) opens it. | |
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
| 413 | `{"error":"too_large","message":"..."}` | A GraphQL body past its caps (see "GraphQL: `POST /api`"). |
| 415 | `{"error":"unsupported_media_type","message":"..."}` | A `multipart/*` body sent to `POST /api`. |
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
| 415 | A file type the route does not take, or an avatar whose bytes are not the declared type. |
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
- **`dmInbound`**, **`dmOutbound`**, **`dmPartners`**: the operator's DM settings, floors for
  every account (`RIM_DM_INBOUND`, `RIM_DM_OUTBOUND`, `RIM_DM_PARTNERS`; "Who May Start A DM" in
  `messaging.md`). A client shows them read-only and explains a greyed choice with
  `myDmSettings.outsideFloorFrom`.
- **`defaultWorkspace`**: the id of the workspace every new account joins as a Member
  (`RIM_DEFAULT_WORKSPACE`, or the workspace of `RIM_DEFAULT_WORKSPACE_FILE` once its owner
  claimed it), or null when the server names none. After a signup a client can open it instead
  of the empty direct space.
- **`registrationFull`**: true when the server holds as many accounts as `limits.accounts`
  allows, so every signup answers `REGISTRATION_FULL`; false when there is no cap. The answer may
  be up to half a minute old. A client says on its sign-up page that the server is full and
  points to another server or to running one.
- **`abuseContact`**: where to report abuse on this server, an email address or an https URL
  (`RIM_ABUSE_CONTACT`), or null when the operator names none. A client shows it on the
  sign-in page, as a `mailto:` link for an address. The signed descriptor carries the same
  value (`specs/server-to-server-v1.md`).
- **`minimumAge`**: the age every signup confirms the person has reached (`RIM_MINIMUM_AGE`,
  13 to 21), or null when the server asks for none. A client shows "I am at least N" at signup
  and sends N back as `confirmedMinimumAge`.

The discovery document carries the same `features`, and `registrationFull` as
`registration.full`, for a client that has not connected yet.

## Handles

`User.handle` is a person's handle, `@<username>:<domain>`: for an account on this server its
username at this server's handle domain (`RIM_HANDLE_DOMAIN`), for a person from another server
their username at their home server and that server's handle domain, such as
`@kuba:rim-platform.org`. It is null for a person from another server whose handle is not known
(their server did not say, or its domain does not point back at it) and for a deleted account;
a client then shows the display name and never the internal `r_…` username of a remote person.
The domain of `me.handle` is the viewer's own server's, so a client can show `@username` for
people whose handle has that domain and the full handle for everyone else. A handle is a name
for the person's key (`principalId`), which stays the identity: a username change changes the
handle only. `searchMessages` takes either form after `from:`: `from:@kuba` names an account on
this server, `from:@kuba:rim-platform.org` names whoever has that handle, here or elsewhere.

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

### Profile picture refresh

Avatar uploads are stored as `avatars/<user id>/avatar-<16 lowercase SHA-256 hex>.png`, the
hash of the re-encoded PNG, so changing the image changes the URL; files stored before
re-encoding keep their `.jpeg`, `.gif` or `.webp` name until replaced. Authenticated downloads of these names use
`Cache-Control: private, max-age=31536000, immutable`; unversioned avatar names require
revalidation. `updateMyProfile` accepts both name forms only for the caller's own directory.
Replacement deletes the old avatar file, and an operator's `instanceClearProfile` deletes all
of them. `MemberProfileUpdated` identifies a changed profile
in each workspace; the direct space sends the account's own change to its other sessions.
Clients refetch the directory and, for their own user id, `me` when the event arrives.

### Pinned plugin link fetch

`POST /api/plugin-bundles/fetch` takes JSON `{url, sha256}`, at most 8 KiB, with a session and
CSRF token. It returns a validated, hash-pinned bundle as an attachment, at most 8 MiB. The
`plugin_link_fetch` feature is enabled by default, independent of `workspace_plugins`. Only
public HTTPS targets are accepted, with at most ten safe redirects and a 120-second timeout.
Twenty attempts per account per hour are allowed, with four concurrent downloads per server.
Failures use the normal feature/auth/rate responses; invalid pins or bundles return 422.

### Rejoining the default workspace and removal notices

`joinDefaultWorkspace: ID!` requires a signed-in local account and returns the current default
workspace id. It preserves an existing role, or joins as Member with public channels. It
returns `NOT_FOUND` if no default is available, and `FORBIDDEN` for remote accounts or an
active ban ("you are banned from this workspace"). Older accounts and accounts whose ban
expired may use it without an invite. Unbanning a local account in the current default
workspace restores that membership automatically; other workspaces still require an invite.
Private conversations and deleted automations are never restored.

On a message's `author`, `User.banned` and `bannedUntil` describe the active ban in that
message's workspace; `banReason` is visible only to workspace moderators. Other User contexts
return false/null. `ServerMember.banned` is false for active directory members, with null
`bannedUntil`; banned users have been removed from that directory. MemberLeft carries an
optional cause and ban end; the moderator's reason is delivered only to the removed user.

### Public join link and join requests

A workspace's permanent link, `<server>/join/<token>`, lets accounts of this server join it or
ask to join, and accounts of other servers too through their own server unless `allowRemote` is
off; it never creates an account. The same `/join/<token>` path carries invite links;
the token's prefix tells the kinds apart: `rim_pjl_` is a public join link (for
`publicJoinLinkPreview` and `joinByPublicLink`), `rim_inv_` a workspace invite and `rim_iinv_` an
instance invite (for `previewInviteToken` and the signup and join-by-invite roots). `docs/messaging.md`, "Public join link and join
requests", has the rules; this is the surface.

| Root | Who | Answers |
|---|---|---|
| `publicJoinLink(serverId): PublicJoinLink!` | the Owner | settings, `token`, `url`, `joinsLastDay`, `joinsLastWeek`, `openRequests` |
| `updatePublicJoinLink(input: UpdatePublicJoinLinkInput!): PublicJoinLink!` | the Owner | `enabled`, `mode` (`DIRECT`, `REQUESTS`), `role` (`MEMBER`, `GUEST`), `allowRemote`; omitted fields stay |
| `reissuePublicJoinLink(serverId): PublicJoinLink!` | the Owner | a new token; the old one stops at once |
| `publicJoinLinkPreview(token): PublicJoinLinkPreview!` | anyone, no sign-in | `serverId`, `name`, `mode`, `viewer` (`SIGNED_OUT`, `NOT_MEMBER`, `MEMBER`, `BANNED`, `REQUEST_OPEN`), `joinRequestId` |
| `joinByPublicLink(token, message): JoinByPublicLinkPayload!` | signed in | `outcome` (`JOINED`, `REQUESTED`), `serverId`, `joinRequest` |
| `joinRequests(serverId, dir, cursor, limit): JoinRequestPage!` | Owner, Moderators | open requests, newest first; cursor `JoinRequestCursorInput {createdAt, requestId}` |
| `myJoinRequests: [JoinRequest!]!` | signed in | the caller's open requests |
| `acceptJoinRequest(serverId, requestId): JoinRequest!` | Owner, Moderators | the person joins with the link's role |
| `declineJoinRequest(serverId, requestId): JoinRequest!` | Owner, Moderators | the person may ask again after 24 hours |
| `withdrawJoinRequest(requestId): JoinRequest!` | the person | |

Refusals:

| Case | Code |
|---|---|
| Not the Owner (settings, link, new token); not Owner or Moderator (requests) | `FORBIDDEN` |
| The direct space or the server's default workspace; a role other than `MEMBER` or `GUEST` | `VALIDATION` |
| Any invalid link: unknown, switched off, replaced, its workspace deleted | `NOT_FOUND`, message "not found" |
| 20 failed lookups or joins from the address in ten minutes ("Limits") | `RATE_LIMITED` for an hour |
| The caller is banned | `FORBIDDEN`, "you are banned from this workspace" |
| The workspace is at `RIM_LIMIT_MEMBERS_PER_WORKSPACE` | `VALIDATION`, "this instance allows N members per workspace" |
| Already a member; a request already open; a request no longer open | `CONFLICT` |
| A message over 500 characters; 20 open requests; declined less than 24 hours ago | `VALIDATION` |
| A request of another workspace, or someone else's (withdraw) | `NOT_FOUND` |

`RIM_LIMIT_WORKSPACES_PER_USER` counts owned workspaces and does not refuse a join.
`JoinRequestCreated` and `JoinRequestDecided` reach the workspace's Owner and Moderators and the
person, who gets them on the direct-space subscription; `MemberJoined` carries
`"source": "public_link"` for a join through the link.

**From another server.** An account of another server uses the link through its own server, the
relay: `remoteJoinChallenge(hostServerId, hostOrigin, inviteToken, publicLinkToken)` and
`joinRemoteWorkspace(input)` with `publicLinkToken` and an optional `message`
(`docs/relay.md`, "Joining a workspace on another host"). The challenge answers the link's
`workspaceId`, `workspaceName` and `publicLinkMode`; the join answers `outcome` (`JOINED`, or
`REQUESTED` with `joinRequestId` and `joinRequestExpiresAt`). The host's checks and refusals are
the ones above, plus:

| Case | Code |
|---|---|
| The link's `allowRemote` is off | `FORBIDDEN`, "this workspace's public link does not admit accounts from other servers" |
| The host's operator blocked the relay | `FORBIDDEN`, "this server does not accept requests from that server" |
| 20 invalid links from one relay in ten minutes | `RATE_LIMITED` for an hour, counted per relay server id |
| Both an invite and a link token | `VALIDATION` |
| An account of the host presented by a relay elsewhere | `VALIDATION`: it joins through the link at the host |

The decision on a remote person's request reaches them as a `JoinRequestDecided` relay item;
`myJoinRequests` lists only the requests filed on the server answering it.

### Account password

Every account of this server has a password besides its device keys, checked with OPAQUE
(RFC 9807; `specs/key-identity-v1.md`, "Account password"). The server never sees the
password. Every OPAQUE message travels as an unpadded base64url string, and the client library
must use the suite of the spec byte for byte.

| Root | Session | Does |
|---|---|---|
| `passwordRegistrationStart(principalId, request): String!` | none | Answers the `RegistrationRequest` of a principal no account here holds, before `signupWithKey`. `CONFLICT` for a principal an account of this server holds. |
| `signupWithKey(input)` | none | Takes `passwordRegistration`, the `RegistrationUpload`, and answers `serverShare`, the account's new server share, in the clear. |
| `passwordLoginStart(principalId, request): PasswordLoginStart!` | none | Counts a try and answers `{ loginId, response }`. A principal without a password here gets an answer of the same shape that no password finishes. |
| `passwordLoginFinish(loginId, credentialFinalization): PasswordLoginFinish!` | none | Checks the password, clears the tries and answers `{ ticket, serverShare, rewraps }`: a single-use ticket valid for 120 seconds, the server share sealed under the login's transport key, and the rewraps, newest first. A wrong password or an unknown, used or expired login is `UNAUTHENTICATED`. |
| `loginWithKey(proof, passwordTicket): LoginPayload!` | none | For an account of this server, requires a ticket of that account, unexpired and unused, consumed with the new session; otherwise `UNAUTHENTICATED`. |
| `passwordChangeStart(credentialRequest, registrationRequest): PasswordChangeStart!` | yes | Counts a try; answers `{ loginId, credentialResponse, registrationResponse }`. `NOT_FOUND` without a password. |
| `passwordChangeFinish(loginId, credentialFinalization, registrationUpload, rewrap): PasswordChanged!` | yes | Replaces the password, keeps the server share, stores the rewrap (the newest three are kept) and ends every other session of the account. `UNAUTHENTICATED` for a wrong current password. |
| `passwordResetChallenge(principalId): KeyLoginChallenge!` | none | A challenge for a `password-reset` statement, valid 120 seconds, single use. |
| `passwordResetStart(statement, registrationRequest): PasswordResetStart!` | none | Verifies the statement against the recovery key of the account's key log head and answers `{ resetId, registrationResponse }`. Every refusal is `UNAUTHENTICATED`. |
| `passwordResetFinish(resetId, registrationUpload): Boolean!` | none | Replaces the password, keeps the server share, deletes the rewraps, clears the tries and the lock, and ends every session of the account. |

**Sign-in.** The client runs the password login first (`passwordLoginStart`, then
`passwordLoginFinish`), opens what it keeps at rest with the server share, signs the `login`
statement over a `keyLoginChallenge`, and sends it with the ticket to `loginWithKey`. The login
id and the ticket are each valid for 120 seconds and once. An account of this server without a
password does not sign in: `loginWithKey` answers `UNAUTHENTICATED`, as for a wrong proof. A
person from another server signs in at a host with the key alone; their password lives at
their own server.

**Tries.** See "Limits". A wrong password is `UNAUTHENTICATED`, a wait `RATE_LIMITED` with
`retryAfterSeconds`, and a locked password `PASSWORD_LOCKED`.
