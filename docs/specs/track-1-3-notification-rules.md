# Track 1.3 — Notification rules

_Spec written 2026-10-05 by the coordinator. Implements `ROADMAP.md` Track 1.3 ("Enough chat").
Status: ready to implement after Track 1.2 merges (reuses the timeout/role plumbing only
lightly; the real dependency is Track 1.1's membership model, already merged). Server first._

## Today

- The server knows one notification preference: `muted_until` per `(user, conversation)`.
- Every notification decision is made in the client (`useNotificationEventHandler.ts`): on a
  `NewMessage` event it shows a desktop notification unless the conversation is muted, with
  three device-local toggles (enabled, sound, preview) in `localStorage`. Mentions carry a
  `UserMentioned` event. There are no keyword alerts, no do-not-disturb, no per-conversation
  levels, no notification history, and no email digest. Push does not exist yet (Track 5.1).
- Because the decision is client-side, two devices can disagree, a web tab with no permission
  shows nothing, and the future push and digest paths would have to reimplement the rules.

## Goal

1. **One server-side decision** per message: for every member who should be told, the server
   records a notification and sends that user a private `NotificationDue` event. Clients only
   render what the server decided (desktop toast now, push and email later from the same
   record). Privacy follows membership automatically (Track 1.1 gates run first).
2. **Rules a user can set**: per-conversation level (`ALL`, `MENTIONS`, `NONE`), a workspace
   default level, keyword alerts, a do-not-disturb window, and an email digest preference.
3. **A notification inbox**: unread badge, list, mark read. This is also what the digest sends.

## Non-goals

- Web push and mobile (Track 5.1 consumes `NotificationDue` and the inbox; nothing here must
  block it).
- Per-thread follow/unfollow (thread replies notify the root author and participants as
  "mentions-level" by default; refine later).
- Sound and preview settings stay device-local in the client.

## Server (rim-server)

### Model and schema (one migration, `sea migrate generate add_notification_rules`)

- `notification_preference` gains `level` enum (`all | mentions | none`, nullable: null means
  "inherit the workspace default") and keeps `muted_until` (an explicit `NONE` with an expiry;
  `muteConversation` keeps working by writing `muted_until`).
- New `user_notification_settings (user_id, server_id, default_level enum not null default
  'all', keywords text[] not null default '{}', dnd_start time null, dnd_end time null,
  dnd_timezone text null, digest enum (off|hourly|daily) not null default 'off', updated_at)`,
  primary key `(user_id, server_id)`. Keywords: at most 20, each 2..=40 chars, case-insensitive
  whole-word match.
- New `user_notification (id uuid, user_id, server_id, conversation_id, message_id, reason enum
  (direct|mention|keyword|all|thread_reply), created_at, read_at null, emailed_at null)`, index
  `(user_id, read_at, created_at desc)`, and a unique `(user_id, message_id)` so one message
  yields at most one row per user (first reason wins by priority: direct > mention > keyword >
  thread_reply > all).
- Entity merge by hand; mappers; retention: rows older than 90 days are deleted by the same
  opportunistic cleanup hook the audit retention will use (if none exists yet, add a small
  periodic task in `rim_server/src/app.rs` next to the escalation timer, interval 1h).

### Decision engine (`rim_infra_services/src/notification/`)

A pure function plus a thin async wrapper:

```
decide(message, conversation, members, prefs_by_user, settings_by_user, now) -> Vec<(UserId, Reason)>
```

Rules, evaluated per member except the author:
1. Timed-out, banned or non-member users never appear (membership already filtered; timeout
   is irrelevant to receiving).
2. `muted_until > now` → nothing.
3. Effective level = conversation level, else workspace default. `NONE` → nothing, except a
   direct mention still notifies in a DM? No: `NONE` means none; document it.
4. Reason resolution: DM → `direct`; `@user` or `@channel` mention → `mention`; keyword hit in
   the message text → `keyword`; reply in a thread the user authored or replied in →
   `thread_reply`; otherwise `all`.
5. Level `MENTIONS` admits `direct`, `mention`, `keyword`, `thread_reply`; level `ALL` admits
   everything.
6. Do-not-disturb: if `now` falls inside the user's window (in their `dnd_timezone`, window may
   cross midnight), the notification is still **recorded** (inbox and digest) but the
   `NotificationDue` event carries `quiet: true` so clients do not toast or sound. Push (later)
   will respect the same flag.

Unit-test the pure function exhaustively (a table-driven matrix is fine).

Wiring: `DefaultMessageService::send_message` and `reply_to_message` already load the member
list for events; after the message commits, call `NotificationService::notify_for_message`
(best-effort, like mentions today: log on failure, never fail the send). It inserts the rows
and emits one `WorkspaceEvent::NotificationDue { workspace_id, user_id, notification_id,
conversation_id, message_id, reason, quiet }` per recipient with `member_user_ids = [user_id]`
so `user_can_see_event` delivers it to that user only. The existing `UserMentioned` event stays
(clients still use it for the mentions view).

Edits and deletes: deleting a message deletes its unread notification rows (keep read ones).

### Service surface

`NotificationService` (new trait in `rim_services`):
- `get_settings(actor, server_id)`, `update_settings(UpdateNotificationSettingsCmd {
  default_level, keywords, dnd: Option<{start, end, timezone}>, digest })` with validation
  (timezone must be a valid IANA name as `user.timezone` already validates; keywords limits).
- `set_conversation_level(SetConversationNotificationLevelCmd { conversation_id, level:
  Option<Level> })` (membership-gated; `None` clears the override).
- `list_notifications(ListNotificationsQuery { unread_only, dir, cursor, limit })` with the
  standard `(created_at, id)` pagination; `unread_count(server_id)`.
- `mark_read(MarkNotificationsReadCmd { ids: Vec<Id> | all_before: DateTime })`.
- `notify_for_message(...)` (internal, called by the message service; not exposed).
- Reading a conversation (`markAsRead` on it) also marks that conversation's notifications read
  (same transaction as the read-state write).

### Email digest

- A periodic job (interval 15 min, in `app.rs` next to the escalation timer, behind the same
  "is this node the scheduler" assumption the escalation timer makes) selects users with
  `digest != off` who have unread, un-emailed notifications older than the digest interval
  (hourly: 60 min, daily: 24 h since their last digest) and who have had **no presence
  heartbeat in the last 15 minutes**. It sends one email per user per workspace with a new
  template `notification_digest` (count, grouped by conversation, first 80 chars of each text
  with HTML escaped, deep links), then sets `emailed_at`. Uses the existing `EmailSender`; the
  log sender logs recipient and subject only, as today.
- Users without an email (not possible today, email is NOT NULL) or with digest `off` are
  skipped. Test the selection query under `db-tests` and the template rendering as a unit test.

### Commands

`notifications.set_level` (args: conversation from context, level; Mutating, AI-callable, with
intent samples like "mute this channel", "only mentions here"), `notifications.dnd` (args:
`on|off|until`; Mutating, AI-callable), `notifications.mark_all_read` (Mutating, AI-callable).
Update `docs/command_model.md` and the AI-callable pins.

### GraphQL

- Enums `NotificationLevel { ALL MENTIONS NONE }`, `NotificationReason`, `DigestFrequency`.
- `Conversation.notificationLevel: NotificationLevel` (the override, nullable) and
  `Conversation.effectiveNotificationLevel: NotificationLevel!`.
- Query `myNotificationSettings(serverId)`, `myNotifications(serverId, unreadOnly, dir,
  cursor, limit): NotificationPage` (items include `conversation`, `message` via nested
  resolvers, `reason`, `readAt`, `createdAt`), `myUnreadNotificationCount(serverId)`.
- Mutations `updateNotificationSettings(serverId, input)`,
  `setConversationNotificationLevel(serverId, conversationId, level)`,
  `markNotificationsRead(serverId, ids: [ID!], allBefore: DateTime)`.
- Subscription: the new `NotificationDue` event (serialized shape documented in
  `docs/messaging.md`; this is a new variant, no existing field changes).
- `muteConversation` / `unmuteConversation` keep their shape.

### Tests (server)

- Unit: the decision matrix (levels × reasons × mute × DND × keywords, author excluded,
  non-member excluded), settings validation, mark-read semantics, digest selection predicate
  (pure part), template.
- `db-tests`: notification repo paging and unique `(user, message)`; digest candidate query.
- `e2e-tests`: alice sets `MENTIONS` on a channel, bob's plain message creates no notification
  and his `@alice` does; a keyword hit; DM always `direct`; `NONE` yields nothing; DND marks
  `quiet` (unit-level on the event; e2e can check the stored row only); `myNotifications`
  paging and `markNotificationsRead`; reading the conversation clears them; a private channel
  message never creates a notification for a non-member (reuse the private-channel e2e
  fixtures).
- Docs: `docs/messaging.md` (rules, event shape), `docs/email.md` (digest), `docs/command_model.md`.

## Client (rim-ui)

- Codegen. `useNotificationEventHandler` switches from deciding on `NewMessage` to rendering
  `NotificationDue` (toast + sound unless `quiet`; device-local `enabled`/`sound`/`preview`
  toggles still apply). Keep the old path behind nothing: remove it.
- Settings → Notifications becomes server-backed per workspace: default level, keyword chips
  editor (validation mirrors the server), do-not-disturb window with timezone (default the
  user's `timezone`), digest frequency; the three device-local toggles stay in a "This device"
  group.
- Conversation header menu: Notification level submenu (All, Mentions, None, Use workspace
  default) replacing the plain Mute toggle; mute for a duration stays as a quick action.
- A bell in the side rail with the unread notification count; a notifications panel in the
  tool panel listing `myNotifications` (reason icon, conversation, snippet, time), click
  navigates to the message and marks it read; "Mark all read".
- Sidebar unread styling can use `effectiveNotificationLevel` to dim `NONE` conversations.
- Tests: event rendering (quiet vs loud), settings form variables and validation, header
  submenu variables, bell count and panel actions, navigation on click.

## Definition of done

Server: `cargo fmt`, both clippy lines, `cargo test --workspace`, `cargo test -p migration`,
`db-tests`, `infra-db-tests`, `e2e-tests` single-threaded. Client: lint (0 errors), format,
tsc, vitest, build. Two PRs, server first.
