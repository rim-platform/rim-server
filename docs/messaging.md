# Messaging Contract

This document describes the backend contract for conversations, messages, threads, and attachments.

## Product Terms

- `workspace` is the product-level term.
- `Conversation` is the messaging container used for channels, DMs, and group chats.
- `MessageThread` is an optional thread opened from a timeline message.

## Conversation Types

- `Channel`: stable shared conversation inside a server/workspace, public or private.
- `Direct`: 1:1 conversation between two users.
- `Group`: multi-user private conversation.

## Channel Visibility And Membership

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
  conversation: any workspace member but a guest for a public channel, members otherwise. A
  message mentions at most 50 distinct users; later `<@user>` tokens stay plain text, are not
  stored and notify nobody. An edit replaces the message's mentions.
- Realtime: events of a private channel, DM or group reach only its members; public-channel
  events reach the whole workspace. The visibility fields on `WorkspaceEvent` (`is_private`,
  `member_user_ids`, and `conversation_type` where it was not already serialized) are
  server-side only and never serialized, so the WebSocket JSON is unchanged.
- Existing workspace members became members of every existing channel when the
  `m20261005_005743_add_channel_privacy_and_archive` migration ran, so the joined list did not start empty. A user who joins the workspace later
  (invite acceptance, join by invite, owner adding them, server creation — all through
  `util::add_workspace_member`) is joined to every public, unarchived channel at that moment;
  private and archived channels are not joined.
- Unread counts (`unreadCounts`) and `markAllAsRead` cover only conversations the user is a
  member of: joined public channels, private channels, DMs and groups. A public channel the user
  can read but has not joined carries no unread badge; after joining, messages count from the
  join time.

## Who May Start A DM

A first DM from someone arrives directly, as a request the recipient accepts, ignores, blocks or
reports (`dmRequests`, see "The request inbox" in `docs/relay.md`), or not at all. Three layers
decide, and each may only tighten the one above it:

| Layer | Set by | People outside the recipient's workspaces | Fellow members of a workspace |
|---|---|---|---|
| Server | The operator: `RIM_DM_INBOUND`, `RIM_DM_OUTBOUND`, `RIM_DM_PARTNERS` (read-only in `serverInfo.dmInbound`, `dmOutbound`, `dmPartners`) | Inbound `OPEN`, `REQUESTS` (default), `PARTNERS` (people on the listed servers and on this one) or `NONE`; outbound `ALLOWED` (default), `PARTNERS` or `NONE` | Not the server's matter |
| Workspace | Its Owner: `setWorkspaceDmPolicy` (`Server.dmOutsideFloor`, `Server.dmMemberDefault`) | A floor for its members: `NONE` (default, no floor), `REQUESTS` or `CLOSED` | A default: `DIRECT` (default) or `REQUESTS` |
| Person | Each user: `setMyDmSettings`, `setMyWorkspaceDmSetting` (read in `myDmSettings`) | `REQUESTS` (default) or `NONE`, within the floors | Per workspace `DIRECT` or `REQUESTS`, either way |

- **A fellow member** shares a workspace hosted here with the recipient. Their DM is delivered
  when they are an Owner or Moderator of one shared workspace, when they are a contact, or when
  the recipient's choice (else the workspace's default) for one shared workspace is `DIRECT`;
  otherwise it is a request.
- **Anyone else.** From another server, `RIM_DM_INBOUND` must admit their server or the DM is
  refused, contacts included: `PARTNERS` admits the listed servers, `NONE` none. Then a contact's
  DM is delivered. Otherwise the strictest of the operator's floor (`NONE` refuses strangers on
  this server too), the floors of the recipient's workspaces and the recipient's choice decides
  between a request and a refusal. `OPEN` sets no floor; with the choices a person has today it
  works as `REQUESTS`.
- **A contact** is someone the recipient added (`addContact`), accepted, or messaged first:
  creating a DM records the other person as the creator's contact, so their DM back is never a
  request. A user holds at most 1,000 contacts: `addContact` for someone new past that answers
  `VALIDATION` ("a user holds at most 1000 contacts; remove one first"), and a DM records no
  further contact for its creator.
- **Who the sender knows.** The sender's own limits on DMs to strangers, the daily invites to
  people on other servers and a new account's probation (below), skip only people the sender
  knows: a contact who lists the sender as a contact too (only people on this server have a list
  here), or someone either of the two accepted a DM request from on this server (an invite
  delivered directly from a contact is recorded as accepted). A contact the sender added on their
  own, or that a DM they started recorded, counts as a stranger. These limits are the sender's;
  the recipient's request inbox still applies its own rules.
- **Outbound.** A DM with someone on another server needs `RIM_DM_OUTBOUND` to admit their
  relay: `createConversation` with `remoteMembers` answers `FORBIDDEN` ("this server starts no
  direct messages with that server") otherwise, before their relay is asked anything, and so do
  key package claims for their devices. With `NONE` nothing of the direct space reaches another
  server; with `PARTNERS` it goes to the listed servers only. A conversation that invites someone
  on another server the sender does not know counts against the sender's daily invites
  (`RIM_RELAY_DM_REQUESTS_PER_DAY`, 20): past them it answers `RATE_LIMITED` with
  `retryAfterSeconds`.
- **Profile card.** An account with a principal id and no profile card starts no conversation in
  the direct space, and no workspace DM, with a user here who has a principal id:
  `createConversation` and `addConversationMembers` answer `VALIDATION` ("publish a profile card
  before starting a conversation with someone here"). The invite that takes such a conversation
  through the recipient's DM settings, blocks and request limits carries the creator's card. The
  app publishes the card right after signup and, when that fails, from Settings. Reopening an
  existing DM is not refused.
- **Conversations per day.** A user starts at most 50 new conversations a day in the direct
  space, DMs and groups alike: past them `createConversation` answers `VALIDATION` ("you can start
  at most 50 new conversations a day; try again in N hours"). Reopening an existing DM is not
  counted. The count is kept in memory on each node.
- **Probation.** During `RIM_PROBATION_HOURS` a new account starts no DM with a stranger, in the
  direct space or in a workspace: someone it does not know (above) who is in none of its
  workspaces here, the account without a key included: `VALIDATION` saying when it can
  (`docs/api.md`). The server's default workspace does not count, since every new account joins
  it, so a DM in it meets probation like one in the direct space; any other workspace does. The
  instance operator is never on probation.
- **Refused here.** A DM in the direct space that a recipient here refuses answers `FORBIDDEN`
  when it is started (`createConversation`, `addConversationMembers`). A refused DM from another
  server is answered as for a block, so its server stops addressing the recipient.
- **Request limits.** A conversation that would file a DM request with someone here meets the
  request inbox's limits when it is started (`createConversation`, `addConversationMembers`, in
  the direct space and for a workspace's DMs): `FORBIDDEN` ("their request inbox is full") while
  the recipient holds 100 pending requests, and `RATE_LIMITED` with `retryAfterSeconds` (until
  the sender's oldest request of the last 24 hours leaves the window) once the sender would file
  more than `RIM_RELAY_DM_REQUESTS_PER_DAY` (20) requests in 24 hours on this server; a group
  counts each request it files. A DM delivered directly (from a contact, an Owner or Moderator,
  or a member the recipient takes DMs from directly) files no request and meets no limit, and
  reopening an existing DM files none. A conversation the request inbox still refuses for a
  recipient here is kept from them as an ignored request is, with no notification.
- **Workspace DMs.** In a workspace hosted here, a DM a member whose relay is this server takes
  as a request is held in their request inbox like a DM from outside: until they accept it, it stays out of their
  `inboxConversations` and `workspaceEvents`, and its notifications and unread counts out of
  theirs. Like the request inbox it needs server-to-server traffic to be on; without it workspace
  DMs arrive directly. Groups are unchanged: their members are added by someone already in them.
- **The sender's view.** `Conversation.dmRequestPending` is true for a DM whose other member,
  on this server, has not yet accepted it as a request; false once they did or when it was no
  request, null for groups and channels.
- **The settings.** `setMyDmSettings(outside: REQUESTS)` under a floor that refuses DMs from
  outside answers `VALIDATION` naming it; `myDmSettings.effectiveOutside` says what applies and
  `outsideFloorFrom` where a refusing floor comes from (`OPERATOR`, or `WORKSPACE` with its id and
  name). `setWorkspaceDmPolicy` is the Owner's alone (`FORBIDDEN` for anyone else);
  `setMyWorkspaceDmSetting` needs membership of the workspace (`NOT_FOUND`). `Server.dmOutsideFloor`
  and `Server.dmMemberDefault` are null in the direct space. A person whose relay is another server
  sets their choices there; both personal mutations are refused through the forward path.

## Archived Channels

- Owner/Moderator archive and unarchive channels (`setConversationArchived`); other roles get
  `FORBIDDEN`. Only channels can be archived (`VALIDATION` otherwise).
- An archived conversation is read-only: sending, replying, editing, deleting and forwarding into
  it, reactions, pins, topic edits, threads, attachments, typing, joining and inviting fail with
  `CONVERSATION_ARCHIVED`. Leaving and removing members still work.
- Archived channels are hidden from `channels` unless `includeArchived: true`, and stay readable
  and searchable by whoever could read them before.

## Roles And Moderation

### Roles

| Role | Meaning |
|---|---|
| `OWNER` | Everything. One per workspace; the role cannot be granted (`setMemberRole` with `OWNER` is `VALIDATION`). |
| `MODERATOR` | Moderation (kick, ban, timeout, delete any message, slow mode, reports) and channel management (archive, remove members). Changes roles only between `MEMBER` and `GUEST`. |
| `MEMBER` | The default. Reads and writes wherever they are a member (public channels without joining), creates channels and groups, joins public channels, invites people into conversations. |
| `GUEST` | Restricted. Reads and writes only in conversations they were explicitly added to. |

- **Migration:** before the member role existed every new member was created as `guest`, so "guest" meant an
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

### Workspace invites

- `createInvite` is owner-only (`FORBIDDEN` for everyone else). `expiresAt` defaults to seven
  days from now and must be in the future and at most 30 days away, else `VALIDATION`; the
  same bounds as instance invites. `maxUses` defaults to 1 and is capped by
  `RIM_LIMIT_INVITE_MAX_USES` when the operator sets it. The token is returned once and never shown again.
- `recipientEmail` emails the link to that address, and nothing more: the invite is not bound to
  it, and whoever holds the link can use it. One user creates at most
  `RIM_LIMIT_INVITES_PER_USER_PER_DAY` invites (default 50, 0 turns it off) and sends at most as
  many invite emails in 24 hours; past either a `createInvite` is `VALIDATION` and creates
  nothing.
- `workspaceInvites(serverId, dir, cursor, limit)` lists the workspace's invites for its Owner
  (`FORBIDDEN` for everyone else): every invite, revoked, expired and used-up ones included,
  ordered `createdAt DESC, id DESC` with the standard cursor rules (`dir` defaults to `AFTER`,
  towards older invites; `limit` defaults to 50, at most 100). Each `WorkspaceInvite` has its
  role, `useCount`, `maxUses`, `expiresAt`, `createdAt`, `createdBy`, `revokedAt` and `state`:
  `REVOKED` when revoked, else `EXPIRED` once `expiresAt` has passed, else `USED_UP` once
  `useCount` reached `maxUses`, else `ACTIVE`. Neither the token nor its hash is listed.
- `revokeInvite(inviteId)` ends an invite: it admits nobody from then on, and members who joined
  with it stay. Owner-only: another member of the invite's workspace gets `FORBIDDEN`; an
  unknown invite, or one of a workspace the caller is not a member of, is `NOT_FOUND`. Revoking
  a revoked invite answers it unchanged. The revocation is audit-logged
  (`workspace_invite_revoked`), as the creation is (`workspace_invite_created`).
- An invite stored without an expiry, by a server from before invites had to expire, keeps
  admitting until it is used up or revoked.
- Redeeming an invite (`joinServerByInvite`, `signupWithInvite`, `signupWithKey`, a join through
  another server) checks the revocation, the expiry and the uses in the same update that counts
  the use. A revoked, expired or used-up invite is `FORBIDDEN` for `joinServerByInvite`; a signup
  gets `INVALID_TOKEN` for a revoked or used-up invite and `EXPIRED_TOKEN` for an expired one.

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
- The direct space has no Owner or Moderator: everyone there is a guest. Its reports (of DMs and
  groups) go to the instance operators instead (`docs/api.md`, "Instance moderation"):
  `messageReports`, `messageReport` and `resolveReport` with the direct space's id
  (`relayInfo.directSpaceId`) answer an instance operator and refuse everyone else with
  `FORBIDDEN`, an app or bot acting for an operator too. An operator gets nothing else there: no
  kick, ban, timeout, role change or slow mode.

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

A subscription that falls behind loses events, possibly these two. Before it delivers another
event it reads the subscriber's membership and role again: it ends when they are no longer a
member (or the read fails), and otherwise goes on with the visibility of their current role.

## Reactions

`toggleReaction(serverId, conversationId, messageId, emoji)` adds the caller's reaction with
that emoji, or removes it when they already have it, and answers the message's reaction groups.
It needs write access to the conversation and a message that is not deleted.

- `emoji`, trimmed, is 1 to 32 characters and is either a Unicode emoji (pictographs with the
  components that join, modify or tag them: zero-width joiner, variation selectors, skin tones,
  hair, keycaps, regional indicators, tags; digits, `#` and `*` alone are not one) or a custom
  emoji's `:name:`, the name made of letters, digits and underscores. A `:name:` that starts a
  new emoji on the message must name a custom emoji of the workspace while the `custom_emoji`
  feature is enabled; with the feature off every new `:name:` is refused. Anything else answers
  `VALIDATION`.
- A message carries at most 20 different emoji, and one user puts at most 10 reactions on one
  message. Adding past either answers `VALIDATION`; joining an emoji the message already has
  (under the per-user cap) and removing a reaction always work. Toggles on one message run one
  at a time (the message row is locked), so concurrent toggles cannot pass the caps.
- Every toggle sends `ReactionsUpdated` to the conversation's members with all of the
  message's reaction groups, which the caps bound.

## Notification Rules

The server decides, once per message, who is told about it, and tells a message's author when
someone reacts to it.
For every recipient it stores a notification (the inbox, the bell badge and the email digest) and
sends that user a private `NotificationDue` event; clients only render what the server decided.
Sound and preview stay device-local client settings.

### Settings

- **Per-conversation level** (`setConversationNotificationLevel`, `Conversation.notificationLevel`):
  `ALL`, `MENTIONS` or `NONE`; null inherits the workspace default.
  `Conversation.effectiveNotificationLevel` is what applies now: `NONE` while a timed mute is in
  effect, else the conversation level, else the workspace default.
- **Workspace settings** (`myNotificationSettings`, `updateNotificationSettings`), per user and
  workspace: `defaultLevel` (default `ALL`; in the server's default workspace, which every
  account joins, `MENTIONS`, so a plain message there stores no notification per member unless
  they chose `ALL`, here or for the conversation), `keywords` (at most 20, each 2 to 40 characters after
  trimming, stored lower-cased and deduplicated), a daily do-not-disturb window `dnd`
  (`start`/`end` as `"HH:MM"` in an IANA `timezone`; crosses midnight when `end < start`; `start`
  must differ from `end`), `digest` (`OFF`, `HOURLY`, `DAILY`; `docs/email.md`), and
  `reactions` (default `true`: reactions to the user's messages notify them; null in
  `updateNotificationSettings` keeps the stored value).
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

### Reactions

A reaction added to a message (`toggleReaction` that adds, not one that removes) tells its author,
with reason `reaction`:

1. Never about their own reaction or a deleted message, and only while the author is still a
   member of the conversation.
2. A timed mute in effect, effective level `NONE`, or the `reactions` setting off yields nothing.
   `MENTIONS` and `ALL` admit it.
3. Inside do-not-disturb it is stored and the event is `quiet`, as for messages.
4. The author has at most one reaction notification per message. The first reaction inserts it;
   each later one replaces its emoji and reactor, marks it unread and moves its `createdAt` to
   now, so the entry surfaces again. `NotificationDue` is sent when the entry is new, was read,
   or was last surfaced more than 10 minutes ago; a reaction replacing an unread entry from the
   last 10 minutes updates it silently, so reacting and un-reacting in a loop cannot make the
   author's client toast. A notification the message already has for another reason (a
   mention, say) is left as it is, and no event is sent.
5. Best-effort, after the reaction commits: a failure is logged and never fails the reaction.

Digest emails list messages only: reaction notifications never trigger, nor appear in, a
digest. Reading the conversation (`markAsRead`) marks them read like any other notification of
its messages.

One message yields at most one notification per user (unique `(user, message)`). Notifications
are created after the message commits (`sendMessage`, `replyToMessage`, and forwards into the
target conversation) and best-effort: a failure is logged and never fails the send. Edits do not
notify again. Deleting a message deletes its unread notifications (read ones stay).
`UserMentioned` is still emitted for the mentions view.

### Inbox

- `myNotifications(serverId, unreadOnly, dir, cursor, limit)`: newest first (`createdAt DESC,
  id DESC`, standard cursors; `AFTER` goes to older ones). Each item has `reason`, `createdAt`,
  `readAt` and nested `conversation` / `message`, which are null once the viewer can no longer
  read the conversation. A `REACTION` item also has `reactionEmoji` and `reactedBy` (the latest
  reaction); both are null for every other reason.
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

`reason` is `direct`, `mention`, `keyword`, `thread_reply`, `all` or `reaction`. A `reaction`
event adds `reaction_emoji` and `reaction_by` (the reactor's user id); the two keys are absent
from every other event:

```json
{"type": "NotificationDue", "workspace_id": "…", "user_id": "…", "notification_id": "…",
 "conversation_id": "…", "message_id": "…", "reason": "reaction", "quiet": false,
 "reaction_emoji": "👍", "reaction_by": "…"}
```

The event is delivered only to `user_id` (the server-side scope list is `[user_id]`), guests
included.

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

## Attachments

- Sending a message takes no attachments; the author adds them to a message they already sent.
- `createAttachment(serverId, conversationId, messageId, filename, contentType, sizeBytes)`:
  only the message's author, at most 25 MB. The declared size is reserved against the
  workspace quota (`RIM_MAX_WORKSPACE_STORAGE_BYTES`), the attachment is stored as pending
  upload, the message's `has_attachments` is set, and the answer is an upload ticket: the
  attachment id, an upload token, the size limit and an expiry.
- `POST /api/files/upload` (multipart: `upload_token`, `attachment_id` and one file) stores the
  bytes and marks the attachment uploaded; a file larger than declared is refused.
- `finalizeAttachment` marks an uploaded attachment ready. `deleteAttachment` removes it and
  refreshes `has_attachments`.
- An upload still pending 15 minutes after it started is marked failed by the stale-attachment
  cleanup job (`docs/operations/background-jobs.md`).
- Reading an attachment (`GET /api/attachments/{id}/download`) needs workspace membership and read
  access to the conversation of its message.
- In an encrypted conversation the client uploads the encrypted blob: `createAttachment` then
  requires `blobSha256`, a fixed file name and type, and the upload is checked against the hash
  (`docs/specs/e2ee-mls-v1.md`, "Attachments").
- The `attachments` instance feature switches all of this off (`docs/operations/features.md`).

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
