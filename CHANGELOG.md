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

### Changed

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

### Fixed

- A local account's first use of the direct space, any operation that needs membership there,
  joins it as a guest instead of being refused.

### Security

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

[Unreleased]: https://github.com/rim-platform/rim-server/commits/master
