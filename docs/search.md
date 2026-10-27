# Message Search (Full-Text Search)

`searchMessages` runs Postgres full-text search over message text, scoped to conversations the
requesting actor can see. This document covers the query operator syntax, the ranking/pagination
design, and the visibility contract.

## Operator syntax

The `query` string mixes free text with structured operators. Operators are stripped from the
free text before it is passed to Postgres's `websearch_to_tsquery`; the remainder is the tsquery.
Whitespace-separated tokens matching `key:value` are treated as operators; everything else
(including an unrecognized `key:`, or an operator with an empty value) is free text — the parser
never errors on unknown syntax, matching `websearch_to_tsquery`'s own forgiving behavior.

| Operator | Example | Meaning |
|---|---|---|
| `from:` | `from:@alice`, `from:<user-uuid>` | Only messages sent by this user. |
| `in:` | `in:#general`, `in:<conversation-uuid>` | Only messages in this channel. |
| `has:link` | `has:link` | Only messages containing a URL (`https?://`). |
| `has:attachment` / `has:file` | `has:attachment` | Only messages with at least one attachment. |
| `before:` | `before:2026-07-01` | Messages sent strictly before UTC midnight on this date (exclusive). |
| `after:` | `after:2026-06-01` | Messages sent on or after UTC midnight on this date (inclusive). |

Multiple `has:` operators AND-combine. Operators can be freely mixed with free text and with each
other, e.g. `deploy from:@alice in:#ops has:link after:2026-06-01`.

**Unknown/inaccessible `from:`/`in:` targets never error.** If `from:` doesn't resolve to a real
user, or `in:` doesn't resolve to a channel the actor can see, the query returns an **empty page**,
not an error — this avoids leaking whether a username or channel exists to someone who can't see
it (see "Visibility" below).

## Ranking vs. pagination — why recency, not relevance

Search results are **paginated by recency** (`sentAt`, `id` — the same cursor contract as every
other message list; see `docs/pagination.md`), not by relevance. Each result also carries a `rank`
(`ts_rank` score) and an optional `headline` (`ts_headline` snippet with `<mark>...</mark>`
highlights: the whole text of a message up to 300 characters, otherwise one excerpt around the
matches) for client-side display or secondary sort.

This is a deliberate tradeoff: cursor pagination requires a stable total order with a unique
tie-breaker. `ts_rank` is neither unique nor stable across concurrent inserts, so paginating by
rank would produce skips and duplicates across pages. Pagination by `(sentAt, id)` avoids that at
the cost of not surfacing the single "best" match first — the client can still sort a page's
`items` by `rank` for display if it wants a best-match-first feel within a page. A dedicated
relevance-ranked top-N (non-paginated) query is a possible future addition for grounded AI recall,
but is out of scope here.

## Visibility (security-critical)

Search **never returns a message from a conversation the actor cannot see** — this is upheld
per-request by `DefaultMessageService::search_messages`, which computes the accessible
conversation set from `list_accessible_conversation_ids` (plus a full member-access gate when an
explicit `conversationId` scope is given) before ever calling the repository. The repository
itself is intentionally visibility-agnostic: it only ever searches the conversation ids it's
handed, and an empty set yields zero rows — never "search everything". See the doc comments on
`MessageSearchParams` (`rim_core::message::repository`) and the db-tests in
`rim_infra_db/tests/message_search_fts.rs` for the enforced guarantees.

## Underlying storage

`message.search_tsv` is a `GENERATED ALWAYS ... STORED` `tsvector` column
(`to_tsvector('english', coalesce(text, ''))`) with a GIN index, added in migration
`m20260708_225022_add_message_search_tsv`. The `'english'` text-search configuration is
hardcoded; per-workspace configuration or a `simple` fallback for non-English content is a
possible follow-up, not implemented here.

## GraphQL contract

```graphql
input SearchMessagesInput {
  serverId: ID!
  query: String!
  conversationId: ID
  limit: Int
  cursor: MessageCursorInput
  dir: PageDir
}

type SearchResultMessage {
  # ...existing message fields...
  rank: Float!
  headline: String
}

type SearchResultPage {
  items: [SearchResultMessage!]!
  next: MessageCursorOutput
  prev: MessageCursorOutput
  pageInfo: PageInfo!
}
```

This is a **breaking change** from the pre-Track-B1 `searchMessages` (which returned a flat,
unpaginated `[SearchResultMessage!]!` and took top-level `limit`/`includeDeleted` args). Frontend
codegen must be re-run against the updated schema.
