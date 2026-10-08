# Message Change Feed

The change feed lets a client ask "what changed in these channels since I last synced" and get
every message created, edited or deleted since then, in order, with a cursor it can store. It is
the server half of the client-side history cache (master plan, Stage 2). This document is the
contract the cache builds on: **the GraphQL shape and the cursor semantics below are frozen.**
Additive changes (a new optional input field, a new output field) are fine; anything else needs a
new field or query next to this one.

## Query

```graphql
query MessageChanges($input: MessageChangesInput!) {
  messageChanges(input: $input) {
    conversationId
    status          # OK | NO_ACCESS | RESET
    cursor          # store this; pass it back as `after`
    hasMore         # more changes follow `cursor`; ask again right away
    changes {
      kind          # CREATED | EDITED | DELETED
      version       # decimal string, increases with every change of the message
      editedAt      # null for a never-edited message
      message { id threadId authorId text isDeleted sentAt ... }
    }
  }
}
```

```graphql
input MessageChangesInput {
  serverId: ID!
  conversations: [MessageChangeCursorInput!]!   # at most 100
  limit: Int                                     # per conversation; clamped to 1..=200, default 100,
                                                 # lowered so one request returns at most 1000 changes
}

input MessageChangeCursorInput {
  conversationId: ID!
  after: String       # a cursor from a previous call; null reads from the beginning
}
```

The result has one entry per requested conversation, in request order.

## Semantics

- **Scope.** Every message of the conversation: timeline messages and thread replies (a reply
  carries `threadId`). An `after` that is not a cursor this API returned fails the request with
  `VALIDATION`; a caller who is not a member of the workspace gets `FORBIDDEN`.
- **Status, per conversation.** Each entry says how its conversation was answered, so one
  conversation never stalls the sync of the others:
  - `OK`: the changes after `after`.
  - `NO_ACCESS`: the caller cannot read the conversation (same gate as `messages`: removed from
    it, not a member of a private one, or it does not exist in the workspace). `changes` is
    empty, `hasMore` is false and `cursor` echoes `after`. Stop syncing it; whether the cached
    copy is kept is the client's policy.
  - `RESET`: the stored cursor is ahead of the conversation, because the server's counters were
    reset after the client stored it (the migration rolled back and re-run, or a database
    restored from an older backup). The changes are read from the beginning instead. The client
    drops its cached copy of the conversation, then applies them and follows `hasMore` as in an
    initial sync. Without this the feed would silently return nothing until the counter caught
    up, skipping real changes.
- **Budget.** One request returns at most 1000 changes in total: with many conversations the
  per-conversation `limit` is lowered to share that budget (never below 1). Follow `hasMore`.
- **Order.** Changes come oldest first, ordered by `version`.
- **One entry per message.** A message appears once, at its newest version, carrying its current
  state. Three edits since the cursor give one `EDITED` entry with the latest text, not three.
- **Kind.** Judged from the current state: `DELETED` if `message.isDeleted`, else `EDITED` if
  `editedAt` is set, else `CREATED`. A client cache can treat every entry as an upsert keyed by
  `message.id` and keep the copy with the higher `version`.
- **Deletes are tombstones.** A deleted message comes back with `isDeleted: true`, `text: null`
  and no `attachments` (soft delete; the row stays). Hard delete is not supported.
- **Edits made before the feed existed.** Messages edited before migration
  `m20261008_211159_add_message_change_feed` ran have no `editedAt`, so they come back as
  `CREATED` until they change again.
- **Cursor.** Opaque string; today it is the conversation's last returned `version`. When
  nothing changed it echoes the cursor that was sent (`"0"` for a null one). Store it per
  conversation.
- **No skipped changes.** Versions become visible in increasing order with no gaps that fill in
  later, so storing the last cursor and asking again never misses a change. See "How versions
  are assigned".
- **Initial sync.** `after: null` returns the conversation's whole history, page by page
  (`hasMore`), oldest first.

## What the feed does not carry

Only changes to the message row itself bump a version: creation, text edits, soft delete, and
`hasAttachments`. Reactions, pins, mentions, link previews and a message becoming a thread root
(`rootThreadId`) do not; those stay on their own queries and real-time events.

## Real-time events

The WebSocket subscription keeps delivering new, edited and deleted messages as before; its
events carry no `version`. A client cache treats an event as a hint: it may show the message
right away, and it then calls `messageChanges` with the conversation's stored cursor to record
the change at its version. A client that reconnects after a gap does the same for every cached
conversation, so a missed event never leaves the cache behind.

## How versions are assigned

Migration `m20261008_211159_add_message_change_feed` adds `message.version`,
`message.edited_at` and a `message_change_seq` table holding one counter per conversation. A
database trigger runs before every insert into `message`, and before every update that changes
`text`, `is_deleted` or `has_attachments`. It upserts the conversation's counter row and writes
the new value to `version`; on a text change it also sets `edited_at`. Because this lives in the
database, every write path (send, reply, forward, edit, delete, attachments, and any added later)
feeds the change log without service code.

The counter row stays locked until the writing transaction commits. A second writer in the same
conversation waits, then takes the next number; after a rollback the number is reused. So a
reader cannot see version N+1 while N is still uncommitted, which is what makes "store the last
cursor" safe. A global sequence would not give this: it hands out numbers at insert time, so a
later number can commit and become visible first.

Deleting a conversation while a message is being written into it can deadlock (the delete
cascades to the counter row while the send holds it and waits on the conversation row);
Postgres aborts one of the two and nothing else is affected.

Existing messages were numbered `1..n` per conversation in `(sent_at, id)` order when the
migration ran. The back-fill rewrites every message row while `message` is locked, so on a large
table run the migration in a maintenance window. The migration is additive; `down` drops the trigger, the counter table and the
two columns, and clients then re-sync from the beginning once it runs again.
