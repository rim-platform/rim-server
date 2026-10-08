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
  limit: Int                                     # per conversation; clamped to 1..=200, default 100
}

input MessageChangeCursorInput {
  conversationId: ID!
  after: String       # a cursor from a previous call; null reads from the beginning
}
```

The result has one entry per requested conversation, in request order.

## Semantics

- **Scope.** Every message of the conversation: timeline messages and thread replies (a reply
  carries `threadId`). Each requested conversation must be readable by the caller (same gate as
  `messages`); if any is not, the whole request fails with that error (`NOT_FOUND` or
  `FORBIDDEN`). An `after` that is not a cursor this API returned fails with `VALIDATION`.
- **Order.** Changes come oldest first, ordered by `version`.
- **One entry per message.** A message appears once, at its newest version, carrying its current
  state. Three edits since the cursor give one `EDITED` entry with the latest text, not three.
- **Kind.** Judged from the current state: `DELETED` if `message.isDeleted`, else `EDITED` if
  `editedAt` is set, else `CREATED`. A client cache can treat every entry as an upsert keyed by
  `message.id` and keep the copy with the higher `version`.
- **Deletes are tombstones.** A deleted message comes back with `isDeleted: true` and
  `text: null` (soft delete; the row stays). Hard delete is not supported.
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

Existing messages were numbered `1..n` per conversation in `(sent_at, id)` order when the
migration ran. The migration is additive; `down` drops the trigger, the counter table and the
two columns, and clients then re-sync from the beginning once it runs again.
