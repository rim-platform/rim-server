# Messaging Contract

This document describes the current v0 backend contract for conversations, messages, threads, and attachments.

## Product Terms

- `workspace` is the product-level term.
- `Conversation` is the messaging container used for channels, DMs, and group chats.
- `MessageThread` is an optional thread opened from a timeline message.

## Conversation Types

- `Channel`: stable shared conversation inside a server/workspace.
- `Direct`: 1:1 conversation between two users.
- `Group`: multi-user private conversation.

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
- Channels: `ORDER BY created_at ASC, id ASC`

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
