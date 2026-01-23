# Pagination Contract

This repo uses cursor-based pagination for any list that can grow without bound (threads, messages,
inbox conversations, etc.). The goal is: stable ordering, no duplicates between pages, and
consistent semantics across all storage backends.

## Definitions

- Each endpoint defines a fixed sort key and direction (ASC/DESC), and a cursor type that matches
  that sort key.
- `PageDir` describes the relation to the cursor *in terms of the sort key*, not UI scrolling.
  - `Before`: items strictly before the cursor.
  - `After`: items strictly after the cursor.
- Tie-breaker IDs are mandatory to make ordering deterministic.

## Cursor Comparison Rules

For a cursor `(ts, id)`:

- `Before` filter:
  - `(ts < cursor.ts) OR (ts == cursor.ts AND id < cursor.id)`
- `After` filter:
  - `(ts > cursor.ts) OR (ts == cursor.ts AND id > cursor.id)`

The DB query must always include an `ORDER BY` that matches the cursor fields.

## Page Fields

`Page<T, C>` has:

- `items`: returned in the endpoint's defined order.
- `next`: cursor to continue in the *same* `PageDir` as the request.
- `prev`: cursor to go in the *opposite* `PageDir`.

This allows a client to implement infinite scroll by repeatedly calling the same endpoint with:

- `cursor = page.next`
- `dir = (unchanged)`

## How To Compute `next` and `prev`

Given the returned `items` and a request `dir`:

- `next` points to the "edge" that is further in the requested direction:
  - for `After`: cursor of the *maximum* key in `items`
  - for `Before`: cursor of the *minimum* key in `items`
- `prev` is the opposite edge:
  - for `After`: cursor of the *minimum* key in `items`
  - for `Before`: cursor of the *maximum* key in `items`

Where "minimum/maximum" are defined by the endpoint's sort key, independent of the endpoint's
display order.

If `items` is empty, both cursors are `None`.

## Endpoint Ordering (Current)

- Messages in a thread:
  - `ORDER BY sent_at DESC, id DESC` (API returns newest-first; UI can reverse for newest-at-bottom)
  - cursor: `(sent_at, message_id)`
- Threads in a conversation:
  - `ORDER BY last_message_at DESC, id DESC` (thread-first view)
  - cursor: `(last_message_at, thread_id)`
- Inbox conversations (DM + Group):
  - `ORDER BY last_activity_at DESC, id DESC`
  - cursor: `(last_activity_at, conversation_id)`
- Channels:
  - `ORDER BY created_at ASC, id ASC` (stable, oldest-first)
  - cursor: `(created_at, conversation_id)`
