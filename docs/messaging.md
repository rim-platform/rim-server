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

Spec: `docs/specs/track-1-1-private-channels.md`.

- **Public channels** (`isPrivate = false`) are readable by every workspace member, joined or not.
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
  (invite acceptance, join by invite, owner adding them, bootstrap, server creation — all through
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
  it, reactions, pins, topic edits, threads, attachments, typing, joining, inviting and starting an
  incident fail with `CONVERSATION_ARCHIVED`; alert ingestion into an archived channel is refused
  (409 `{"error": "conversation is archived"}` on the webhook). Leaving and removing members still work.
- Archived channels are hidden from `channels` unless `includeArchived: true`, and stay readable
  and searchable by whoever could read them before.

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
- Threads: `ORDER BY last_message_at DESC, id DESC`
- Inbox conversations: `ORDER BY last_activity_at DESC, id DESC`
- Channels: `ORDER BY created_at ASC, id ASC` (filtered by `ChannelListFilter`: `joinedOnly`,
  `includeArchived`; both default to `false`)

Cursor pagination uses overfetch (`limit + 1`) so `next` and `prev` reflect actual additional data, not page-size guesses.

## Message References

- The backend exposes message references/permalinks for both timeline messages and thread replies.
- A message reference contains:
  - workspace/server id
  - conversation id
  - message id
  - optional thread id
  - permalink path
  - stable UI anchor
- Timeline message references use a conversation message path.
- Thread reply references use a thread message path.
