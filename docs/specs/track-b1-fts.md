# Track B1 — Postgres Full-Text Search (implementation spec)

**Author:** iron-man · **Implementer:** hulk · **Requested by:** nick-fury
**Priority:** first in Track B — Track C (grounded AI recall) depends on real search.
**Prereq:** `track-b-dbtests-loud-skip-fix.md` (db-tests must fail loudly before we trust FTS tests).

## Goal
Replace the LIKE/`ILIKE`-based message search with Postgres full-text search (tsvector + GIN),
add a small server-side search-syntax parser (`from:`, `in:`, `has:`, `before:`/`after:`), expose
relevance ranking, and make `searchMessages` a cursor-paginated connection per `docs/pagination.md`
— all while preserving the existing per-conversation visibility rules exactly.

## Constraints
- Forward-only migration; no drop/rebuild. `sea migrate generate` convention.
- `cargo fmt` / `clippy -D warnings` / tests green. Tests **and** docs for everything new.
- No `unwrap`/`expect` in library code; typed `ServiceError`.
- **Security:** search must never return a message in a conversation the actor cannot see. This is
  the load-bearing requirement — see §5.
- This **is** a GraphQL contract change (searchMessages becomes paginated + ranked). It requires
  frontend codegen and vision coordination — see §6 and the SDL delta. Coordinate via nick-fury
  before landing; do not silently break the frontend.

## Current state (what you are replacing)
- Resolver `search_messages` — `rim_api/src/query.rs:414`, returns a flat `Vec<SearchResultMessage>`
  (no pagination, no rank), limit clamped to 50.
- Service `MessageService::search_messages` — `rim_services/src/message/service.rs:39`, takes
  `SearchMessagesQuery` (`rim_services/src/message/command.rs:86`).
- Impl — `rim_infra_services/src/message.rs:790` (scoped vs workspace-wide branches; forces
  `include_deleted = false`).
- Repo — `rim_core/src/message/repository.rs:42` (`search_messages`,
  `search_messages_across_conversations`); LIKE impl `rim_infra_db/src/sea_orm_impl/message.rs:192`
  using `message::Column::Text.contains(query)`.
- Content column: **`message.text`, `VARCHAR(1024)`, NULLABLE** (entity `entity/src/message.rs`,
  created in `migration/src/m20251123_105135_create_threads_message.rs:44`). No FTS scaffolding exists.
- Visibility set: `list_accessible_conversation_ids(server_id, user_id)` —
  `rim_infra_db/src/sea_orm_impl/conversation.rs:349` (channels: server membership; DM/group:
  explicit `conversation_member`). Gate helpers in `rim_infra_services/src/util.rs`.
- Pagination: `docs/pagination.md` — cursor `(sent_at, message_id)`, `ORDER BY sent_at DESC, id
  DESC` for messages; `MessagePage`/`PageInfo`/`MessageCursorInput`/`MessageCursorOutput` in
  `rim_api/src/types.rs` (~1261–1400).

## §1 Migration — tsvector generated column + GIN index

Generate: `sea migrate generate add_message_search_tsv`. Register in `migration/src/lib.rs` (mod +
`Box::new`, last in order).

**Decision: a STORED generated column, not a trigger.** `to_tsvector('english', …)` with a constant
config literal is IMMUTABLE, so a `GENERATED ALWAYS AS … STORED` column is valid and stays in sync
automatically with zero trigger maintenance. sea-query can't express generated columns, so use raw
`execute_unprepared` (consistent with how this codebase adds CHECK constraints/indexes).

`up`:
```sql
ALTER TABLE message
  ADD COLUMN search_tsv tsvector
  GENERATED ALWAYS AS (to_tsvector('english', coalesce(text, ''))) STORED;

CREATE INDEX idx_message_search_tsv ON message USING GIN (search_tsv);
```
`down`:
```sql
DROP INDEX IF EXISTS idx_message_search_tsv;
ALTER TABLE message DROP COLUMN IF EXISTS search_tsv;
```

**Ops / rollback note (put in the migration doc comment):** adding a STORED generated column
rewrites the `message` table and takes an `ACCESS EXCLUSIVE` lock for the duration of the backfill.
At RIM's current self-hosted scale this is acceptable during a deploy window. For a large existing
table, the low-lock alternative is: add a plain nullable `tsvector` column, backfill in batches, add
a `BEFORE INSERT/UPDATE` trigger, then `CREATE INDEX CONCURRENTLY`. Do **not** do the concurrent
variant inside a sea-orm migration (migrations run in a transaction; `CREATE INDEX CONCURRENTLY`
cannot). Recommend the generated-column form now; note the alternative for scale. Forward-only: the
column backfills existing rows automatically; no data migration needed.

**Entity:** do **not** add `search_tsv` to `entity/src/message.rs`. The FTS query is written as a
custom SQL expression / `FromQueryResult` struct (§4), and keeping the column off the `Model` means
inserts/updates never reference the generated column (Postgres rejects writes to generated columns).

**English config note:** `'english'` is a reasonable default. If multilingual content matters later,
that's a follow-up (per-workspace config / `simple` fallback). Hardcode `'english'` now and note it.

## §2 Search-syntax parser (server-side)

**Decision: parse server-side into a typed `ParsedSearchQuery`; do not reuse the channel-lens
`FilterSpec` directly.** The lens model (`rim_core/src/channel_lens/model.rs`) is applied
*in-memory* over a bounded batch and has neither date-range fields nor comparison operators, so it
is the wrong tool for a SQL FTS predicate. Align field *names/semantics* with lenses (author,
has-link, has-attachment) for consistency, but the parser and its SQL application are new. Document
this decision in the module.

New module `rim_services/src/message/search_query.rs` (pure, no I/O):
```rust
/// A user search string parsed into free-text terms plus structured operators.
/// Operators are stripped from the free text; the remainder becomes the tsquery.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct ParsedSearchQuery {
    /// Free-text remainder, fed to `websearch_to_tsquery`. May be empty (operator-only search).
    pub text: String,
    /// `from:@name` / `from:<uuid>` — resolved to a UserId by the service layer, not here.
    pub from: Option<FromToken>,
    /// `in:#channel` / `in:<uuid>` — resolved to a ConversationId by the service layer.
    pub in_channel: Option<ChannelToken>,
    /// `has:link`, `has:attachment` (repeatable; AND-combined).
    pub has: Vec<HasFilter>,
    /// `after:YYYY-MM-DD` — inclusive lower bound on sent_at (UTC midnight).
    pub after: Option<chrono::DateTime<chrono::Utc>>,
    /// `before:YYYY-MM-DD` — exclusive upper bound on sent_at (UTC midnight next day is NOT applied;
    /// treat as sent_at < YYYY-MM-DD 00:00:00Z; document the boundary).
    pub before: Option<chrono::DateTime<chrono::Utc>>,
}
pub enum FromToken { Username(String), UserId(uuid::Uuid) }
pub enum ChannelToken { Name(String), ConversationId(uuid::Uuid) }
pub enum HasFilter { Link, Attachment }

pub fn parse_search_query(raw: &str) -> ServiceResult<ParsedSearchQuery>;
```
Parsing rules:
- Tokenize on whitespace; a token matching `^(from|in|has|before|after):(.+)$` is an operator, else
  it's a free-text term. Unknown `key:value` → treat as free text (don't error; forgiving like
  `websearch_to_tsquery`).
- `from:@x` and `from:x` both → `Username("x")`; a token that parses as a UUID → `UserId`.
- `in:#x` / `in:x` → `Name("x")`; UUID → `ConversationId`.
- `has:link` → `Link`, `has:attachment`/`has:file` → `Attachment`; other `has:` values → `Validation` error.
- `before:`/`after:` values parse as `YYYY-MM-DD` (`chrono::NaiveDate` → UTC midnight). Bad date → `Validation`.
- Reject total input longer than a sane cap (e.g. 512 chars) with `Validation`.
- Free-text is re-joined with spaces and passed as-is to `websearch_to_tsquery` (which never errors
  on syntax — it tolerates quotes, `OR`, leading `-`).

Unit-test this module exhaustively (pure function) — see §7.

## §3 Service layer

Replace the body of `DefaultMessageService::search_messages` (`rim_infra_services/src/message.rs:790`)
and widen `SearchMessagesQuery` (`rim_services/src/message/command.rs:86`):
```rust
pub struct SearchMessagesQuery {
    pub actor: Actor,
    pub server_id: ServerId,
    pub conversation_id: Option<ConversationId>, // optional explicit scope (in addition to in:)
    pub query: String,
    pub limit: u32,
    pub cursor: Option<MessageCursor>,           // reuse rim_core message cursor (sent_at, id)
    pub dir: PageDir,                            // reuse rim_core PageDir
    // include_deleted removed — search never returns deleted messages (see below).
}
```
Flow:
1. `parse_search_query(&query)?`.
2. **Resolve operators to IDs:**
   - `from: Username(n)` → look up the user by username (add/confirm a `UserRepository`
     `find_by_username` or reuse the existing lookup used by mentions/`@`-resolution — check
     `rim_infra_services` mention/user paths). If unknown user → return an **empty page** (not an
     error; avoids user-enumeration and matches "no results"). `from: UserId` used directly.
   - `in: Name(n)` → resolve the channel by name within `server_id` (reuse the channel-title lookup
     used elsewhere; conversations have unique channel titles per server per
     `m20260322_000000_add_unique_channel_title`). If unknown/not a channel → empty page.
3. **Compute the accessible conversation set** (security core, §5):
   - Base set = `conversation_repo.list_accessible_conversation_ids(server_id, actor.user_id)`.
   - If `conversation_id` (explicit scope) is set: run the full `ensure_conversation_member_access`
     gate; the search set becomes `{conversation_id}` (already access-checked).
   - If `in:` resolved to a channel id: **intersect** it with the accessible set. If the requested
     channel isn't in the accessible set → empty page (do not leak existence).
   - Otherwise: the search set is the full accessible set. Require `ensure_server_member` first.
4. Build repo params: the free-text tsquery string, the resolved `from`/`has`/`before`/`after`
   filters, the accessible conversation-id list, `limit` (clamp to `MAX_PAGE_LIMIT`), `cursor`, `dir`.
5. Call the new repo method (§4); map hits → DTOs; return a `Page<MessageSearchHitDto, MessageCursor>`.

`include_deleted` is dropped from search entirely: search always filters `is_deleted = false`
(matches today's forced behavior at `message.rs:817` and avoids leaking tombstoned content).

## §4 Repository layer

Replace the two LIKE methods with one FTS method (delete
`search_messages`/`search_messages_across_conversations` from the trait + impl; update the one
command executor caller at `rim_infra_services/src/command/executor/chat.rs:200` and the
`chat.search_messages` registry entry to go through the new path):

`rim_core/src/message/repository.rs`:
```rust
/// Full-text search within an explicit set of conversation ids (the caller-computed
/// accessible set — the repo does NOT decide visibility). Ordered by (sent_at DESC, id DESC)
/// for stable cursor pagination; `rank` is ts_rank against the query for client-side display.
async fn search_messages_fts(
    &self,
    params: MessageSearchParams,
) -> RepoResult<Vec<MessageSearchHit>>;

pub struct MessageSearchParams {
    pub conversation_ids: Vec<ConversationId>, // empty => empty result (never "all")
    pub tsquery: String,                        // free text for websearch_to_tsquery; may be empty
    pub author_id: Option<UserId>,
    pub has_link: bool,
    pub has_attachment: bool,
    pub after: Option<DateTime<Utc>>,
    pub before: Option<DateTime<Utc>>,
    pub limit: u32,
    pub cursor: Option<MessageCursor>,
    pub dir: PageDir,
}
pub struct MessageSearchHit { pub message: Message, pub rank: f32, pub headline: Option<String> }
```
`rim_infra_db/src/sea_orm_impl/message.rs` — implement with a `FromQueryResult` struct and a
statement built via `sea_orm::Statement`/`sea_query` with bind params (never string-interpolate user
input). Core SQL shape:
```sql
SELECT m.*,                                            -- all message columns
       ts_rank(m.search_tsv, q.query) AS rank,
       ts_headline('english', coalesce(m.text,''), q.query,
                   'StartSel=<mark>,StopSel=</mark>,MaxFragments=1') AS headline
FROM message m,
     websearch_to_tsquery('english', $1) AS q(query)
WHERE m.conversation_id = ANY($2)                      -- accessible set, bind as uuid[]
  AND m.is_deleted = false
  AND ($1 = '' OR m.search_tsv @@ q.query)             -- empty free-text => operator-only search
  AND ($3::uuid IS NULL OR m.sent_by = $3)             -- from:
  AND (NOT $4 OR m.text ~* 'https?://')                -- has:link
  AND (NOT $5 OR m.has_attachments = true)             -- has:attachment
  AND ($6::timestamptz IS NULL OR m.sent_at >= $6)     -- after:
  AND ($7::timestamptz IS NULL OR m.sent_at <  $7)     -- before:
  AND (<cursor predicate on (sent_at,id) per dir>)     -- see docs/pagination.md rules
ORDER BY m.sent_at DESC, m.id DESC
LIMIT $8 + 1;                                          -- fetch limit+1 to detect next page
```
Notes:
- Order by `(sent_at DESC, id DESC)` — **recency**, matching the messages endpoint, so the existing
  `(sent_at, message_id)` cursor contract applies unchanged. `rank` is returned for display, not used
  as the sort key. See §6 for why relevance is not the cursor sort key.
- The cursor predicate and next/prev computation follow `docs/pagination.md` exactly (mirror
  `list_messages`' existing cursor handling in the same file).
- Bind `conversation_ids` as a Postgres `uuid[]` and use `= ANY($2)`. An **empty** array yields zero
  rows — that is the correct "nothing accessible / unknown from/in" outcome.
- `ts_headline` is somewhat expensive; it runs only over the page (LIMIT+1 rows), which is fine.
  Keep `MaxFragments=1`. If clippy/perf review objects, `headline` may be made opt-in — but include
  it by default; the frontend wants highlight snippets.

## §5 Permission filtering (security-critical — call out in review)
The repo method is deliberately visibility-agnostic: it searches **only** the `conversation_ids` the
service hands it. The service is the sole authority and MUST derive that set from
`list_accessible_conversation_ids` (+ the `ensure_conversation_member_access` gate for an explicit
scope), exactly as the current workspace-wide search does. Explicit tests (§7) must prove:
- a message in a DM/group the actor is **not** a member of never appears, even on an exact-term match;
- `in:#channel` for a channel the actor can't see returns empty, not an error (no existence leak);
- `from:@user` does not widen visibility (still constrained to the accessible set).
Do not add a "search all messages" repo path. There is no admin bypass in this spec.

## §6 GraphQL contract + SDL delta

**Breaking change (coordinate with vision via nick-fury; frontend must re-run codegen).**
`searchMessages` moves from a flat list to a ranked, cursor-paginated connection.

`rim_api/src/types.rs` — new/changed types (match existing derive conventions; reuse the message
cursor types):
```graphql
input SearchMessagesInput {
  serverId: ID!
  query: String!
  conversationId: ID          # optional explicit single-conversation scope
  limit: Int                  # clamped server-side (default 25, max = MAX_PAGE_LIMIT)
  cursor: MessageCursorInput  # reuse existing (sentAt, messageId)
  dir: PageDir                # reuse existing Before/After; default After
}

type SearchResultMessage {    # extend the existing type
  id: ID!
  serverId: ID!
  conversationId: ID!
  conversationTitle: String
  conversationType: ConversationTypeEnum!
  threadId: ID
  rootMessageId: ID
  authorId: ID!
  text: String
  sentAt: DateTime!
  rank: Float!                # NEW — ts_rank score
  headline: String            # NEW — ts_headline snippet with <mark> highlights
  author: User                # existing ComplexObject resolver (DataLoader)
}

type SearchResultPage {       # NEW — mirrors MessagePage
  items: [SearchResultMessage!]!
  next: MessageCursorOutput
  prev: MessageCursorOutput
  pageInfo: PageInfo!
}
```
Resolver (`rim_api/src/query.rs`): `async fn search_messages(&self, ctx, input: SearchMessagesInput)
-> Result<SearchResultPage>`. Build the service `SearchMessagesQuery`, call the service, map the
`Page` → `SearchResultPage` via a `from_page`-style constructor (mirror `MessagePage::from_page` at
`types.rs:1377`). Keep the `#[graphql(complexity = N)]` and `#[ComplexObject] author` pattern.

**Removed from SDL:** the old `searchMessages(serverId, conversationId, query, limit,
includeDeleted): [SearchResultMessage!]!` field signature (the args `limit`/`includeDeleted` as
top-level and the list return). **Added:** `SearchMessagesInput`, `SearchResultPage`, `rank`,
`headline`. Hand nick-fury this delta for vision so the frontend `searchMessages` call + result
handling updates in lockstep. Regenerate `schema.graphql` and note it in the handoff.

**Why recency-cursor, not relevance-cursor (document this):** cursor pagination requires a stable
total order with a unique tie-breaker (`docs/pagination.md`). `ts_rank` is neither unique nor stable
across concurrent inserts, so paginating by rank produces skips/dupes. We therefore paginate by
`(sent_at, id)` and expose `rank` for display/secondary client sort. A dedicated relevance-ranked
top-N query (no pagination) can be added for Track C grounded-recall if needed — out of scope here.

## §7 Tests (hulk writes)
- **Parser unit tests** (`search_query.rs`, no DB): each operator (`from:@u`, `from:<uuid>`, `in:#c`,
  `in:<uuid>`, `has:link`, `has:attachment`, `before:`, `after:`), mixed free-text + operators,
  bad date → Validation, unknown `has:` → Validation, unknown `key:` → treated as free text,
  over-length → Validation, operator-only (empty free text) parses with empty `text`.
- **Service unit tests** (mocked repo/user/conversation): unknown `from:`/`in:` → empty page (no
  error); explicit `conversationId` runs the access gate; `in:` intersects the accessible set.
- **db-tests** (`rim_infra_db --features db-tests`, using the now-loud `db_conn()`):
  1. FTS matches on stemmed terms (e.g. query `deploy` matches "deploying") — proves it's tsvector,
     not LIKE.
  2. Ranking present and `rank > 0` for matches; ordering is by `(sent_at DESC, id DESC)`.
  3. **Permission isolation** (the security tests in §5): non-member DM excluded; hidden channel
     `in:` returns empty; `from:` doesn't widen visibility.
  4. Operator filters: `has:link`, `has:attachment`, `before:`/`after:` each constrain results.
  5. Cursor pagination: page 1 + page 2 via `next` cover the full ordered set with no gap/overlap
     (mirror the existing `list_messages` pagination db-test).
  6. Deleted messages never returned.
- Ensure `message` (and any new tables — none here) is in the db-tests TRUNCATE/reset list (it
  already is via `reset_db`).

## §8 Docs
Doc comments on: the migration (ops note above), `ParsedSearchQuery`/`parse_search_query` (+ the
"why not lens FilterSpec" rationale), `MessageSearchParams`/`MessageSearchHit`/`search_messages_fts`
(visibility-agnostic contract), the resolver (breaking-change + recency-cursor rationale). Update
CLAUDE.md's search note if one exists. Add a short `docs/search.md` describing the operator syntax
and the recency-cursor/ranking decision (frontend + product will want it).

## Rollback
Forward-only in prod. `down` drops the index + generated column. The service/repo/GraphQL changes
revert as code. No data loss on rollback beyond the search index (rebuildable). Flag the table-lock
cost of the `up` migration in the deploy notes.
