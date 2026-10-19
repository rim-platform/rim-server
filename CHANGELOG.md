# Changelog

What changes between rim-server releases, for the people who run a server and the people who
write a client. The format is [Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the
versions follow [Semantic Versioning](https://semver.org/spec/v2.0.0.html); a release is the
tag `v<version>`, the version every crate in the workspace carries.

## How entries are written

- A pull request that changes what an operator, a user or a client author sees adds its line
  under `Unreleased` in the same pull request. Refactors, tests and internal documentation do
  not.
- Each line goes under one heading: `Added`, `Changed`, `Deprecated`, `Removed`, `Fixed` or
  `Security`. It says what changed for the reader, not how it was built.
- A change the operator has to act on (a new required setting, a migration that rewrites a
  large table, a contract migration, a changed default) starts with **Upgrade:** and says what
  to do. `docs/operations/upgrade.md` is how an upgrade runs.
- A change to the GraphQL schema or a spec in `docs/specs/` names the type, field or spec.
- At release time `Unreleased` becomes `## [<version>] - <YYYY-MM-DD>` and a new empty
  `Unreleased` starts above it (`docs/operations/release-checklist.md`).

## [Unreleased]

### Added

- `deploy/compose.monitoring-containers.yml`, an opt-in overlay on top of
  `compose.monitoring.yml` that adds cAdvisor, so Alloy also sends each compose service's CPU
  and memory use (job `cadvisor`). cAdvisor reaches the Docker and containerd sockets only
  through proxies that allow it read-only calls alone; it needs the new `DOCKER_GID` in
  `deploy/.env`, and a host on the overlay2 storage driver also adds
  `compose.monitoring-containers-overlay2.yml`. "Container metrics" in
  `docs/operations/metrics.md` describes it.
- `deploy/compose.monitoring.yml`, an opt-in overlay that turns on the metrics listener and
  adds a Grafana Alloy collector: every 15 seconds it scrapes rim-server's metrics, probes
  `/readyz` and reads the host's metrics, and remote-writes them to Grafana Cloud with the new
  `GRAFANA_CLOUD_METRICS_URL`, `GRAFANA_CLOUD_METRICS_USER` and `GRAFANA_CLOUD_METRICS_TOKEN`
  in `deploy/.env`. "Collecting them with Grafana Alloy" in `docs/operations/metrics.md`
  describes what it sends.
- Production monitoring metrics for local accounts and activity, committed plaintext and
  encrypted messages, GraphQL operations, WebSocket connections and pending device delivery.
  Relay delivery metrics distinguish device acknowledgements from human read receipts and
  stored queue rows. Adds `user_activity` for persistent, throttled activity tracking.
  `docs/operations/metrics.md` describes their scope and sampling.
- Who may start a DM, in three layers where each only tightens the one above ("Who May Start A
  DM" in `docs/messaging.md`). The operator sets `RIM_DM_INBOUND` (`open`, `requests` by default,
  `partners`, `none`), `RIM_DM_OUTBOUND` (`allowed` by default, `partners`, `none`) and
  `RIM_DM_PARTNERS`, shown in the new `ServerInfo.dmInbound`, `dmOutbound` and `dmPartners`; a
  workspace's Owner sets a floor for DMs from outside and a default for members' DMs with the new
  `setWorkspaceDmPolicy` (`Server.dmOutsideFloor`, `Server.dmMemberDefault`); each person chooses
  with the new `setMyDmSettings` and `setMyWorkspaceDmSetting`, read with `myDmSettings`
  (`effectiveOutside`, `outsideFloorFrom`). New enums `DmInbound`, `DmOutbound`,
  `DmOutsideFloor`, `DmMembers`, `DmOutside`, `DmFloorSourceKind`. A workspace DM a member takes
  as a request now goes through their request inbox. `docs/operations/closed-company-server.md`
  is the recipe for a server closed to DMs from outside or open to partners only.
- GraphQL `Conversation.dmRequestPending`: for a DM, true while the other member's DM request
  for it is pending, so the sender can show that their messages wait for acceptance; null for
  groups and channels.
- GraphQL `userCommand(id)` and `userCommands` return the caller's own user commands as the new
  `UserCommand` type (`UserCommandStep`), enabled or not, with their saved steps in order, so a
  client can edit a user command without asking for its steps again. `userCommand` answers null
  for a built-in id or a command that is not the caller's.

- GraphQL `ServerInfo.signupRequiresKeys`: true when new accounts sign up with a key only and
  `signupWithInvite` is refused, so a client offers only the key signup.
- Invitations to encrypted conversations: an encrypted DM or group may now include members who
  have no device ready for encryption. They are listed in the new
  `Conversation.pendingMemberIds`, see the conversation but cannot read, send or be notified,
  and join with the new `acceptConversationInvite` once a device is ready, or decline with
  `declineConversationInvite`. Invitations expire after 30 days, and
  `RIM_LIMIT_PENDING_INVITES_PER_USER` (`serverInfo.limits.pendingInvitesPerUser`, default 50)
  caps how many one user holds. Before, the server refused such members; it still refuses a
  creator who is not ready (`e2ee-mls-v1.md`).
- Instance moderation in the app: `rim_server operator grant | revoke | list` makes an account
  an instance operator, `me.instanceOperator` says whether the caller is one, and the queries
  `instanceReports`, `instancePrincipalBlocks`, `instanceIpBlocks` and the mutations
  `instanceBlockPrincipal`, `instanceUnblockPrincipal`, `instanceResolveReport`,
  `instanceBlockIp`, `instanceUnblockIp` do what the `abuse-reports`, `principal` and `ip`
  commands do; anyone else gets `FORBIDDEN`. Their audit lines now carry `actor_id`.

- `LoginPayload.csrfToken` and `SignupWithInvitePayloadGql.csrfToken`: `login`, `loginWithKey`,
  `signupWithInvite` and `signupWithKey` return the session's CSRF token, the value of the
  `rim_csrf` cookie, so a client that cannot read that cookie (a packaged desktop app talking to
  a server on another host) can send it in `X-CSRF-Token`.
- `docs/operations/upgrade.md`: how to move a server to a new release and back.
- Release images: every release tag publishes `ghcr.io/rim-platform/rim-server:<version>`
  (also `<major>.<minor>`, `sha-<short sha>` and `latest`), signed with cosign keyless by the
  release workflow and with an SPDX SBOM attested to it and attached to the GitHub release.
  `docs/self-hosting.md`, "Verify the images", checks the signature.
- GraphQL `ServerInfo.auditRetentionDays`: the days command audit rows are kept before they are
  exported and deleted, null when they are kept forever. Readable without a session, like the
  rest of `serverInfo`.
- Request limits, each off unless set: `RIM_RATE_REQUESTS_PER_MIN_PER_IP` (requests to `/api`
  routes per client address, answered 429 with `Retry-After`),
  `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION`, `RIM_RATE_MESSAGES_PER_MIN_PER_USER` with
  `RIM_RATE_MESSAGES_BURST` (both answered `RATE_LIMITED`), `RIM_RATE_WS_CONNECTIONS_PER_USER`
  and `RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION` (the new GraphQL error code
  `TOO_MANY_SUBSCRIPTIONS`). Refusals are counted in `rim_rate_limit_rejections_total`, and
  browsers may read `Retry-After`. `docs/api.md` ("Limits") describes them.
- An IP deny list: `rim_server ip block <cidr> --reason <text> [--expires-in-days <1-365>]`,
  `ip unblock <cidr>` and `ip list`. A blocked address or range, IPv4 or IPv6, is answered 403
  with `{"error":"blocked"}` on every route but `/healthz`, server-to-server routes included.
  A running server picks up changes within 30 seconds; blocks are audited as `ip.block` and
  `ip.unblock` and refusals counted in `rim_ip_blocked_requests_total`. Adds the `ip_block`
  table.
- `RIM_INSTANCE_NOTICE` and `RIM_TERMS_URL`: a short operator notice and the terms of service
  URL, served as GraphQL `ServerInfo.instanceNotice` and `ServerInfo.termsUrl`. With terms set,
  `signupWithInvite` and `signupWithKey` require `acceptedTermsUrl` (a new optional field of
  `SignupWithInviteInput` and `SignupWithKeyInput`) equal to the terms URL and answer
  `VALIDATION` otherwise; the acceptance is stored on the account.
- Instance limits, each off unless set: `RIM_LIMIT_WORKSPACES_PER_USER`,
  `RIM_LIMIT_MEMBERS_PER_WORKSPACE`, `RIM_LIMIT_CHANNELS_PER_WORKSPACE`,
  `RIM_LIMIT_DEVICES_PER_USER`, `RIM_LIMIT_DM_REQUESTS_PER_DAY`,
  `RIM_LIMIT_MACROS_PER_WORKSPACE`, `RIM_LIMIT_PIPELINES_PER_WORKSPACE`,
  `RIM_LIMIT_PLUGINS_PER_WORKSPACE`, `RIM_LIMIT_INVITES_PER_USER_PER_DAY`,
  `RIM_LIMIT_INVITE_MAX_USES` and `RIM_LIMIT_MESSAGE_BYTES`. Creating one more past a cap answers
  `VALIDATION` naming it. GraphQL `ServerInfo.limits` (type `ServerLimits`, one nullable `Int`
  per cap) reports them; for devices and DM requests it reports the stricter of the cap and the
  relay quota, which is what applies. `docs/api.md` ("Limits") lists them.
- Probation for new accounts: `RIM_PROBATION_HOURS` (off unless set) keeps an account younger
  than that from creating a workspace or sending a DM request to a stranger, answered
  `VALIDATION` naming the minutes left, and `RIM_RATE_PROBATION_MESSAGES_PER_MIN` gives it a
  lower message rate. GraphQL `ServerLimits.probationHours` reports the setting and
  `User.probationEndsAt`, read on `me`, says when the caller's probation ends.
- GraphQL `transferWorkspaceOwnership(input: { serverId, userId })` hands a workspace to another
  member, who becomes its one owner while the caller becomes a moderator, and
  `deleteWorkspace(input: { serverId, confirmName })` (answers `DeleteWorkspacePayload {
  deleted }`) deletes a workspace with everything in it once its owner types its name.
- Self-service account deletion: GraphQL `requestAccountDeletion(input: { confirmUsername })`
  (answers `AccountDeletionSchedule { scheduledFor }`), `cancelAccountDeletion` and
  `User.deletionScheduledFor`, set on `me` while a deletion is pending. After a grace period
  (`RIM_ACCOUNT_DELETION_GRACE_DAYS`, default 14, 0 to 30) an hourly job purges the account and
  leaves a tombstone ("Deleted account") as the author of its encrypted messages; `rim_server
  account purge [--dry-run]` runs the purge on demand. The only owner of a workspace with other
  members is refused until they transfer or delete it. The tombstone keeps the key identity id,
  so `signupWithKey` with a deleted account's identity answers `VALIDATION`. Signups may no
  longer take usernames starting with `deleted-`. Adds `user.deletion_requested_at`, `deletion_scheduled_for` and
  `deleted_at`.
- The invite chain and the complete ban. Every account records the invite it signed up with
  and that invite's creator (new nullable `user.created_by_invite_id` and
  `user.invited_by_user_id`). `rim_server principal block` now also revokes the account's
  invites that have uses left, blocks its email (the new `blocked_email` table: a signup with
  it answers `VALIDATION` "this email cannot sign up here", a password login `FORBIDDEN`),
  records the reason as a resolved abuse report, and lists the accounts its invites created;
  `--cascade` blocks those too, recursively, in the same transaction. `principal unblock` also
  lifts the email block; sessions and invites stay ended. The new `principal show
  <principal-id>` prints the account, its inviter, its invite tree, its invites, its open
  reports and its block.
- `RIM_RESERVED_NAMES`: comma-separated usernames and display names that `signupWithInvite`,
  `signupWithKey` and `updateMyProfile` refuse with `VALIDATION` "this name is reserved",
  compared after trimming, Unicode NFKC normalisation and lowercasing, and also followed by
  `-`, `_`, `.` or a space (`rim` reserves `rim-admin`, not `rimmer`). An account whose own
  username is reserved may still take a reserved display name. An empty entry stops the server
  at boot.
- Reaction notifications: a reaction on someone's message tells its author with the new
  `NotificationReason.REACTION`, one entry per message that each later reaction re-surfaces
  unread. `Notification.reactionEmoji`, `Notification.reactedBy`, the setting
  `NotificationSettings.reactions` / `UpdateNotificationSettingsInput.reactions` (default on),
  and `reaction_emoji` / `reaction_by` on the `NotificationDue` event.
- `RIM_DEFAULT_WORKSPACE`: the id of a workspace on this server that every new account joins as
  a Member, with its public channels, whichever way it signs up, so people land where others
  already are. GraphQL `ServerInfo.defaultWorkspace` names it, null when unset. A workspace that
  does not exist here, or the direct space, stops the server at boot.
- Plugin settings in `docs/specs/plugin-protocol-v1.md`, section 15, for plugin and client host
  authors: the rules of a manifest's `settings_schema` (`value_type` `string`, `boolean`,
  `integer`, `path` or `enum`, `is_secret`, and the optional `default`, `min`, `max`, `options`
  and `placeholder`), and `rim.settings.get` and `rim.settings.changed`, added within protocol v1
  for both plugin kinds, with the panel bridge's `rim.settings.open`, which opens the plugin's
  settings form over the current view, leaving the panel in place. Values stay on the device
  and never reach the server, so the server does not change. Sections 15 to 20 of that spec are
  now 16 to 21. `docs/decisions/plugin-settings-2026-10-18.md` records why.
- Workspace invites can be listed and revoked. The new `workspaceInvites(serverId, dir, cursor,
  limit)` gives a workspace's Owner its invites newest first as a `WorkspaceInvitePage` of
  `WorkspaceInvite` (role, uses, expiry, creator, revocation and a `WorkspaceInviteState` of
  `ACTIVE`, `EXPIRED`, `USED_UP` or `REVOKED`), paged with `WorkspaceInviteCursorInput` and
  `WorkspaceInviteCursorOutput`, never with their tokens; the new `revokeInvite(inviteId)`
  stops an invite admitting anyone, while members who joined with it stay ("Workspace invites"
  in `docs/messaging.md`).

### Changed

- **Upgrade:** usernames are stored lowercase and compared without regard to case. A signup
  stores `Alice` as `alice` and refuses `aLiCe` once `alice` exists (`CONFLICT`); `login`,
  `requestPasswordReset`, `requestAccountDeletion`, `from:@name` in message search and the
  `rim_server operator` commands accept a username in any case. A migration adds a unique index
  on the lowercased username and fails while two accounts have usernames that differ only in
  case: find them with `SELECT lower(username) FROM "user" GROUP BY 1 HAVING count(*) > 1` and
  rename one of each pair before upgrading.
- A confirmation of a command now lasts 60 seconds, at most 300, instead of five, so a person
  has time to read the confirmation dialog and answer it before it lapses. A proof is still good
  once, for the same caller, command, arguments and context ("Confirmation gate (H1)" in
  `docs/command_model.md`).
- Workspace members are no longer always delivered: a member's DM follows the recipient's choice
  for that workspace, `DIRECT` unless the workspace or the person set `REQUESTS`; Owners and
  Moderators are always delivered. With the defaults everything arrives as before.
- Creating a DM records the other person as the creator's contact (`contacts`), so their DM back
  is never a request.
- With `RIM_DM_INBOUND=partners` or `none`, `GET /s2s/v1/principals/<id>` answers `404` and key
  package claims answer `403` to servers that are not partners, except a server hosting a
  workspace the person subscribed to (`server-to-server-v1.md`, "The request inbox").

- **Upgrade:** password signups are refused by default. `RIM_SIGNUP_REQUIRE_KEYS` (default
  `true`) makes `signupWithInvite` answer `FORBIDDEN` in every registration mode, so new
  accounts sign up with `signupWithKey`; existing password accounts still sign in with
  `login`. Set it to `false` in `deploy/.env` while your users run a client that cannot sign up
  with a key.
- `signupWithKey` accepts an instance invite bound to an email address and gives the new account
  that address, refusing it like a password signup does when another account has it
  (`EMAIL_TAKEN`) or an operator blocked it (`VALIDATION`); before, it refused such an invite. A
  password signup with another address now answers "this invite admits only the email address
  it was issued for".
- `enrollIdentityKey` ends every other session of the account in the same transaction and
  closes their WebSocket connections; before, other password sessions stayed valid until they
  expired or were signed out.
- `key-identity-v1.md` and `device-linking-v1.md`: the web client may create and hold an
  identity, sign up with a key and link other devices; a browser-held identity is never backed
  up, the recovery kit is shown after signup or enrollment and its entropy is kept until the
  person confirms they saved it.
- **Upgrade:** `deploy/compose.yml` runs the published images `ghcr.io/rim-platform/rim-server`
  and `ghcr.io/rim-platform/rim-web` at `RIM_VERSION` (default `latest`) instead of building
  them. Set `RIM_VERSION` in `deploy/.env` to the release you run, and run `pull` then `up -d`
  where you ran `up -d --build`. Operators who build locally add `-f deploy/compose.dev.yml` to
  every compose command; `RIM_SERVER_IMAGE` and `RIM_WEB_IMAGE` still name other images.
- **Upgrade:** a GraphQL batch holds at most 20 requests; a larger one is answered 400 with
  `{"error":"batch_too_large"}` and none of it runs. `RIM_API_MAX_BATCH` changes the cap.

- With `link_previews` in `RIM_FEATURES_DISABLED`, sending or replying to a message no longer
  starts a link preview fetch at all, so the server makes no outbound request for the links
  people post.
- The packaged desktop app's origins (`tauri://localhost`, `http://tauri.localhost` and
  `https://tauri.localhost`) are accepted by every server without `RIM_CORS_ALLOWED_ORIGIN`.
- GraphQL `commands` and `commandDiscover` list only the commands the caller may run. A command
  the server runs needs a context workspace the caller is a member of, a role there that may run
  it and its capabilities: a non-member, or a call without a workspace, sees only the commands
  the client runs itself, and the moderation and owner commands are listed only to the roles
  that run them. Naming the direct space makes the caller a guest of it.
- Workspace-only features answer `VALIDATION` ("not available in the direct space") in the
  direct space instead of an empty result, for reads and writes alike: custom emojis, workspace
  plugins and plugin bundles (`POST /api/plugin-bundles/{server_id}` answers 400), keybinds
  scoped to a workspace, pipelines, workspace state and channels. The caller's own keybinds,
  asked without a workspace id, still answer.
- Typing is announced in direct conversations only. `startTyping` in a channel or a group
  answers `false` and emits no `TypingStarted`; it also answers `false` on a repeat inside the
  throttle window, and `true` only when it emitted the event.
- The dependency audit in CI and in the release gates is `cargo audit --deny unsound`: an
  unsoundness advisory fails it like a vulnerability. Each advisory it accepts is listed in
  `.cargo/audit.toml` with why it does not apply and when the entry comes out.

### Fixed

- The `TypingStarted` event of `workspaceEvents` carries the typing user's `username` and
  `display_name`, so a client can say who is typing.
- A local account's first use of the direct space, any operation that needs membership there,
  joins it as a guest instead of being refused.
- `executeCommand` refuses a builtin whose required role the caller does not hold in its
  workspace, with `FORBIDDEN` naming the command, before the service behind it runs; user command
  steps and pipe stages are checked the same way.
- On a server with terms of service, every signup path refuses an account that did not accept
  them, not only the `signupWithInvite` and `signupWithKey` resolvers.
- `addContact` refuses the caller's own principal, and `createConversation` with `remoteMembers`
  naming only the caller answers "cannot create DM with yourself" instead of the group message.
- `executeMacro` asks for one confirmation up front when a step needs one under the command
  confirmation gate: a dangerous step such as `admin.kick`, `admin.ban`, `admin.set_role` or
  `chat.remove_conversation_member`, a `Restricted` one, or for an agent a `Mutating` one. That
  confirmation covers every step of the run, so such a macro no longer fails at the step; the
  `message` of the `CONFIRMATION_REQUIRED` answer names those steps.

### Security

- **Upgrade:** `RIM_LIMIT_INVITES_PER_USER_PER_DAY` is on by default, at 50, and also caps the
  invite emails (`createInvite` with `recipientEmail`) one user sends in 24 hours. Emails are
  counted apart from the invites, so deleting a workspace does not give them back. Past either
  count `createInvite` is `VALIDATION` and creates nothing. Before, the cap was off unless set,
  and any workspace owner could have the server email any number of addresses. Set a higher
  number if your owners invite more people a day, or 0 to turn the cap off (`docs/email.md`).
- Accepting, ignoring or reporting a DM request whose expiry has passed is `NOT_FOUND`, as for
  an unknown request; the decision checks the expiry in the same update that records it.
  Before, an expired request not yet purged could still be accepted.
- A relay files a DM invite another server delivers only when that server speaks for the
  sender it names: the sender's card must be signed by their key, and their newest card must
  list the delivering server (or the sender is a local user subscribed there). Anything else is
  refused `invalid_invite` and reaches no inbox; a sender whose identity neither server can
  produce is taken as a stranger, a request at most. Before, a peer could name anyone,
  including the recipient's contacts or fellow workspace members, to skip the request inbox, or
  invent a new sender for each request to get past the daily limit. A delivering server now
  also files at most 1,000 DM requests per day on a relay, whichever senders it names
  (`docs/specs/server-to-server-v1.md`, "The request inbox").
- `rustls` 0.23.46: the TLS library behind outgoing HTTPS requests picks up the fixes for
  RUSTSEC-2026-0346, -0347, -0348 and -0349 (TLS secrets in `Debug` output, buffered data after
  an ECH rejection, certificate decompression memory, key-exchange downgrade).
- A member removed from a workspace (kicked, banned) or from a conversation (left, removed)
  loses their notifications for it, and the email digest leaves out anything its recipient can
  no longer read. Before, a kicked member's pending notification was still emailed in the next
  digest, carrying the message's current text, edits made after the kick included.
- A mutation whose protected field sits in nested fragments, named or inline, needs the CSRF
  header. Before, a request that showed only an exempt field such as `logout` at its top level
  skipped the check while a fragment two levels down ran `updateMyProfile`. The server reads
  the operation `operationName` selects through fragments at any depth, and requires the header
  whenever it cannot account for every root field. The refusal of account mutations for people
  from another server reads operations the same way.
- `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION` charges every mutation field a request runs: fields
  in nested fragments count, and a fragment spread twice counts twice. Before, fields more than
  one fragment deep were not charged.
- `/api/ws` runs subscriptions only. A query or a mutation sent on it answers one `VALIDATION`
  error, "only subscriptions over the WebSocket", and runs nothing. Before, a mutation sent
  there ran without the CSRF check, the mutation budget or the refusal of account mutations
  for people from another server. Clients send queries and mutations to `POST /api`.
- Link previews stop reading a page at 1 MiB as it arrives, where a page sent without a length
  used to be read whole before the cap applied. At most `RIM_LINK_PREVIEW_CONCURRENCY` fetches
  (default 8, 1 to 64) run at once; a fetch that waits five seconds for a slot is dropped.
- Link previews and server-to-server requests share one address policy that also refuses
  reserved IPv4 (`240.0.0.0/4`), benchmarking and protocol-assignment ranges, and IPv6
  addresses that embed a private IPv4 address through NAT64 (`64:ff9b::/96`) or 6to4
  (`2002::/16`). Server-to-server requests now ignore proxy environment variables, as link
  previews did. `docs/specs/server-to-server-v1.md` lists the refused ranges.
- Password hashing and verification (signups, password logins, password resets) run off the
  request workers, at most `RIM_PASSWORD_HASH_CONCURRENCY` at once (default: the number of
  CPUs, 1 to 64), so a burst of logins no longer stalls other requests. A request that waits
  over 10 seconds for a turn answers `UNAVAILABLE`.
- A WebSocket connection on `/api/ws` no longer outlives its session. When the session is
  signed out (`logout`, `logoutOtherSessions`), reset, revoked with its device, ended by an
  identity recovery, a requested account deletion, a block or the purge, or reaches its expiry,
  the server ends the connection's streams and closes it with code `4401` ("session ended").
  Every operation on a connection checks the session first. A session ended in another server
  process or from the command line closes its connections within
  `RIM_WS_SESSION_RECHECK_SECS` (default 30, 0 turns the recheck off). `docs/api.md`
  ("Subscriptions") describes it; a client that sees `4401` signs in again before reconnecting.
- A message on `/api/ws` is at most 256 KiB, whole or in fragments; a larger one closes the
  connection with code `1009` ("message too large"), refused at the frame header where it can
  be. Before, fragments were reassembled without a limit and queued for the server unbounded.
- Pipelines react only to events their creator may see: a message, edit, deletion or reaction
  in a private channel, DM or group the creator is not in (for a guest, any conversation they
  are not in) no longer fires their pipeline or counts towards its cooldown or suspension, and a
  creator who has left the workspace has no pipeline firing. Only the creator is told when a
  pipeline fires or is suspended, and `PipelineGql.lastFiredAt` reads null for everyone else.
  Before, any member could build a pipeline with a text or author filter and learn from its
  firing when a private conversation they were not in received a matching message.
- A password sign-in that finishes while the account's password is reset, a key is enrolled or
  an operator bans the account no longer leaves a working session behind: its session either
  ends with the others or is never created, the sign-in refused like a wrong password
  (`UNAUTHENTICATED`) after a reset or an enrollment and with `FORBIDDEN` after a ban. Before,
  a sign-in with the old password that was already being checked could create a session after
  the reset or enrollment had ended all the others.
- GraphQL a relay forwards for one of its users keeps this server's `RIM_API_MAX_BATCH` and
  `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION`, the mutation budget counted per device of the remote
  user, with the same refusals as `/api`. Before, a forwarded batch ran with no batch cap and
  no mutation budget.
- A workspace state key with non-ASCII characters, given to `setWorkspaceState` or to the
  `state.incr` command, is refused with a `VALIDATION` error. Before, a key with a non-ASCII
  character across its 64th byte made the request fail instead.
- A message's reactions, `reactedByMe` included, are read for the user who asked. Before, when
  several users' requests loaded reactions at the same moment, one of them could be shown
  another user's reaction state.
- A `workspaceEvents` subscription that falls behind and misses events checks the subscriber's
  membership and role again before it delivers the next event, and ends when they are no
  longer a member of the workspace. Before, a member whose removal or demotion to guest was
  among the missed events went on receiving the workspace's events as before.
- An identity recovered with the recovery phrase reaches other servers again. The identity
  document a relay serves and pushes (`GET /s2s/v1/principals/<id>`) leaves out the devices
  the recovery revoked, so hosts accept it, apply the recovery and revoke those devices; before,
  hosts refused the whole document and kept trusting the old devices, and the person could not
  be looked up or join a workspace on another server. A receiver ignores such a device instead
  of refusing the document, and a document past 64 devices keeps the current ones. Identity key
  rotations and recoveries are now pushed to hosts like device and card changes
  (`server-to-server-v1.md` "Profile cards and contacts", `key-identity-v1.md` "Rotation and
  recovery").
- A host applies an identity pushed for someone from another server (`POST /s2s/v1/identity`)
  only from a relay that speaks for them: one their profile card lists (or a newer pushed card
  lists), their home relay while they have no card, or one holding a delegation of one of
  their devices. Any other server gets `403 not_delegated` and nothing changes. Before, any
  peer could push a copy of someone's public identity and switch their devices' encryption
  activity on or off or become the relay the host refreshed them from. The relay a local user
  names, or one presenting a delegation or revocation for an unknown device, likewise only
  updates a known person when it speaks for them; refreshes go to the relays the person's card
  lists and then their home relay. `RIM_HOST_ALLOWED_RELAYS` now covers identity pushes too.
  `docs/specs/server-to-server-v1.md` ("Profile cards and contacts (7.4a)", "Remote users on a
  host", the threat model) states the rule.
- User commands have limits: at most 20 steps each, at most 100 per user, at most eight nested,
  and at most 100 built-in steps in one run, nested user commands and pipe stages included.
  `createUserCommand` and `updateUserCommand` refuse a command over a limit with a `VALIDATION`
  error naming it. A stored user command nested deeper than eight or expanding past 100 built-in
  steps is refused when it runs, before any step runs, and `commands` leaves out a user command
  that does not resolve instead of failing. A disabled user command no longer runs
  (`NOT_FOUND`). Before, a few small user commands naming each other many times made one lookup
  or run do work that grew exponentially ("User Commands And Macros" in
  `docs/command_model.md`).
- When a host applies a recovery or a device revocation that arrives in someone's identity
  (pushed by their relay or fetched from it), it ends their direct sign-in sessions and the
  delegations of the revoked devices, as it does for a revocation sent to
  `/s2s/v1/revocations`, and closes those sessions' live connections. Before, a device lost and
  recovered from kept its direct session at the host and stayed listed as delegated there. A
  direct sign-in of someone from another server is now refused (`UNAUTHENTICATED`) when their
  relay answers with an identity the host refuses; it still falls back to the stored copy when
  no relay answers (`server-to-server-v1.md` "Profile cards and contacts", "Direct
  connection").
- A relay bounds the items DM requests hold apart from each user's queue: at most 1 MiB per
  request and 16 MiB per user, the invites of ignored requests included, past which a new
  invite is refused `limited` and a later item is dropped. Held items no longer count toward
  `RIM_RELAY_QUEUE_BYTES_PER_USER`. Before, they counted toward it but could not be dropped,
  so requests from strangers could make the relay drop every item queued for that user's
  devices (`server-to-server-v1.md` "Relay queues", "The request inbox", "Limits and audit").
- A host keeps the challenges it issues for joining a workspace apart from login challenges.
  A join challenge is bound to the relay that asked for it and is accepted only by that relay's
  join; `loginWithKey`, `signupWithKey` and `enrollIdentityKey` refuse it, and the join refuses a
  login challenge. Before, a relay could present the proof a person's device signed for joining
  to the host's `loginWithKey` and get a session there as that person. Clients and relays change
  nothing: the proof keeps its format (`server-to-server-v1.md` §Joining, `key-identity-v1.md`).
- Workspace invites expire: `createInvite` without `expiresAt` gives an invite that expires in
  seven days, and `CreateInviteInput.expiresAt` must be in the future and at most 30 days away
  (`VALIDATION` otherwise), as for instance invites. Invites created before keep their expiry,
  or none; `revokeInvite` ends them. Redeeming a workspace invite now checks its revocation and
  expiry in the same update that counts the use, so an invite revoked or expiring while a join
  or signup runs admits nobody.
- The request inbox's limits hold for DMs started on this server. A conversation that would
  file a DM request past them is refused when it is started (`createConversation`,
  `addConversationMembers`): `RATE_LIMITED` with `retryAfterSeconds` once the sender would file
  more than `RIM_RELAY_DM_REQUESTS_PER_DAY` requests in 24 hours, `FORBIDDEN` ("their request
  inbox is full") while the recipient holds 100 pending requests. Before, a DM past either
  limit from someone on this server, or from someone elsewhere through the forward path,
  skipped the request inbox and reached the recipient's conversations and notifications
  directly. Workspace DMs a member takes as requests meet the same limits.
- A relay queues a host's items that name no conversation (workspace events, presence,
  directory events) only for recipients subscribed to that workspace, or, for presence and a
  recipient's own directory events from a host's direct space, for recipients who accepted a
  conversation that host homes; it refuses the rest `not_subscribed`. It also queues at most
  2 GiB of items from one host a day across all its users, items of subscribed workspaces not
  counted, and refuses that host's further items `limited` (metric limiter
  `relay_peer_bytes`). Before, any server could queue such items for every user of the relay,
  for any workspace, up to each user's queue cap, and keep them there for the retention period
  (`server-to-server-v1.md` "Outbox and deliver", "Limits and audit").

[Unreleased]: https://github.com/rim-platform/rim-server/commits/master
