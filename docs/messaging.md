# Messaging Contract

This document describes the current v0 backend contract for conversations, messages, threads, and attachments.

## Product Terms

- `workspace` is the product-level term.
- `Conversation` is the messaging container used for channels, DMs, and group chats.
- `MessageThread` is an optional thread opened from a timeline message.

## Conversation Types

- `Channel`: stable shared conversation inside a server/workspace, public or private.
- `Direct`: 1:1 conversation between two users.
- `Group`: multi-user private conversation.

## Channel Visibility And Membership (Track 1.1)

- **Public channels** (`isPrivate = false`) are readable by every workspace member, joined or not
  (except guests, see Roles And Moderation below).
  Joining (`joinChannel`) adds a `conversation_member` row; joined channels are what the sidebar
  lists (`channels(filter: { joinedOnly: true })`), all public channels appear in browse
  (`channels` with the default filter). Posting does not require joining.
- **Private channels** (`isPrivate = true`) are readable, listable, searchable and subscribable by
  their members only. A workspace member outside one gets `NOT_FOUND` on every read and write —
  never `FORBIDDEN` — so the response does not reveal that the channel exists. This holds for
  workspace owners and moderators too.
- Privacy is chosen at creation (`createConversation(input: { isChannel: true, isPrivate: true })`)
  and **cannot be changed later** in either direction. `isPrivate` on a DM or group is rejected
  with `VALIDATION`.
- The creator of any channel is its first member. A private channel's initial members are added
  by the client with `addConversationMembers` right after creation.
- `addConversationMembers` (invite): the caller must be a member of the channel or group (for a
  public channel: join first, `FORBIDDEN` otherwise); targets must be workspace members;
  existing members are skipped. Not valid for DMs.
- `leaveConversation`: channels and groups, not DMs. Leaving a public channel you never joined
  is a no-op. Leaving a private channel as its last member is allowed; it then stays invisible to
  everyone, and only an Owner/Moderator can archive it, by id (from the audit log).
- `removeConversationMember`: Owner/Moderator may remove anyone (also from a private channel
  they are not in); everyone else only themselves.
- DMs and groups are unchanged: non-members get `FORBIDDEN`.
- Mentions (`<@user>`) are stored and notified (`UserMentioned`) only for users who can read the
  conversation: any workspace member for a public channel, members otherwise.
- Realtime: events of a private channel, DM or group reach only its members; public-channel
  events reach the whole workspace. The visibility fields on `WorkspaceEvent` (`is_private`,
  `member_user_ids`, and `conversation_type` where it was not already serialized) are
  server-side only and never serialized, so the WebSocket JSON is unchanged.
- Existing workspace members became members of every existing channel when the Track 1.1
  migration ran, so the joined list did not start empty. A user who joins the workspace later
  (invite acceptance, join by invite, owner adding them, server creation — all through
  `util::add_workspace_member`) is joined to every public, unarchived channel at that moment;
  private and archived channels are not joined.
- Unread counts (`unreadCounts`) and `markAllAsRead` cover only conversations the user is a
  member of: joined public channels, private channels, DMs and groups. A public channel the user
  can read but has not joined carries no unread badge; after joining, messages count from the
  join time.

## Archived Channels

- Owner/Moderator archive and unarchive channels (`setConversationArchived`); other roles get
  `FORBIDDEN`. Only channels can be archived (`VALIDATION` otherwise).
- An archived conversation is read-only: sending, replying, editing, deleting and forwarding into
  it, reactions, pins, topic edits, threads, attachments, typing, joining and inviting fail with
  `CONVERSATION_ARCHIVED`. Leaving and removing members still work.
- Archived channels are hidden from `channels` unless `includeArchived: true`, and stay readable
  and searchable by whoever could read them before.

## Roles And Moderation (Track 1.2)

### Roles

| Role | Meaning |
|---|---|
| `OWNER` | Everything. One per workspace; the role cannot be granted (`setMemberRole` with `OWNER` is `VALIDATION`). |
| `MODERATOR` | Moderation (kick, ban, timeout, delete any message, slow mode, reports) and channel management (archive, remove members). Changes roles only between `MEMBER` and `GUEST`. |
| `MEMBER` | The default. Reads and writes wherever they are a member (public channels without joining), creates channels and groups, joins public channels, invites people into conversations. |
| `GUEST` | Restricted. Reads and writes only in conversations they were explicitly added to. |

- **Migration:** until Track 1.2 every new member was created as `guest`, so "guest" meant an
  ordinary member. The `m20261005_024730_add_member_role` migration adds `member` to the
  `server_member_role` enum and turns **every existing `guest` into `member`**; the column
  default is now `member`.
- New members get `MEMBER` on every join path (invite acceptance, join by invite, owner-added).
  Invites carry a role (`CreateInviteInput.role`, `MEMBER` by default, `GUEST` for a guest link;
  any other value is `VALIDATION`). Workspace invites (`createInvite`) stay owner-only.
- `ServerMember.memberRole` is the enum `ServerMemberRoleGql` (it was a string); the field name is
  unchanged. `ServerMember.timeoutUntil` is set while the member is timed out.
- `setMemberRole(serverId, userId, role)`: the Owner may set `MODERATOR`, `MEMBER` or `GUEST` on
  anyone but themselves; a Moderator may move a `MEMBER` to `GUEST` and back; everyone else gets
  `FORBIDDEN`. Changing your own role is `VALIDATION`. Emits `MemberRoleChanged`.

### Guest restrictions (enforced in the services)

- Creating a channel or a group, joining a channel, inviting into a conversation
  (`addConversationMembers`) and creating workspace invites: `FORBIDDEN`. Guests may open DMs.
- A public channel the guest has not been added to behaves like a private channel: `NOT_FOUND`
  on every read and write, absent from `channels` (a guest's channel list is always the joined
  list, whatever `joinedOnly` says), and from `searchMessages`, and its realtime
  events do not reach the guest.
- `serverMembers` for a guest lists only themselves and the members of conversations they share;
  `ServerMember` lookups of anyone else are `NOT_FOUND`. The directory events `MemberJoined`,
  `MemberLeft` and `MemberRoleChanged` reach a guest only when they are about the guest.
- Guests are not auto-joined to public channels when they join the workspace.
- Inside a conversation they belong to, a guest can do everything a member can.

### Kick, ban, timeout

Who may target whom (kick, ban, timeout, clear timeout): the actor must be Owner or Moderator;
nobody can target the Owner; a Moderator cannot target another Moderator; nobody targets
themselves (`VALIDATION`). Otherwise `FORBIDDEN`.

- `kickMember`: removes the member from the workspace and from every conversation of it, emits
  `MemberLeft`. They can re-join with a new invite.
- `banMember(reason, expiresAt)`: records a ban (`server_ban`), then removes the member like a
  kick. A user who is not a member can be banned pre-emptively. While a ban is active (no
  `expiresAt`, or `expiresAt` in the future) accepting an invite, joining by invite and being
  added by the owner fail with `FORBIDDEN`; the refused join consumes no invite use.
  `unbanMember` deletes the ban; `serverBans` lists active bans (Owner/Moderator).
- `timeoutMember(until, reason)`: `until` must be in the future and at most 30 days ahead. While
  it lasts, every write fails with `TIMED_OUT` (extension `until`, RFC 3339): sending, replying,
  editing, forwarding, reactions, pins, topic, attachments (REST upload: 403 with `until`),
  joining, inviting, creating conversations, and `executeCommand` of
  any `Mutating` (or stricter) command in that workspace. Reads continue, and a timed-out author
  may still delete their own messages over GraphQL. Expired timeouts are ignored;
  `clearTimeout` ends one early.

### Slow mode

- `setSlowMode(serverId, conversationId, seconds)`: Owner/Moderator, channels only, `0..=21600`
  (`0` turns it off), not on archived channels. Shown as `Conversation.slowModeSeconds`; changes
  emit `ConversationUpdated`.
- While it is on, a member's `sendMessage`, `replyToMessage` or forward into the channel fails
  with `RATE_LIMITED` (extension `retryAfterSeconds`, rounded up) if their previous message there
  is younger than the interval. Owner and Moderators are exempt. The check reads the author's
  last `sent_at` in the conversation; no in-memory state.

### Deleting messages

`deleteMessage`: the author, or an Owner/Moderator of the workspace who can read the
conversation (a moderator outside a private channel gets `NOT_FOUND`, like any read). A
moderator delete writes an `admin.delete_message` audit row with the message id and author (no
text). `MessageDeleted` is unchanged.

### Reports

- `reportMessage(serverId, conversationId, messageId, reason)`: any member who can read the
  message, once per message (`CONFLICT` on a second report); deleted messages cannot be
  reported. Allowed while timed out.
- `messageReports(serverId, status, dir, cursor, limit)`: Owner/Moderator, ordered newest first
  (`(createdAt, id)` descending; `AFTER` walks toward older reports) with the standard cursor
  contract (`docs/pagination.md`). Moderators
  see every report of the workspace, but the reported message's current text and author
  (`messageText`, `messageAuthorId`) are included only when the moderator can read the
  conversation (`messageReadable`); for a private channel they are not in, only the ids and the
  reporter's reason are shown.
- `resolveReport(serverId, reportId, outcome: RESOLVED | DISMISSED)`: Owner/Moderator; a report
  that is no longer open is `CONFLICT`.

### Audit

Every moderation action writes one `command_audit_log` row (initiator `moderation`) through one
shared helper, whether it came from GraphQL or a command: `admin.set_member_role`, `admin.kick`,
`admin.ban`, `admin.unban`, `admin.timeout`, `admin.clear_timeout`, `chat.set_slow_mode`,
`chat.report_message`, `admin.resolve_report`, `admin.delete_message`. Arguments hold ids and
flags only, never reasons or message text. An action run as a command also gets the generic
command-execution row. Refused actions are not audited by the helper.

### Realtime events

Two new `WorkspaceEvent` variants (snake_case fields, like every other variant):

- `{"type": "MemberLeft", "workspace_id": "…", "user_id": "…"}` — a member was kicked or banned.
  Delivered to the workspace (guests: only about themselves), including the removed user, whose
  subscription then ends.
- `{"type": "MemberRoleChanged", "workspace_id": "…", "user_id": "…", "role": "member"}` — `role`
  is `owner`, `moderator`, `member` or `guest`. Same visibility as `MemberLeft`. A subscriber whose
  own role changes to or from `guest` gets the matching visibility from the next event on.

## Notification Rules (Track 1.3)

The server decides, once per message, who is told about it.
For every recipient it stores a notification (the inbox, the bell badge and the email digest) and
sends that user a private `NotificationDue` event; clients only render what the server decided.
Sound and preview stay device-local client settings.

### Settings

- **Per-conversation level** (`setConversationNotificationLevel`, `Conversation.notificationLevel`):
  `ALL`, `MENTIONS` or `NONE`; null inherits the workspace default.
  `Conversation.effectiveNotificationLevel` is what applies now: `NONE` while a timed mute is in
  effect, else the conversation level, else the workspace default.
- **Workspace settings** (`myNotificationSettings`, `updateNotificationSettings`), per user and
  workspace: `defaultLevel` (default `ALL`), `keywords` (at most 20, each 2 to 40 characters after
  trimming, stored lower-cased and deduplicated), a daily do-not-disturb window `dnd`
  (`start`/`end` as `"HH:MM"` in an IANA `timezone`; crosses midnight when `end < start`; `start`
  must differ from `end`), and `digest` (`OFF`, `HOURLY`, `DAILY`; `docs/email.md`).
- **Do-not-disturb pause** (`setDoNotDisturb`, command `notifications.dnd`): on until turned off,
  or until an instant (`dndPaused`, `dndPausedUntil`). It is separate from the daily window and
  `updateNotificationSettings` leaves it alone.
- **Mutes** keep working: `muteConversation` with `mutedUntil` is a timed `NONE` (it clears a
  `NONE` level so the mute really ends; another level stays); without `mutedUntil` it sets level
  `NONE`. `unmuteConversation` clears `mutedUntil` and a `NONE` level. `isMuted` is true for a
  timed mute in effect or level `NONE`; `mutedConversations` lists rows with level `NONE` or any
  `mutedUntil`.

### The decision, per member except the author

1. Candidates are the conversation's members (the list its events go to): a private channel
   never notifies a non-member, and a public-channel reader who has not joined is not notified.
   A timeout does not affect receiving.
2. A timed mute in effect yields nothing.
3. Effective level = conversation level, else workspace default. `NONE` yields nothing, direct
   messages and mentions included.
4. Reason, strongest first: `direct` (a Direct conversation), `mention` (`<@user>` naming the
   member, or `@channel` / `@here` as a whole word), `keyword` (a keyword as a whole word,
   case-insensitive, outside `<@…>` tokens), `thread_reply` (a reply in a thread whose root the
   member wrote or where they replied), else `all`.
5. `MENTIONS` admits every reason but `all`; `ALL` admits everything.
6. Inside the do-not-disturb window (in the user's timezone) or pause the notification is still
   stored, but the event carries `quiet: true` so clients neither toast nor play a sound.

One message yields at most one notification per user (unique `(user, message)`). Notifications
are created after the message commits (`sendMessage`, `replyToMessage`, and forwards into the
target conversation) and best-effort: a failure is logged and never fails the send. Edits do not
notify again. Deleting a message deletes its unread notifications (read ones stay).
`UserMentioned` is still emitted for the mentions view.

### Inbox

- `myNotifications(serverId, unreadOnly, dir, cursor, limit)`: newest first (`createdAt DESC,
  id DESC`, standard cursors; `AFTER` goes to older ones). Each item has `reason`, `createdAt`,
  `readAt` and nested `conversation` / `message`, which are null once the viewer can no longer
  read the conversation.
- `myUnreadNotificationCount(serverId)`; `markNotificationsRead(serverId, ids | allBefore)`
  (exactly one; returns how many changed; ids of other users are ignored).
- `markAsRead` on a conversation also marks its notifications for messages sent up to `readAt`
  read, in the same transaction; `markAllAsRead` marks every notification of the workspace read.
- Notifications older than 90 days are deleted hourly.

### `NotificationDue` event

```json
{"type": "NotificationDue", "workspace_id": "…", "user_id": "…", "notification_id": "…",
 "conversation_id": "…", "message_id": "…", "reason": "mention", "quiet": false}
```

`reason` is `direct`, `mention`, `keyword`, `thread_reply` or `all`. The event is delivered only
to `user_id` (the server-side scope list is `[user_id]`), guests included; it is a new variant and
no existing event changed.

## Titles

- `Channel` and `Group` conversations may have an optional `title`.
- Empty or whitespace-only titles are normalized to `None`.
- `Direct` conversations cannot have titles.

## Timeline And Threads

- Every message belongs to a `conversation_id`.
- A timeline message has `thread_id = None`.
- A thread reply has `thread_id = Some(thread_id)`.
- Creating a thread from a timeline message is optional.
- The root message remains in the main timeline after a thread is created.
- Replies inside the thread live only in the thread view; they do not appear in the main timeline listing.

## Reply Semantics

- Timeline replies use `parent_message_id` and still stay in the main timeline.
- Thread replies may point to:
  - the root timeline message for the thread, or
  - another message already inside the same thread.
- A message from another conversation or another thread is rejected as a reply parent.

## `thread_id` vs `root_thread_id`

- `thread_id`
  - `None` for main timeline messages
  - `Some(thread_id)` for thread replies
- `root_thread_id`
  - `Some(thread_id)` only for a timeline message that is the root of an existing thread
  - `None` for ordinary timeline messages
  - `None` for thread replies

This lets clients render the timeline normally while also knowing whether a timeline message can open an existing thread.

## Attachments In v0

- Message send APIs still reject inline attachment refs in v0.
- `has_attachments` is still part of the message shape because attachment rows already exist in the data model and may be read for existing data.
- Attachment reads require both:
  - server membership, and
  - membership in the conversation that owns the attachment's message.
- Attachment create/finalize/delete endpoints now support a minimal lifecycle for existing messages:
  - `createAttachment` requires `message_id`
  - the actor must be the author of that message
  - create returns a temporary upload ticket and stores the attachment as pending
  - finalize marks the attachment as ready
  - delete removes the attachment and refreshes the message `has_attachments` flag
- Upload-ticket validation and pre-message attachment staging are not implemented yet.

## Pagination

- Timeline messages: `ORDER BY sent_at DESC, id DESC`
- Thread messages: `ORDER BY sent_at DESC, id DESC`
- Inbox conversations: `ORDER BY last_activity_at DESC, id DESC`
- Channels: `ORDER BY created_at ASC, id ASC` (filtered by `ChannelListFilter`: `joinedOnly`,
  `includeArchived`; both default to `false`)

Cursor pagination uses overfetch (`limit + 1`) so `next` and `prev` reflect actual additional data, not page-size guesses.

## Permalinks

- A timeline message's permalink is
  `/workspaces/{server}/conversations/{conversation}/messages/{message}`.
- A thread reply's permalink is
  `/workspaces/{server}/conversations/{conversation}/threads/{thread}/messages/{message}`.
- The email digest links each message by its permalink.
