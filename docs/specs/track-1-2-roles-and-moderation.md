# Track 1.2 — A real member role and basic moderation

_Spec written 2026-10-05 by the coordinator. Implements `ROADMAP.md` Track 1.2 ("Enough chat").
Assumes roadmap decision 6 as recommended: a four-role ladder, no permission bitfields. Status:
ready to implement after Track 1.1 merges (it builds on channel membership). Server first._

## Today

- `ServerMemberRole` is `Owner | Moderator | Guest`, and **every new member is created as
  `Guest`** (invite acceptance, join by invite, owner-added). "Guest" is therefore the ordinary
  member role in practice, and there is no restricted role at all.
- Only the author can delete a message (`DefaultMessageService::delete_message`, "v0").
- Owner can `removeServerMember` (kick); nothing stops the kicked user from re-joining with a
  valid invite. There is no ban, no timeout, no slow mode, no report, and no way to change a
  member's role through the API even though `change_member_role` exists on the repository.
- Moderation actions are not audited beyond the generic command audit when they go through a
  command.

## Goal

1. **Four roles with clear meaning**: `Owner` (everything), `Moderator` (moderation and channel
   management, no role changes above Member, no workspace deletion), `Member` (the default:
   read and write wherever they are a member, create channels, invite), `Guest` (restricted:
   read and write only in conversations they were explicitly added to; cannot create channels,
   browse or join channels, invite, or see the member directory).
2. **Moderation primitives** usable from GraphQL, the palette and macros: kick, ban (with
   reason, blocks re-join), timeout (mute a member for a duration), delete any message
   (Moderator+), per-channel slow mode, message reports to moderators.
3. **Every moderation action audited** in `command_audit_log` with a stable pseudo command id,
   regardless of whether it came through GraphQL or a command, in the same style as
   `workspace.set_feature`.

## Non-goals

- Per-channel permission overrides, custom roles, permission bitfields (revisit only if a real
  community asks).
- Ownership transfer and multiple owners' semantics beyond what exists.
- Automated moderation (word filters, anti-spam) — pipelines can do that later.
- Appeals and ban history UI beyond a list.

## Server (rim-server)

### Roles

- Enum becomes `Owner | Moderator | Member | Guest` (`as_str`: `owner`, `moderator`, `member`,
  `guest`). Postgres enum `server_member_role` gets `member` added. **Migration data step:**
  every existing `guest` row becomes `member` (today's guests are ordinary members). Document
  this in the migration and in `docs/messaging.md`.
- Defaults: invite acceptance, join by invite, owner-added and auto-join all create `Member`.
  Invites gain an optional `role: Member | Guest` (default Member) so an owner can mint a
  guest link; `Guest` invites cannot grant anything higher. Check the invite model under
  `rim_core` (the invite module name may differ; grep `InviteRepository`).
- New `setMemberRole(serverId, userId, role)`: Owner may set any role except on themselves;
  Moderator may set `Member` ↔ `Guest` only; nobody can create a second Owner here (out of
  scope). Emits a `MemberRoleChanged` workspace event (new variant; serialized shape:
  `workspaceId`, `userId`, `role`) and an audit row `admin.set_member_role`.
- Guest restrictions, enforced in the service layer (never only in the UI):
  - `create_conversation` for channels and groups: `PermissionDenied`.
  - `join_channel`, `add_conversation_members` (inviting), `createInvite`: `PermissionDenied`.
  - `list_channels` with `joinedOnly: false` returns only joined channels for a Guest (no
    browsing); `serverMembers` returns only members who share a conversation with the guest,
    or is denied, your call, but say which and test it.
  - Auto-join on workspace join is skipped for Guests.
  - Everything that a Member can do inside a conversation they are a member of stays allowed.
- `ServerMember` GraphQL type: `memberRole` becomes the enum `ServerMemberRole` instead of a
  string (SDL change: the client must regenerate; keep the field name).

### Moderation state

New tables (one migration, `sea migrate generate add_moderation`):
- `server_ban (server_id, user_id, reason text null, banned_by uuid, created_at, expires_at
  timestamptz null)`, primary key `(server_id, user_id)`. A banned user is removed from the
  workspace on ban; every join path (`accept_invite`, `join_server_by_invite`,
  `signup_with_invite`, owner-added, bootstrap excluded) refuses with `PermissionDenied` while
  a non-expired ban exists; `unban` deletes the row.
- `server_member_timeout (server_id, user_id, until timestamptz, reason text null, set_by
  uuid, created_at)`, primary key `(server_id, user_id)`. While `until > now()`, the user's
  writes fail with a new `ServiceError::TimedOut { until }` (GraphQL code `TIMED_OUT`, with an
  `until` extension): send, reply, edit, reactions, pins, topic, attachments, join, invite,
  create conversation, command execution of any Mutating command in that workspace (gate in
  `execute_command` next to the feature gate). Reads continue. Expired rows are ignored and
  can be cleaned opportunistically.
- `conversation.slow_mode_seconds int not null default 0`. When > 0, a non-moderator member's
  `send_message`/`reply_to_message` in that conversation is refused with
  `ServiceError::RateLimited { retry_after_seconds }` if their previous message there is
  younger than the interval (use the existing `RATE_LIMITED` code and `retryAfterSeconds`
  extension; look up the author's last `sent_at` in the conversation, no new in-memory state).
  Owner/Moderator are exempt.
- `message_report (id, server_id, conversation_id, message_id, reporter_user_id, reason text,
  status enum open|resolved|dismissed, resolved_by uuid null, resolved_at null, created_at)`.
  Any member who can read the message may report it once (unique on `(message_id,
  reporter_user_id)`); Moderators list open reports for the workspace (with the message text
  as it is now, membership-gated like any read) and resolve or dismiss them. Reporting a
  message in a private channel is visible only to moderators who are **also** members of that
  channel? No: keep it simple and honest, moderators see all reports but the message body is
  only included when the moderator can read the conversation; otherwise only ids and the
  reporter's reason. Document it.

### Message deletion by moderators

`delete_message`: author, or Owner/Moderator of the workspace. A moderator delete writes an
audit row `admin.delete_message` with the message id and author (text redacted). The
`MessageDeleted` event is unchanged.

### Service surface (`rim_services` + `rim_infra_services`)

New `ModerationService` (keep `ServerMemberService` for membership):
`kick(KickMemberCmd)` (Owner/Moderator; cannot target Owner; Moderator cannot target
Moderator), `ban(BanMemberCmd { reason, expires_at })` (same rules; kicks and records),
`unban`, `list_bans`, `timeout(TimeoutMemberCmd { until, reason })` (same rules; `until`
at most 30 days ahead), `clear_timeout`, `set_slow_mode(SetSlowModeCmd { conversation_id,
seconds })` (Owner/Moderator; channels only; 0..=21600), `report_message`,
`list_reports(status)`, `resolve_report(id, outcome)`. Every mutating method writes an audit
row via a shared helper (reuse the pattern in `rim_infra_services/src/workspace_feature.rs`,
pseudo ids `admin.kick`, `admin.ban`, `admin.unban`, `admin.timeout`, `admin.clear_timeout`,
`chat.set_slow_mode`, `chat.report_message`, `admin.resolve_report`, `admin.set_member_role`,
`admin.delete_message`).

Events: `MemberRemoved` on kick/ban if a workspace-level event exists for leaving (check
`MemberJoined`'s counterpart; add `MemberLeft { workspaceId, userId }` if missing),
`MemberRoleChanged`, `ConversationUpdated` on slow-mode change. New variants need the
visibility matrix in `subscription.rs` extended and the wire shape documented in
`docs/messaging.md`.

### Commands

Builtins (`admin.*` category, `Backend`): `admin.kick` (Mutating, dangerous), `admin.ban`
(Mutating, dangerous), `admin.unban` (Mutating), `admin.timeout` (Mutating), `admin.set_role`
(Mutating, dangerous), `chat.set_slow_mode` (Mutating), `chat.report_message` (Mutating; uses
the selected message from context), `chat.delete_message` already exists: its executor now
goes through the same service so moderators can use it. AI-callable: `chat.set_slow_mode`,
`admin.timeout`, `chat.report_message` with `nl_description`/`intent_samples`; kick, ban and
set_role stay `ai_callable: false` for now. Update the AI-callable pins and
`docs/command_model.md`.

### GraphQL

- `enum ServerMemberRole { OWNER MODERATOR MEMBER GUEST }`; `ServerMember.memberRole:
  ServerMemberRole!`; `CreateInviteInput.role`.
- Mutations: `setMemberRole`, `kickMember(serverId, userId, reason)`, `banMember(serverId,
  userId, reason, expiresAt)`, `unbanMember`, `timeoutMember(serverId, userId, until, reason)`,
  `clearTimeout(serverId, userId)`, `setSlowMode(serverId, conversationId, seconds)`,
  `reportMessage(serverId, conversationId, messageId, reason)`, `resolveReport(serverId,
  reportId, outcome: RESOLVED | DISMISSED)`.
- Queries: `serverBans(serverId)`, `memberTimeout(serverId, userId)` (or expose
  `timeoutUntil` on `ServerMember`, preferred), `messageReports(serverId, status, dir, cursor,
  limit)` with the standard `(created_at, id)` pagination, `Conversation.slowModeSeconds`.
- Error codes: `TIMED_OUT` (new), `RATE_LIMITED` (existing, slow mode), `FORBIDDEN`,
  `VALIDATION`, `NOT_FOUND`.

### Tests (server)

- Unit: role matrix for every moderation method (who can target whom), guest restrictions,
  slow-mode arithmetic and exemption, timeout gate in message and command paths, report
  uniqueness, audit row written for each action.
- `db-tests`: ban blocks re-join at the repository/service level, timeout expiry, role
  migration data step (`guest` → `member`).
- `e2e-tests`: a moderator kicks and bans a member who then cannot accept a new invite;
  a timed-out member gets `TIMED_OUT` on `sendMessage` and on `executeCommand` of a Mutating
  command; slow mode returns `RATE_LIMITED` with `retryAfterSeconds` and the moderator is
  exempt; a guest cannot create a channel, join, invite or browse; a moderator deletes someone
  else's message and the audit row exists; report, list and resolve; `setMemberRole` matrix
  through GraphQL; `memberRole` is the enum in the SDL.
- Docs: `docs/messaging.md` (roles table, moderation), `docs/capability_security_model.md`
  (gate order now includes timeout), `docs/command_model.md`, `docs/architecture.md`.

## Client (rim-ui)

- Codegen; `memberRole` is now an enum everywhere (fix the string comparisons; grep for
  `"owner"` / `"moderator"`).
- Member directory and the conversation members panel: role badge; a per-member menu with
  Change role (Owner; Moderator for Member/Guest), Timeout (duration picker: 5m, 1h, 1d,
  custom), Kick, Ban (reason, optional expiry), each with a confirmation dialog in the
  dangerous-action style.
- Settings → Workspace: a **Moderation** section (Owner/Moderator): bans list with Unban,
  open reports list with the message (when readable), Resolve/Dismiss; invites gain a role
  selector (Member/Guest).
- Conversation header menu: Slow mode (Owner/Moderator) with a picker; a small slow-mode
  indicator near the composer with a countdown after you send; `RATE_LIMITED` from slow mode
  shows "Slow mode: wait Ns".
- Message context menu: Report (any member; reason dialog), Delete for moderators on others'
  messages.
- Timed-out state: a banner above the composer "You are timed out until …", composer
  disabled; `TIMED_OUT` from a command shows the same message.
- Guest state: hide Browse channels, New channel, Invite, and the member directory for guests
  (the server enforces anyway).
- Realtime: handle `MemberRoleChanged` (refetch members and own role), `MemberLeft` for self
  (navigate out with a toast "You were removed from this workspace").
- Tests for each surface's role gating and mutation variables, the timed-out banner, the
  slow-mode countdown, report flow.

## Definition of done

Server: `cargo fmt`, both clippy lines, `cargo test --workspace`, `cargo test -p migration`,
`db-tests`, `infra-db-tests`, `e2e-tests` single-threaded. Client: lint (0 errors), format,
tsc, vitest, build. Two PRs, server first.
