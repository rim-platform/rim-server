//! Through-the-stack e2e tests for messaging and cursor pagination.
//!
//! Every assertion goes through GraphQL: resolver → `MessageService` → `SeaOrmMessageRepository`
//! → Postgres, which is the path the client uses. The cursor contract under test:
//!
//! - `messages` / `threadMessages` are `DESC` endpoints (`sent_at DESC, id DESC`) whose items are
//!   returned in display order, oldest-first. `AFTER` walks toward older messages, `BEFORE` toward
//!   newer ones.
//! - `searchMessages` is the same `DESC` endpoint but returns items newest-first.
//! - `next` continues in the request's `dir`; `prev` goes back the opposite way. A page's rows
//!   are the ones adjacent to the cursor, so following `next` until it is null visits every row
//!   exactly once, in either direction.
//!
//! Run with:
//! ```sh
//! cargo test -p rim_api --features e2e-tests -- --test-threads=1
//! ```
#![cfg(feature = "e2e-tests")]

mod support;

use std::collections::HashSet;

use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use uuid::Uuid;

use rim_api::AppSchema;
use rim_core::server_member::model::ServerMemberRole;
use rim_services::auth::dto::RegistrationMode;

use support::graphql::{
    Caller, anonymous_request, assert_each_refused, assert_each_refused_as, exec_err_code, exec_ok,
    merged,
};
use support::roles::create_channel;
use support::roots::{ATTACHMENT_ROOTS, CONVERSATION_MESSAGE_ROOTS, MESSAGE_ROOTS, Targets};
use support::{TestCtx, unknown_id};

const MESSAGE_FIELDS: &str = r#"
fragment MessageFields on Message {
  id text sentAt isDeleted threadId rootThreadId authorId
}
"#;

const PAGE_FIELDS: &str = r#"
fragment PageFields on MessagePage {
  items { ...MessageFields }
  next { sentAt messageId }
  prev { sentAt messageId }
  pageInfo { hasNext hasPrev }
}
"#;

const MESSAGES_QUERY: &str = r#"
query Messages($serverId: ID!, $conversationId: ID!, $dir: PageDir!,
               $cursor: MessageCursorInput, $limit: Int!, $includeDeleted: Boolean!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: $dir,
                 cursor: $cursor, limit: $limit, includeDeleted: $includeDeleted) {
    ...PageFields
  }
}
"#;

const THREAD_MESSAGES_QUERY: &str = r#"
query ThreadMessages($serverId: ID!, $conversationId: ID!, $threadId: ID!, $dir: PageDir!,
                     $cursor: MessageCursorInput, $limit: Int!, $includeDeleted: Boolean!) {
  page: threadMessages(serverId: $serverId, conversationId: $conversationId,
                       threadId: $threadId, dir: $dir, cursor: $cursor, limit: $limit,
                       includeDeleted: $includeDeleted) {
    ...PageFields
  }
}
"#;

const SEARCH_QUERY: &str = r#"
query Search($input: SearchMessagesInput!) {
  page: searchMessages(input: $input) {
    items { id text sentAt }
    next { sentAt messageId }
    prev { sentAt messageId }
    pageInfo { hasNext hasPrev }
  }
}
"#;

const SEND_MUTATION: &str = r#"
mutation Send($input: SendMessageInput!) {
  msg: sendMessage(input: $input) { ...MessageFields }
}
"#;

const REPLY_MUTATION: &str = r#"
mutation Reply($input: ReplyToMessageInput!) {
  msg: replyToMessage(input: $input) { ...MessageFields }
}
"#;

const EDIT_MUTATION: &str = r#"
mutation Edit($input: EditMessageInput!) {
  msg: editMessage(input: $input) { ...MessageFields }
}
"#;

const DELETE_MUTATION: &str = r#"
mutation Delete($input: DeleteMessageInput!) { deleteMessage(input: $input) }
"#;

const CREATE_CONVERSATION_MUTATION: &str = r#"
mutation CreateConversation($input: CreateConversationInput!) {
  conversation: createConversation(input: $input) { id }
}
"#;

/// The `(timestamp, id)` sort key every message cursor is built from.
type Key = (DateTime<Utc>, Uuid);

#[derive(Clone, Debug)]
struct Msg {
    id: String,
    text: Option<String>,
    sent_at: DateTime<Utc>,
    is_deleted: bool,
    thread_id: Option<String>,
    root_thread_id: Option<String>,
}

impl Msg {
    fn from_json(v: &Value) -> Self {
        let opt_str = |field: &str| v[field].as_str().map(str::to_string);
        Self {
            id: opt_str("id").expect("message id"),
            text: opt_str("text"),
            sent_at: parse_ts(&v["sentAt"]),
            is_deleted: v["isDeleted"].as_bool().unwrap_or(false),
            thread_id: opt_str("threadId"),
            root_thread_id: opt_str("rootThreadId"),
        }
    }

    fn key(&self) -> Key {
        (self.sent_at, parse_uuid(&self.id))
    }

    /// The cursor a client would build for this message.
    fn cursor(&self) -> Value {
        json!({ "sentAt": self.sent_at.to_rfc3339(), "messageId": self.id })
    }
}

#[derive(Debug)]
struct Page {
    items: Vec<Msg>,
    next: Option<Key>,
    prev: Option<Key>,
    next_json: Value,
    prev_json: Value,
    has_next: bool,
    has_prev: bool,
}

impl Page {
    fn from_json(v: &Value) -> Self {
        let cursor_key = |c: &Value| {
            (!c.is_null()).then(|| {
                (
                    parse_ts(&c["sentAt"]),
                    parse_uuid(c["messageId"].as_str().expect("cursor messageId")),
                )
            })
        };
        Self {
            items: v["items"]
                .as_array()
                .expect("page items")
                .iter()
                .map(Msg::from_json)
                .collect(),
            next: cursor_key(&v["next"]),
            prev: cursor_key(&v["prev"]),
            next_json: v["next"].clone(),
            prev_json: v["prev"].clone(),
            has_next: v["pageInfo"]["hasNext"].as_bool().expect("hasNext"),
            has_prev: v["pageInfo"]["hasPrev"].as_bool().expect("hasPrev"),
        }
    }

    fn ids(&self) -> Vec<String> {
        self.items.iter().map(|m| m.id.clone()).collect()
    }
}

fn parse_ts(v: &Value) -> DateTime<Utc> {
    DateTime::parse_from_rfc3339(v.as_str().expect("timestamp string"))
        .expect("RFC 3339 timestamp")
        .with_timezone(&Utc)
}

fn parse_uuid(s: &str) -> Uuid {
    Uuid::parse_str(s).expect("UUID")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Dir {
    After,
    Before,
}

impl Dir {
    fn gql(self) -> &'static str {
        match self {
            Dir::After => "AFTER",
            Dir::Before => "BEFORE",
        }
    }

    fn opposite(self) -> Self {
        match self {
            Dir::After => Dir::Before,
            Dir::Before => Dir::After,
        }
    }
}

/// Which paginated list a request targets.
#[derive(Clone, Debug)]
enum Target {
    Timeline {
        conversation_id: String,
    },
    Thread {
        conversation_id: String,
        thread_id: String,
    },
    Search {
        conversation_id: String,
        query: String,
    },
}

impl Target {
    /// Whether items come back newest-first (`searchMessages`) rather than oldest-first.
    fn newest_first(&self) -> bool {
        matches!(self, Target::Search { .. })
    }
}

struct Fixture {
    ctx: TestCtx,
    schema: AppSchema,
    server_id: String,
    channel_id: String,
    alice: Caller,
    bob: Caller,
    /// Server member who is not part of the DM.
    carol: Caller,
    /// Not a member of the server at all.
    outsider: Caller,
}

/// A workspace with three members (alice owns it), one non-member, and a channel.
async fn setup() -> Fixture {
    let ctx = TestCtx::new().await;
    let server = ctx.create_server("messaging-e2e").await;
    let alice_user = ctx.create_user("alice").await;
    let bob_user = ctx.create_user("bob").await;
    let carol_user = ctx.create_user("carol").await;
    let outsider_user = ctx.create_user("outsider").await;
    ctx.add_owner(&server, &alice_user).await;
    ctx.add_member(&server, &bob_user, ServerMemberRole::Member)
        .await;
    ctx.add_member(&server, &carol_user, ServerMemberRole::Member)
        .await;

    let schema = ctx.build_schema(RegistrationMode::InviteOnly).await;
    let alice = ctx.caller(&alice_user).await;
    let bob = ctx.caller(&bob_user).await;
    let carol = ctx.caller(&carol_user).await;
    let outsider = ctx.caller(&outsider_user).await;
    let server_id = server.id.to_string();

    let channel_id = create_channel(&schema, &alice, &server_id, "general", false).await;

    Fixture {
        ctx,
        schema,
        server_id,
        channel_id,
        alice,
        bob,
        carol,
        outsider,
    }
}

impl Fixture {
    /// Append the fragments `query` uses (GraphQL rejects unused fragments).
    fn with_fragments(query: &str) -> String {
        if query.contains("...PageFields") {
            format!("{query}\n{PAGE_FIELDS}\n{MESSAGE_FIELDS}")
        } else {
            format!("{query}\n{MESSAGE_FIELDS}")
        }
    }

    /// Send a message and wait a little so the next one gets a strictly later `sent_at`. The
    /// assertions below order by the full `(sent_at, id)` key anyway, so a tie would still be
    /// handled per the contract; the pause just keeps send order and key order identical.
    async fn send(&self, who: &Caller, conversation_id: &str, text: &str) -> Msg {
        let data = exec_ok(
            &self.schema,
            who.request(
                &Self::with_fragments(SEND_MUTATION),
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": conversation_id,
                    "text": text,
                } }),
            ),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        Msg::from_json(&data["msg"])
    }

    async fn reply(&self, who: &Caller, root: &Msg, text: &str) -> Msg {
        let data = exec_ok(
            &self.schema,
            who.request(
                &Self::with_fragments(REPLY_MUTATION),
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": self.channel_id,
                    "rootMessageId": root.id,
                    "text": text,
                } }),
            ),
        )
        .await;
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
        Msg::from_json(&data["msg"])
    }

    /// Send `count` channel messages, alternating alice/bob, with texts `"{prefix} {i}"`.
    async fn send_many(&self, count: usize, prefix: &str) -> Vec<Msg> {
        let mut sent = Vec::with_capacity(count);
        for i in 1..=count {
            let who = if i % 2 == 1 { &self.alice } else { &self.bob };
            let msg = self
                .send(who, &self.channel_id, &format!("{prefix} {i}"))
                .await;
            assert_eq!(msg.text.as_deref(), Some(format!("{prefix} {i}").as_str()));
            sent.push(msg);
        }
        sent
    }

    fn page_request(
        &self,
        who: &Caller,
        target: &Target,
        dir: Dir,
        cursor: &Value,
        limit: i64,
        include_deleted: bool,
    ) -> async_graphql::Request {
        match target {
            Target::Timeline { conversation_id } => who.request(
                &Self::with_fragments(MESSAGES_QUERY),
                json!({
                    "serverId": self.server_id,
                    "conversationId": conversation_id,
                    "dir": dir.gql(),
                    "cursor": cursor,
                    "limit": limit,
                    "includeDeleted": include_deleted,
                }),
            ),
            Target::Thread {
                conversation_id,
                thread_id,
            } => who.request(
                &Self::with_fragments(THREAD_MESSAGES_QUERY),
                json!({
                    "serverId": self.server_id,
                    "conversationId": conversation_id,
                    "threadId": thread_id,
                    "dir": dir.gql(),
                    "cursor": cursor,
                    "limit": limit,
                    "includeDeleted": include_deleted,
                }),
            ),
            Target::Search {
                conversation_id,
                query,
            } => who.request(
                SEARCH_QUERY,
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": conversation_id,
                    "query": query,
                    "dir": dir.gql(),
                    "cursor": cursor,
                    "limit": limit,
                } }),
            ),
        }
    }

    async fn page(
        &self,
        target: &Target,
        dir: Dir,
        cursor: &Value,
        limit: i64,
        include_deleted: bool,
    ) -> Page {
        let req = self.page_request(&self.alice, target, dir, cursor, limit, include_deleted);
        let data = exec_ok(&self.schema, req).await;
        Page::from_json(&data["page"])
    }

    /// Request a first page in `dir` from `start`, then follow `next` until it is null.
    async fn walk(
        &self,
        target: &Target,
        dir: Dir,
        start: &Value,
        limit: i64,
        include_deleted: bool,
    ) -> Vec<Page> {
        let mut pages = Vec::new();
        let mut cursor = start.clone();
        loop {
            let page = self
                .page(target, dir, &cursor, limit, include_deleted)
                .await;
            let next = page.next_json.clone();
            pages.push(page);
            if next.is_null() {
                return pages;
            }
            assert!(pages.len() <= 50, "walk did not terminate");
            cursor = next;
        }
    }
}

/// A cursor ordered before every real message: `BEFORE`-walking from it (toward newer
/// messages) must visit the whole list starting at the oldest message.
fn before_everything() -> Value {
    json!({ "sentAt": "1970-01-01T00:00:00Z", "messageId": Uuid::nil().to_string() })
}

/// The pages the contract prescribes for a walk over `all` (any order) in `dir`, starting from
/// `start` (`None` = no cursor, only meaningful for `AFTER`, which then starts at the newest).
///
/// Each page holds the `limit` rows adjacent to the previous boundary — nothing skipped —
/// listed in the endpoint's display order.
fn expected_pages(
    all: &[Msg],
    dir: Dir,
    start: Option<Key>,
    limit: usize,
    newest_first: bool,
) -> Vec<Vec<Msg>> {
    let mut sorted = all.to_vec();
    sorted.sort_by_key(Msg::key);
    // Walk order: AFTER moves toward older rows, BEFORE toward newer ones.
    let walk: Vec<Msg> = match (dir, start) {
        (Dir::After, None) => sorted.into_iter().rev().collect(),
        (Dir::After, Some(c)) => sorted.into_iter().rev().filter(|m| m.key() < c).collect(),
        (Dir::Before, Some(c)) => sorted.into_iter().filter(|m| m.key() > c).collect(),
        (Dir::Before, None) => panic!("the contract does not define a cursorless BEFORE walk"),
    };
    walk.chunks(limit)
        .map(|chunk| {
            let mut page = chunk.to_vec();
            page.sort_by_key(Msg::key);
            if newest_first {
                page.reverse();
            }
            page
        })
        .collect()
}

fn ids(msgs: &[Msg]) -> Vec<String> {
    msgs.iter().map(|m| m.id.clone()).collect()
}

/// Assert a walk produced by [`Fixture::walk`] matches the contract exactly.
fn assert_walk(
    label: &str,
    pages: &[Page],
    all: &[Msg],
    dir: Dir,
    start: Option<Key>,
    limit: usize,
    newest_first: bool,
) {
    let expected = expected_pages(all, dir, start, limit, newest_first);
    let got: Vec<Vec<String>> = pages.iter().map(Page::ids).collect();
    let want: Vec<Vec<String>> = expected.iter().map(|p| ids(p)).collect();
    // An empty list is still answered with one (empty) page.
    let want = if want.is_empty() { vec![vec![]] } else { want };
    assert_eq!(
        got, want,
        "{label}: pages differ from the contract (dir {dir:?}, page size {limit})"
    );

    // Every expected row exactly once.
    let mut seen = HashSet::new();
    for id in got.iter().flatten() {
        assert!(seen.insert(id.clone()), "{label}: {id} appeared twice");
    }

    for (i, (page, want)) in pages.iter().zip(&expected).enumerate() {
        let is_last = i + 1 == expected.len();
        // Walk order within this page: AFTER newest→oldest, BEFORE oldest→newest.
        let mut walk_order = want.clone();
        walk_order.sort_by_key(Msg::key);
        if dir == Dir::After {
            walk_order.reverse();
        }
        let first_in_walk = walk_order.first().map(Msg::key);
        let last_in_walk = walk_order.last().map(Msg::key);

        // `next` continues from the far edge of the page; null once the list is exhausted.
        let want_next = if is_last { None } else { last_in_walk };
        assert_eq!(page.next, want_next, "{label}: page {i} `next`");
        assert_eq!(
            page.has_next,
            want_next.is_some(),
            "{label}: page {i} hasNext"
        );

        // `prev` points back from the near edge — only when the page was reached via a cursor
        // (a cursorless first page starts at the boundary, so there is nothing behind it).
        let reached_by_cursor = i > 0 || start.is_some();
        let want_prev = if reached_by_cursor {
            first_in_walk
        } else {
            None
        };
        assert_eq!(page.prev, want_prev, "{label}: page {i} `prev`");
        assert_eq!(
            page.has_prev,
            want_prev.is_some(),
            "{label}: page {i} hasPrev"
        );
    }
}

/// Following page k's `prev` in the opposite direction lands exactly on page k-1.
async fn assert_prev_round_trips(
    f: &Fixture,
    label: &str,
    target: &Target,
    pages: &[Page],
    dir: Dir,
    limit: i64,
    include_deleted: bool,
) {
    for k in 1..pages.len() {
        let back = f
            .page(
                target,
                dir.opposite(),
                &pages[k].prev_json,
                limit,
                include_deleted,
            )
            .await;
        assert_eq!(
            back.ids(),
            pages[k - 1].ids(),
            "{label}: following page {k}'s `prev` with {:?} must return page {}",
            dir.opposite(),
            k - 1
        );
    }
}

/// Run the full matrix on one list: `AFTER` from no cursor, `BEFORE` from before the oldest
/// row, `BEFORE` from the cursor the `AFTER` walk ends on, and the `prev` round trips of each.
async fn assert_pagination_matrix(
    f: &Fixture,
    label: &str,
    target: &Target,
    all: &[Msg],
    limit: i64,
    include_deleted: bool,
) {
    let newest_first = target.newest_first();
    let size = limit as usize;

    let after = f
        .walk(target, Dir::After, &Value::Null, limit, include_deleted)
        .await;
    assert_walk(
        &format!("{label} AFTER"),
        &after,
        all,
        Dir::After,
        None,
        size,
        newest_first,
    );
    assert_prev_round_trips(f, label, target, &after, Dir::After, limit, include_deleted).await;

    let before = f
        .walk(
            target,
            Dir::Before,
            &before_everything(),
            limit,
            include_deleted,
        )
        .await;
    let origin = (DateTime::<Utc>::UNIX_EPOCH, Uuid::nil());
    assert_walk(
        &format!("{label} BEFORE from the start"),
        &before,
        all,
        Dir::Before,
        Some(origin),
        size,
        newest_first,
    );
    assert_prev_round_trips(
        f,
        label,
        target,
        &before,
        Dir::Before,
        limit,
        include_deleted,
    )
    .await;

    // Turning around at the oldest row: the last AFTER page's `prev` points at the oldest row;
    // walking BEFORE from it must visit everything newer, starting right next to it, not jump
    // to the newest rows and skip the ones in between.
    let last = after.last().expect("at least one page");
    if let Some(turn) = last.prev {
        let back = f
            .walk(target, Dir::Before, &last.prev_json, limit, include_deleted)
            .await;
        assert_walk(
            &format!("{label} BEFORE from the AFTER walk's last `prev`"),
            &back,
            all,
            Dir::Before,
            Some(turn),
            size,
            newest_first,
        );
    }
}

/// The channel timeline, paged 3 at a time in every direction the schema exposes.
#[tokio::test]
async fn timeline_pagination_visits_every_message_exactly_once() {
    let f = setup().await;
    let sent = f.send_many(7, "timeline message").await;
    let timeline = Target::Timeline {
        conversation_id: f.channel_id.clone(),
    };

    assert_pagination_matrix(&f, "timeline", &timeline, &sent, 3, false).await;

    // Page size larger than the list: one page, both cursors null.
    let whole = f.page(&timeline, Dir::After, &Value::Null, 50, false).await;
    assert_eq!(whole.ids(), ids(&sent), "one big page, oldest-first");
    assert!(whole.next.is_none() && whole.prev.is_none());

    // The client opens the list newest-first and loads older history on scroll-up.
    // Opening request: the newest page, oldest-first within it.
    let newest = ids(&sent[4..7]);
    let open_before = f.page(&timeline, Dir::Before, &Value::Null, 3, false).await;
    assert_eq!(
        open_before.ids(),
        newest,
        "cursorless BEFORE opens on the newest page"
    );
    let open = f.page(&timeline, Dir::After, &Value::Null, 3, false).await;
    assert_eq!(
        open.ids(),
        newest,
        "cursorless AFTER opens on the newest page"
    );
    // Scroll-up: on this DESC endpoint older history is `AFTER` + `page.next`, which must yield
    // the next-older messages, not an empty page.
    let older = f
        .page(&timeline, Dir::After, &open.next_json, 3, false)
        .await;
    assert_eq!(
        older.ids(),
        ids(&sent[1..4]),
        "scroll-up loads the next-older page"
    );
    // `BEFORE` is the newer direction: nothing is newer than the newest
    // message, so `BEFORE` from it is empty. A client that scrolls up with `BEFORE` dead-ends.
    let newer_than_newest = f
        .page(&timeline, Dir::Before, &sent[6].cursor(), 3, false)
        .await;
    assert!(
        newer_than_newest.items.is_empty(),
        "BEFORE the newest message must be empty, got {:?}",
        newer_than_newest.ids()
    );
    assert!(newer_than_newest.next.is_none() && newer_than_newest.prev.is_none());
}

/// Replies live in their thread, page like the timeline, and stay out of the root timeline.
#[tokio::test]
async fn thread_pagination_and_timeline_isolation() {
    let f = setup().await;
    let sent = f.send_many(7, "root message").await;
    let root = &sent[2];

    let r1 = f.reply(&f.bob, root, "first reply").await;
    let r2 = f.reply(&f.alice, root, "second reply").await;
    let thread_id = r1.thread_id.clone().expect("a reply belongs to a thread");
    assert_eq!(r2.thread_id.as_deref(), Some(thread_id.as_str()));
    assert_eq!(r2.text.as_deref(), Some("second reply"));

    let thread = Target::Thread {
        conversation_id: f.channel_id.clone(),
        thread_id: thread_id.clone(),
    };
    let replies = vec![r1.clone(), r2.clone()];
    assert_pagination_matrix(&f, "thread", &thread, &replies, 1, false).await;

    // The root timeline holds exactly the 7 root messages, with the thread linked from its root.
    let timeline = f
        .page(
            &Target::Timeline {
                conversation_id: f.channel_id.clone(),
            },
            Dir::After,
            &Value::Null,
            50,
            false,
        )
        .await;
    assert_eq!(
        timeline.ids(),
        ids(&sent),
        "replies must not leak into the timeline"
    );
    for m in &timeline.items {
        assert!(m.thread_id.is_none(), "{} is a timeline message", m.id);
        let want = (m.id == root.id).then(|| thread_id.clone());
        assert_eq!(m.root_thread_id, want, "rootThreadId of {}", m.id);
    }
}

/// `editMessage` returns and persists the new text; `deleteMessage` is a soft delete that drops
/// the message from the default timeline and shows it as a text-less tombstone when
/// `includeDeleted` is set.
#[tokio::test]
async fn edit_and_delete_are_reflected_in_the_timeline() {
    let f = setup().await;
    let sent = f.send_many(7, "editable message").await;
    let timeline = Target::Timeline {
        conversation_id: f.channel_id.clone(),
    };
    let edited = &sent[0]; // alice's
    let deleted = &sent[3]; // bob's

    let data = exec_ok(
        &f.schema,
        f.alice.request(
            &Fixture::with_fragments(EDIT_MUTATION),
            json!({ "input": {
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "messageId": edited.id,
                "text": "edited text",
            } }),
        ),
    )
    .await;
    let edit_result = Msg::from_json(&data["msg"]);
    assert_eq!(edit_result.id, edited.id);
    assert_eq!(edit_result.text.as_deref(), Some("edited text"));
    assert_eq!(
        edit_result.sent_at, edited.sent_at,
        "editing keeps the sort key"
    );

    // Only the author may edit.
    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            &Fixture::with_fragments(EDIT_MUTATION),
            json!({ "input": {
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "messageId": edited.id,
                "text": "hijacked",
            } }),
        ),
    )
    .await;
    assert_eq!(code, "FORBIDDEN");

    let delete_vars = json!({ "input": {
        "serverId": f.server_id,
        "conversationId": f.channel_id,
        "messageId": deleted.id,
    } });
    // Carol is an ordinary member: only the author or an Owner/Moderator may delete
    // (moderator deletes are covered in `moderation_e2e.rs`).
    let code = exec_err_code(
        &f.schema,
        f.carol.request(DELETE_MUTATION, delete_vars.clone()),
    )
    .await;
    assert_eq!(
        code, "FORBIDDEN",
        "a plain member may not delete others' messages"
    );
    let data = exec_ok(&f.schema, f.bob.request(DELETE_MUTATION, delete_vars)).await;
    assert_eq!(data["deleteMessage"], json!(true));

    // Default timeline: the deleted message is gone, the edit is visible, and pagination still
    // visits every remaining message exactly once.
    let remaining: Vec<Msg> = sent
        .iter()
        .filter(|m| m.id != deleted.id)
        .cloned()
        .collect();
    assert_pagination_matrix(
        &f,
        "timeline without deleted",
        &timeline,
        &remaining,
        3,
        false,
    )
    .await;
    let visible = f.page(&timeline, Dir::After, &Value::Null, 50, false).await;
    assert_eq!(visible.ids(), ids(&remaining));
    assert_eq!(visible.items[0].text.as_deref(), Some("edited text"));
    assert!(visible.items.iter().all(|m| !m.is_deleted));

    // includeDeleted: the full list again, with the deleted message as a tombstone.
    assert_pagination_matrix(&f, "timeline with deleted", &timeline, &sent, 3, true).await;
    let all = f.page(&timeline, Dir::After, &Value::Null, 50, true).await;
    assert_eq!(all.ids(), ids(&sent));
    let tombstone = &all.items[3];
    assert!(tombstone.is_deleted);
    assert_eq!(tombstone.text, None, "a deleted message exposes no text");

    // A deleted message cannot be edited.
    let code = exec_err_code(
        &f.schema,
        f.bob.request(
            &Fixture::with_fragments(EDIT_MUTATION),
            json!({ "input": {
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "messageId": deleted.id,
                "text": "resurrected",
            } }),
        ),
    )
    .await;
    assert_eq!(code, "VALIDATION");
}

/// `searchMessages` shares the cursor contract (newest-first items).
#[tokio::test]
async fn search_pagination_visits_every_match_exactly_once() {
    let f = setup().await;
    let hits = f.send_many(7, "pagination needle").await;
    // Non-matching noise in the same channel must not show up or disturb the cursors.
    f.send(&f.bob, &f.channel_id, "unrelated chatter").await;

    let search = Target::Search {
        conversation_id: f.channel_id.clone(),
        query: "needle".to_string(),
    };
    assert_pagination_matrix(&f, "search", &search, &hits, 3, false).await;
}

/// Members of neither the workspace nor the DM cannot page its messages.
#[tokio::test]
async fn non_members_cannot_page_messages() {
    let f = setup().await;
    let sent = f.send_many(2, "private message").await;
    let r1 = f.reply(&f.bob, &sent[0], "thread reply").await;
    let thread_id = r1.thread_id.expect("thread id");

    let channel = Target::Timeline {
        conversation_id: f.channel_id.clone(),
    };
    let thread = Target::Thread {
        conversation_id: f.channel_id.clone(),
        thread_id,
    };
    for target in [&channel, &thread] {
        for dir in [Dir::After, Dir::Before] {
            let req = f.page_request(&f.outsider, target, dir, &Value::Null, 3, false);
            assert_eq!(
                exec_err_code(&f.schema, req).await,
                "FORBIDDEN",
                "non-member paging {target:?} {dir:?}"
            );
        }
    }
    // Following a cursor leaked from a member's page does not help.
    let req = f.page_request(
        &f.outsider,
        &channel,
        Dir::Before,
        &sent[0].cursor(),
        3,
        false,
    );
    assert_eq!(exec_err_code(&f.schema, req).await, "FORBIDDEN");

    // A DM between alice and bob: carol is in the workspace but not in the DM.
    let data = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CONVERSATION_MUTATION,
            json!({ "input": { "serverId": f.server_id, "otherUserId": f.bob.user_id.to_string() } }),
        ),
    )
    .await;
    let dm_id = data["conversation"]["id"]
        .as_str()
        .expect("dm id")
        .to_string();
    let dm_msg = f.send(&f.alice, &dm_id, "just between us").await;
    let dm = Target::Timeline {
        conversation_id: dm_id.clone(),
    };

    let req = f.page_request(&f.bob, &dm, Dir::After, &Value::Null, 3, false);
    let page = Page::from_json(&exec_ok(&f.schema, req).await["page"]);
    assert_eq!(
        page.ids(),
        vec![dm_msg.id.clone()],
        "DM members can read it"
    );

    for who in [&f.carol, &f.outsider] {
        let req = f.page_request(who, &dm, Dir::After, &Value::Null, 3, false);
        assert_eq!(exec_err_code(&f.schema, req).await, "FORBIDDEN");
    }
    let code = exec_err_code(
        &f.schema,
        f.carol.request(
            &Fixture::with_fragments(SEND_MUTATION),
            json!({ "input": { "serverId": f.server_id, "conversationId": dm_id, "text": "hi" } }),
        ),
    )
    .await;
    assert_eq!(
        code, "FORBIDDEN",
        "non-members cannot post into the DM either"
    );
}

// ── Message change feed ─────────────────────────────────────────────────────

const CHANGES_QUERY: &str = r#"
query Changes($input: MessageChangesInput!) {
  feed: messageChanges(input: $input) {
    conversationId status cursor hasMore
    changes { kind version editedAt message { ...MessageFields } }
  }
}
"#;

/// One feed entry as a client sees it.
#[derive(Debug)]
struct Change {
    kind: String,
    version: i64,
    edited: bool,
    message: Msg,
}

/// One conversation's slice of a `messageChanges` response.
#[derive(Debug)]
struct ConversationChanges {
    conversation_id: String,
    status: String,
    cursor: String,
    has_more: bool,
    changes: Vec<Change>,
}

impl ConversationChanges {
    fn from_json(v: &Value) -> Self {
        Self {
            conversation_id: v["conversationId"].as_str().expect("id").to_string(),
            status: v["status"].as_str().expect("status").to_string(),
            cursor: v["cursor"].as_str().expect("cursor").to_string(),
            has_more: v["hasMore"].as_bool().expect("hasMore"),
            changes: v["changes"]
                .as_array()
                .expect("changes")
                .iter()
                .map(|c| Change {
                    kind: c["kind"].as_str().expect("kind").to_string(),
                    version: c["version"]
                        .as_str()
                        .expect("version")
                        .parse()
                        .expect("decimal version"),
                    edited: !c["editedAt"].is_null(),
                    message: Msg::from_json(&c["message"]),
                })
                .collect(),
        }
    }

    fn summary(&self) -> Vec<(String, String)> {
        self.changes
            .iter()
            .map(|c| (c.message.id.clone(), c.kind.clone()))
            .collect()
    }
}

impl Fixture {
    /// `messageChanges` for `(conversation_id, after)` pairs, as `who`.
    fn changes_request(
        &self,
        who: &Caller,
        conversations: &[(&str, Option<&str>)],
        limit: i64,
    ) -> async_graphql::Request {
        let conversations: Vec<Value> = conversations
            .iter()
            .map(|(id, after)| json!({ "conversationId": id, "after": after }))
            .collect();
        who.request(
            &Self::with_fragments(CHANGES_QUERY),
            json!({ "input": {
                "serverId": self.server_id,
                "conversations": conversations,
                "limit": limit,
            } }),
        )
    }

    async fn changes(
        &self,
        conversations: &[(&str, Option<&str>)],
        limit: i64,
    ) -> Vec<ConversationChanges> {
        let data = exec_ok(
            &self.schema,
            self.changes_request(&self.alice, conversations, limit),
        )
        .await;
        data["feed"]
            .as_array()
            .expect("feed")
            .iter()
            .map(ConversationChanges::from_json)
            .collect()
    }

    /// Follow one conversation's feed from `after` until `hasMore` is false, the way a client
    /// catches up. Returns every change and the cursor to store.
    async fn sync(
        &self,
        conversation_id: &str,
        after: Option<&str>,
        limit: i64,
    ) -> (Vec<Change>, String) {
        let mut all = Vec::new();
        let mut cursor = after.map(str::to_string);
        for _ in 0..50 {
            let mut feed = self
                .changes(&[(conversation_id, cursor.as_deref())], limit)
                .await;
            let page = feed.pop().expect("one conversation");
            assert!(page.changes.len() <= limit as usize);
            all.extend(page.changes);
            cursor = Some(page.cursor);
            if !page.has_more {
                return (all, cursor.expect("cursor"));
            }
        }
        panic!("sync did not terminate");
    }

    async fn edit(&self, who: &Caller, msg: &Msg, text: &str) {
        exec_ok(
            &self.schema,
            who.request(
                &Self::with_fragments(EDIT_MUTATION),
                json!({ "input": {
                    "serverId": self.server_id,
                    "conversationId": self.channel_id,
                    "messageId": msg.id,
                    "text": text,
                } }),
            ),
        )
        .await;
    }
}

/// A client syncs from scratch, stores the cursor, and later learns about edits and deletes
/// (timeline and thread alike) without re-reading the conversation.
#[tokio::test]
async fn change_feed_reports_creates_edits_and_deletes_since_the_cursor() {
    let f = setup().await;
    let sent = f.send_many(3, "feed message").await;
    let reply = f.reply(&f.bob, &sent[0], "feed reply").await;

    // Initial sync from the beginning, two changes per page.
    let (initial, cursor) = f.sync(&f.channel_id, None, 2).await;
    let created: Vec<(String, String)> = initial
        .iter()
        .map(|c| (c.message.id.clone(), c.kind.clone()))
        .collect();
    let expected: Vec<(String, String)> = sent
        .iter()
        .chain(std::iter::once(&reply))
        .map(|m| (m.id.clone(), "CREATED".to_string()))
        .collect();
    assert_eq!(created, expected, "every message once, in write order");
    assert!(initial.windows(2).all(|w| w[0].version < w[1].version));
    assert_eq!(
        initial[3].message.thread_id, reply.thread_id,
        "thread replies are in the feed"
    );
    assert!(initial.iter().all(|c| !c.edited));

    // Nothing changed: an empty page that hands back the same cursor.
    let idle = f.changes(&[(&f.channel_id, Some(&cursor))], 10).await;
    assert!(idle[0].changes.is_empty());
    assert!(!idle[0].has_more);
    assert_eq!(idle[0].cursor, cursor);

    // An edit, a delete, two edits of one message, and an edit of the thread reply.
    f.edit(&f.alice, &sent[0], "first, edited").await;
    let data = exec_ok(
        &f.schema,
        f.bob.request(
            DELETE_MUTATION,
            json!({ "input": {
                "serverId": f.server_id,
                "conversationId": f.channel_id,
                "messageId": sent[1].id,
            } }),
        ),
    )
    .await;
    assert_eq!(data["deleteMessage"], json!(true));
    f.edit(&f.alice, &sent[2], "third, draft").await;
    f.edit(&f.alice, &sent[2], "third, final").await;
    f.edit(&f.bob, &reply, "reply, edited").await;

    let (delta, next_cursor) = f.sync(&f.channel_id, Some(&cursor), 2).await;
    assert_eq!(
        delta
            .iter()
            .map(|c| (c.message.id.clone(), c.kind.clone()))
            .collect::<Vec<_>>(),
        vec![
            (sent[0].id.clone(), "EDITED".to_string()),
            (sent[1].id.clone(), "DELETED".to_string()),
            (sent[2].id.clone(), "EDITED".to_string()),
            (reply.id.clone(), "EDITED".to_string()),
        ],
        "each changed message once, at its newest change"
    );
    assert!(delta[0].version > initial[3].version);
    assert!(delta.windows(2).all(|w| w[0].version < w[1].version));
    assert_eq!(delta[0].message.text.as_deref(), Some("first, edited"));
    assert!(delta[0].edited);
    assert!(delta[1].message.is_deleted);
    assert_eq!(delta[1].message.text, None, "a tombstone exposes no text");
    assert_eq!(delta[2].message.text.as_deref(), Some("third, final"));
    assert_eq!(delta[3].message.text.as_deref(), Some("reply, edited"));

    let idle = f.changes(&[(&f.channel_id, Some(&next_cursor))], 10).await;
    assert!(idle[0].changes.is_empty());
}

/// Several conversations in one request, each with its own cursor and its own status.
#[tokio::test]
async fn change_feed_reads_several_conversations_and_enforces_access() {
    let f = setup().await;
    let channel_msg = f.send(&f.alice, &f.channel_id, "in the channel").await;
    let data = exec_ok(
        &f.schema,
        f.alice.request(
            CREATE_CONVERSATION_MUTATION,
            json!({ "input": { "serverId": f.server_id, "otherUserId": f.bob.user_id.to_string() } }),
        ),
    )
    .await;
    let dm_id = data["conversation"]["id"]
        .as_str()
        .expect("dm id")
        .to_string();
    let dm_msg = f.send(&f.alice, &dm_id, "just between us").await;

    let (_, channel_cursor) = f.sync(&f.channel_id, None, 10).await;
    let later = f.send(&f.bob, &f.channel_id, "later").await;

    let feed = f
        .changes(
            &[(&dm_id, None), (&f.channel_id, Some(&channel_cursor))],
            10,
        )
        .await;
    assert_eq!(feed.len(), 2, "one entry per requested conversation");
    assert_eq!(feed[0].conversation_id, dm_id, "in request order");
    assert_eq!(
        feed[0].summary(),
        vec![(dm_msg.id.clone(), "CREATED".to_string())]
    );
    assert_eq!(feed[1].conversation_id, f.channel_id);
    assert_eq!(
        feed[1].summary(),
        vec![(later.id.clone(), "CREATED".to_string())],
        "only what changed after the channel's cursor"
    );
    assert_ne!(later.id, channel_msg.id);

    assert!(feed.iter().all(|c| c.status == "OK"));

    // Carol is in the workspace but not in the DM: the DM is reported on its own entry and the
    // channel next to it still syncs. The outsider is not in the workspace at all.
    let req = f.changes_request(
        &f.carol,
        &[(&f.channel_id, None), (&dm_id, Some(&channel_cursor))],
        10,
    );
    let data = exec_ok(&f.schema, req).await;
    let feed: Vec<_> = data["feed"]
        .as_array()
        .expect("feed")
        .iter()
        .map(ConversationChanges::from_json)
        .collect();
    assert_eq!(feed[0].status, "OK");
    assert!(
        !feed[0].changes.is_empty(),
        "the readable channel still syncs"
    );
    assert_eq!(feed[1].conversation_id, dm_id);
    assert_eq!(feed[1].status, "NO_ACCESS");
    assert!(feed[1].changes.is_empty(), "nothing from the DM leaks");
    assert_eq!(feed[1].cursor, channel_cursor, "the cursor is echoed");
    assert!(!feed[1].has_more);

    // A stored cursor ahead of the server (its counters were reset) restarts from the beginning.
    let feed = f.changes(&[(&dm_id, Some("999999"))], 10).await;
    assert_eq!(feed[0].status, "RESET");
    assert_eq!(
        feed[0].summary(),
        vec![(dm_msg.id.clone(), "CREATED".to_string())]
    );

    // A cursor the API never returned.
    for bad in ["not-a-cursor", "-1"] {
        let req = f.changes_request(&f.alice, &[(&f.channel_id, Some(bad))], 10);
        assert_eq!(exec_err_code(&f.schema, req).await, "VALIDATION", "{bad}");
    }
}

// ── Nested message fields ─────────────────────────────────────────────────

const NESTED_QUERY: &str = r#"
query Nested($serverId: ID!, $conversationId: ID!) {
  page: messages(serverId: $serverId, conversationId: $conversationId, dir: AFTER, limit: 50,
                 includeDeleted: false) {
    items {
      id text
      author { id username }
      reactions { emoji count reactedByMe userIds }
      isPinned
      mentionedUserIds
      mentions { id username }
      linkPreviews { messageId url title description imageUrl siteName }
      forwardedFromMessageId
      forwardedFrom { id text conversationId authorId }
    }
  }
}
"#;

const TOGGLE_REACTION: &str = r#"
mutation React($input: ToggleReactionInput!) { toggleReaction(input: $input) { emoji } }
"#;

const PIN_MESSAGE: &str = "mutation Pin($input: PinMessageInput!) { pinMessage(input: $input) }";

const UNPIN_MESSAGE: &str =
    "mutation Unpin($input: UnpinMessageInput!) { unpinMessage(input: $input) }";

const FORWARD_MESSAGE: &str = r#"
mutation Forward($input: ForwardMessageInput!) { msg: forwardMessage(input: $input) { id } }
"#;

impl Fixture {
    /// The fixture channel's messages as `who` sees them, oldest first, keyed by id.
    async fn nested(&self, who: &Caller) -> std::collections::HashMap<String, Value> {
        let vars = json!({ "serverId": self.server_id, "conversationId": self.channel_id });
        let data = exec_ok(&self.schema, who.request(NESTED_QUERY, vars)).await;
        data["page"]["items"]
            .as_array()
            .expect("items")
            .iter()
            .map(|m| (m["id"].as_str().expect("id").to_string(), m.clone()))
            .collect()
    }

    /// Run `mutation` as `who` on the message `msg` of `conversation_id`, plus `extra` input.
    async fn on_message(
        &self,
        who: &Caller,
        mutation: &str,
        conversation_id: &str,
        msg: &Msg,
        extra: Value,
    ) -> Value {
        let input = json!({
            "serverId": self.server_id, "conversationId": conversation_id, "messageId": msg.id,
        });
        let vars = json!({ "input": merged(input, extra) });
        exec_ok(&self.schema, who.request(mutation, vars)).await
    }

    async fn react(&self, who: &Caller, msg: &Msg, emoji: &str) {
        self.on_message(
            who,
            TOGGLE_REACTION,
            &self.channel_id,
            msg,
            json!({ "emoji": emoji }),
        )
        .await;
    }

    /// Forward `msg` from `source_id` into the fixture channel as alice; returns the new id.
    async fn forward(&self, source_id: &str, msg: &Msg) -> String {
        let vars = json!({ "input": {
            "serverId": self.server_id, "sourceConversationId": source_id,
            "targetConversationId": self.channel_id, "messageId": msg.id,
        } });
        let data = exec_ok(&self.schema, self.alice.request(FORWARD_MESSAGE, vars)).await;
        data["msg"]["id"].as_str().expect("id").to_string()
    }

    fn user(&self, who: &Caller, username: &str) -> Value {
        json!({ "id": who.user_id.to_string(), "username": username })
    }
}

/// A page of messages resolves each message's author, reaction groups (by emoji, counted, with
/// the viewer's own reaction marked), pin state, mentions, link previews and forwarded source; a
/// message with none of them answers empty lists, `false` and null.
#[tokio::test]
async fn messages_resolve_their_nested_fields_for_the_viewer() {
    use rim_core::link_preview::model::NewLinkPreview;
    use rim_core::link_preview::repository::LinkPreviewRepository;

    let f = setup().await;
    let (bob_id, carol_id) = (f.bob.user_id.to_string(), f.carol.user_id.to_string());
    let mentioning = f
        .send(
            &f.alice,
            &f.channel_id,
            &format!("standup <@{bob_id}> <@{carol_id}>"),
        )
        .await;
    let linking = f.send(&f.bob, &f.channel_id, "see the docs").await;
    let plain = f.send(&f.carol, &f.channel_id, "plain").await;
    let forwarded = f.forward(&f.channel_id, &linking).await;

    f.react(&f.bob, &mentioning, "👍").await;
    f.react(&f.carol, &mentioning, "👍").await;
    f.react(&f.alice, &mentioning, "🎉").await;
    f.react(&f.carol, &linking, "👀").await;
    f.on_message(&f.alice, PIN_MESSAGE, &f.channel_id, &mentioning, json!({}))
        .await;
    // Stored directly, as the fetcher would after reaching the page; no message text has a URL,
    // so the fetcher itself never runs.
    rim_infra_db::sea_orm_impl::link_preview::SeaOrmLinkPreviewRepository::new(f.ctx.db.clone())
        .create(NewLinkPreview {
            message_id: parse_uuid(&linking.id).into(),
            url: "https://example.com".to_string(),
            title: Some("Example".to_string()),
            description: None,
            image_url: Some("https://example.com/i.png".to_string()),
            site_name: Some("example".to_string()),
        })
        .await
        .expect("seed link preview");

    let seen = f.nested(&f.bob).await;
    let m = &seen[&mentioning.id];
    assert_eq!(m["author"], f.user(&f.alice, "alice"));
    assert_eq!(
        m["reactions"],
        json!([
            { "emoji": "🎉", "count": 1, "reactedByMe": false,
              "userIds": [f.alice.user_id.to_string()] },
            { "emoji": "👍", "count": 2, "reactedByMe": true, "userIds": [bob_id, carol_id] },
        ])
    );
    assert_eq!(m["isPinned"], json!(true));
    assert_eq!(m["mentionedUserIds"], json!([bob_id, carol_id]));
    assert_eq!(
        m["mentions"],
        json!([f.user(&f.bob, "bob"), f.user(&f.carol, "carol")])
    );

    let l = &seen[&linking.id];
    assert_eq!(l["author"], f.user(&f.bob, "bob"));
    assert_eq!(
        l["reactions"],
        json!([{ "emoji": "👀", "count": 1, "reactedByMe": false, "userIds": [carol_id] }])
    );
    assert_eq!(l["isPinned"], json!(false));
    assert_eq!(
        l["linkPreviews"],
        json!([{
            "messageId": linking.id, "url": "https://example.com", "title": "Example",
            "description": null, "imageUrl": "https://example.com/i.png", "siteName": "example",
        }])
    );

    let fw = &seen[&forwarded];
    assert_eq!(fw["author"], f.user(&f.alice, "alice"));
    assert_eq!(fw["forwardedFromMessageId"], json!(linking.id));
    assert_eq!(
        fw["forwardedFrom"],
        json!({
            "id": linking.id, "text": "see the docs",
            "conversationId": f.channel_id, "authorId": bob_id,
        })
    );

    let p = &seen[&plain.id];
    for field in ["reactions", "mentionedUserIds", "mentions", "linkPreviews"] {
        assert_eq!(p[field], json!([]), "{field}");
    }
    assert_eq!(p["isPinned"], json!(false));
    assert_eq!(p["forwardedFrom"], Value::Null);

    let as_alice = f.nested(&f.alice).await;
    let marks: Vec<_> = as_alice[&mentioning.id]["reactions"]
        .as_array()
        .expect("reactions")
        .iter()
        .map(|g| g["reactedByMe"].clone())
        .collect();
    assert_eq!(marks, [json!(true), json!(false)], "marked for the viewer");

    f.on_message(
        &f.alice,
        UNPIN_MESSAGE,
        &f.channel_id,
        &mentioning,
        json!({}),
    )
    .await;
    assert_eq!(
        f.nested(&f.bob).await[&mentioning.id]["isPinned"],
        json!(false)
    );
}

/// A forwarded message names its source to everyone who reads it, but `forwardedFrom` resolves
/// only for a viewer who can read the source conversation.
#[tokio::test]
async fn forwarded_from_resolves_only_for_readers_of_the_source() {
    let f = setup().await;
    let board = create_channel(&f.schema, &f.alice, &f.server_id, "board", true).await;
    let secret = f.send(&f.alice, &board, "the plan").await;
    let forwarded = f.forward(&board, &secret).await;

    let as_alice = f.nested(&f.alice).await;
    assert_eq!(
        as_alice[&forwarded]["forwardedFrom"]["text"],
        json!("the plan")
    );
    let as_bob = f.nested(&f.bob).await;
    assert_eq!(
        as_bob[&forwarded]["forwardedFromMessageId"],
        json!(secret.id)
    );
    assert_eq!(as_bob[&forwarded]["forwardedFrom"], Value::Null);
}

// ── Every messaging root ──────────────────────────────────────────────────

const CHANGES_ROOTS: &[&str] = &["query($s: ID!, $c: ID!) { messageChanges(input: { \
     serverId: $s, conversations: [{ conversationId: $c }] }) { status } }"];

impl Fixture {
    /// Alice's message "the root" in the channel, bob's reply to it and alice's attachment on it.
    async fn targets(&self) -> Targets {
        let (alice, bob) = (&self.alice, &self.bob);
        let (server, channel) = (&self.server_id, &self.channel_id);
        Targets::seed(
            &self.schema,
            server,
            channel,
            alice,
            bob,
            &self.carol,
            "the root",
        )
        .await
    }
}

/// Every root that reads or writes messages refuses a caller without a session and one outside
/// the workspace, and hides the channel from a guest nobody added to it. None of them changes
/// anything.
#[tokio::test]
async fn every_messaging_root_refuses_callers_outside_the_channel() {
    let f = setup().await;
    let t = f.targets().await;
    let roots = [CONVERSATION_MESSAGE_ROOTS, MESSAGE_ROOTS, ATTACHMENT_ROOTS];
    let calls = t.calls(&[&roots[..], &[CHANGES_ROOTS]].concat());
    assert_each_refused(&f.schema, &calls, "UNAUTHENTICATED", anonymous_request).await;
    assert_each_refused_as(&f.schema, &f.outsider, &calls, "FORBIDDEN").await;
    let gina = f
        .ctx
        .newcomer(&f.server_id, "gina", ServerMemberRole::Guest)
        .await;
    assert_each_refused_as(&f.schema, &gina, &t.calls(&roots), "NOT_FOUND").await;

    let seen = f.nested(&f.alice).await;
    assert_eq!(seen.len(), 1);
    let root = &seen[&t.message];
    assert_eq!(root["text"], "the root");
    assert_eq!(root["isPinned"], false);
    assert_eq!(root["reactions"], json!([]));
}

/// An id that names nothing is `NOT_FOUND`, at the conversation and at the message, thread or
/// attachment under a real conversation; a malformed id is `VALIDATION`.
#[tokio::test]
async fn every_messaging_root_answers_unknown_and_malformed_ids() {
    let f = setup().await;
    let t = f.targets().await;
    let roots = [CONVERSATION_MESSAGE_ROOTS, MESSAGE_ROOTS, ATTACHMENT_ROOTS];
    let nowhere = t.elsewhere(&unknown_id());
    assert_each_refused_as(&f.schema, &f.alice, &nowhere.calls(&roots), "NOT_FOUND").await;
    let missing = Targets {
        conversation: t.conversation.clone(),
        ..nowhere
    };
    let below = missing.calls(&[MESSAGE_ROOTS, ATTACHMENT_ROOTS]);
    assert_each_refused_as(&f.schema, &f.alice, &below, "NOT_FOUND").await;

    let malformed = t
        .elsewhere("not-an-id")
        .calls(&[&roots[..], &[CHANGES_ROOTS]].concat());
    assert_each_refused_as(&f.schema, &f.alice, &malformed, "VALIDATION").await;
}

const TEXT_ROOTS: [&str; 3] = [
    "mutation($s: ID!, $c: ID!, $text: String!) { sendMessage(input: { serverId: $s, \
     conversationId: $c, text: $text }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!, $text: String!) { replyToMessage(input: { \
     serverId: $s, conversationId: $c, rootMessageId: $m, text: $text }) { id } }",
    "mutation($s: ID!, $c: ID!, $m: ID!, $text: String!) { editMessage(input: { serverId: $s, \
     conversationId: $c, messageId: $m, text: $text }) { id } }",
];

const REACT: &str = "mutation($s: ID!, $c: ID!, $m: ID!, $emoji: String!) { \
     toggleReaction(input: { serverId: $s, conversationId: $c, messageId: $m, emoji: $emoji }) \
     { emoji } }";

/// Message text holds at most 10,000 characters. A reply and an edit need text; a new message
/// may have none, since its attachments are added after it. An emoji is 1 to 32 characters.
#[tokio::test]
async fn message_text_and_emoji_are_bounded() {
    let f = setup().await;
    let t = f.targets().await;
    let with = |doc: &'static str, extra: Value| (doc, merged(t.vars(), extra));

    let longest = f.send(&f.alice, &f.channel_id, &"a".repeat(10_000)).await;
    assert_eq!(longest.text.map(|text| text.len()), Some(10_000));
    let over = json!({ "text": "a".repeat(10_001) });
    let calls: Vec<_> = TEXT_ROOTS
        .iter()
        .map(|doc| with(doc, over.clone()))
        .collect();
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;

    let blank = json!({ "text": "  " });
    let calls: Vec<_> = TEXT_ROOTS[1..]
        .iter()
        .map(|doc| with(doc, blank.clone()))
        .collect();
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;
    let textless = f.send(&f.alice, &f.channel_id, "  ").await;
    assert_eq!(textless.text, None);

    let emojis = [json!({ "emoji": " " }), json!({ "emoji": "x".repeat(33) })];
    let calls: Vec<_> = emojis.into_iter().map(|emoji| with(REACT, emoji)).collect();
    assert_each_refused_as(&f.schema, &f.alice, &calls, "VALIDATION").await;
}

/// A page size of zero or less reads one item on every message list instead of failing.
#[tokio::test]
async fn message_lists_read_one_item_for_a_page_size_below_one() {
    let f = setup().await;
    let sent = f.send_many(2, "clamped needle").await;
    let r = f.reply(&f.bob, &sent[0], "first").await;
    f.reply(&f.bob, &sent[0], "second").await;
    let conversation_id = f.channel_id.clone();
    let lists = [
        Target::Timeline {
            conversation_id: conversation_id.clone(),
        },
        Target::Thread {
            conversation_id: conversation_id.clone(),
            thread_id: r.thread_id.expect("thread"),
        },
        Target::Search {
            conversation_id,
            query: "needle".to_string(),
        },
    ];
    for limit in [0, -5] {
        for list in &lists {
            let page = f.page(list, Dir::After, &Value::Null, limit, false).await;
            assert_eq!(page.items.len(), 1, "{list:?} limit {limit}");
        }
        let feed = f.changes(&[(&f.channel_id, None)], limit).await;
        assert_eq!(feed[0].changes.len(), 1, "messageChanges limit {limit}");
    }
}

const THREAD_ROOTS: [&str; 2] = [
    "query($s: ID!, $c: ID!, $t: ID!) { thread(serverId: $s, conversationId: $c, threadId: $t) \
     { id conversationId rootMessageId rootAuthorId rootText } }",
    "query($s: ID!, $c: ID!, $m: ID!) { thread: threadByRootMessage(serverId: $s, \
     conversationId: $c, rootMessageId: $m) { id conversationId rootMessageId rootAuthorId \
     rootText } }",
];

/// `thread` and `threadByRootMessage` resolve the thread under a root message. A root without
/// replies has no thread, a reply is not a root, and a deleted root shows no text.
#[tokio::test]
async fn thread_roots_resolve_the_thread_under_a_root_message() {
    let f = setup().await;
    let t = f.targets().await;
    let (schema, alice, vars) = (&f.schema, &f.alice, t.vars());
    let thread = |doc: &'static str, extra: Value| {
        let request = alice.request(doc, merged(vars.clone(), extra));
        async move { exec_ok(schema, request).await["thread"].clone() }
    };
    let mut expected = json!({
        "id": t.thread, "conversationId": t.conversation, "rootMessageId": t.message,
        "rootAuthorId": f.alice.user_id.to_string(), "rootText": "the root",
    });
    for doc in THREAD_ROOTS {
        assert_eq!(thread(doc, json!({})).await, expected, "{doc}");
    }

    let lonely = f.send(&f.bob, &f.channel_id, "no replies").await;
    assert_eq!(
        thread(THREAD_ROOTS[1], json!({ "m": lonely.id })).await,
        Value::Null
    );
    let reply = f.reply(&f.bob, &lonely, "a reply").await;
    let vars = merged(t.vars(), json!({ "m": reply.id }));
    let code = exec_err_code(&f.schema, f.alice.request(THREAD_ROOTS[1], vars)).await;
    assert_eq!(code, "NOT_FOUND", "a reply is not a root");

    let delete = json!({ "input": {
        "serverId": f.server_id, "conversationId": f.channel_id, "messageId": t.message,
    } });
    exec_ok(&f.schema, f.alice.request(DELETE_MUTATION, delete)).await;
    expected["rootText"] = Value::Null;
    for doc in THREAD_ROOTS {
        assert_eq!(thread(doc, json!({})).await, expected, "{doc}");
    }
}

const CREATE_ATTACHMENT: &str = "mutation($s: ID!, $c: ID!, $m: ID!) { createAttachment(input: { \
     serverId: $s, conversationId: $c, messageId: $m, filename: \"report.txt\", \
     contentType: \"text/plain\", sizeBytes: 5 }) { attachmentId uploadToken maxSizeBytes } }";

const ATTACHMENT_CHANGES: [&str; 2] = [
    "mutation($s: ID!, $a: ID!) { change: finalizeAttachment(input: { serverId: $s, \
     attachmentId: $a }) { id status filename contentType sizeBytes } }",
    "mutation($s: ID!, $a: ID!) { change: deleteAttachment(input: { serverId: $s, \
     attachmentId: $a }) }",
];

const MESSAGE_ATTACHMENTS: &str = "query($s: ID!, $c: ID!, $m: ID!) { message(serverId: $s, \
     conversationId: $c, messageId: $m) { hasAttachments attachments { id status } } }";

/// Only a message's author attaches files to it and finalizes or deletes them. Finalizing waits
/// for the upload; deleting the last attachment clears `hasAttachments`.
#[tokio::test]
async fn only_the_author_manages_a_messages_attachments() {
    let f = setup().await;
    let t = f.targets().await;
    assert_each_refused_as(
        &f.schema,
        &f.bob,
        &t.calls(&[ATTACHMENT_ROOTS]),
        "FORBIDDEN",
    )
    .await;

    let ticket = exec_ok(&f.schema, f.alice.request(CREATE_ATTACHMENT, t.vars())).await;
    let ticket = &ticket["createAttachment"];
    assert_eq!(ticket["maxSizeBytes"], 5);
    let uploaded = merged(t.vars(), json!({ "a": ticket["attachmentId"] }));
    let finalize = (ATTACHMENT_CHANGES[0], uploaded.clone());
    assert_eq!(
        exec_err_code(&f.schema, f.alice.call(finalize.clone())).await,
        "VALIDATION",
        "nothing uploaded yet"
    );
    let attachment_id: Uuid = ticket["attachmentId"]
        .as_str()
        .expect("id")
        .parse()
        .expect("uuid");
    f.ctx
        .build_provider()
        .message_attachment_service()
        .complete_upload(
            rim_services::message_attachment::command::CompleteUploadCmd {
                actor_user_id: f.alice.user_id,
                attachment_id: attachment_id.into(),
                upload_token: ticket["uploadToken"].as_str().expect("token").to_string(),
                uploaded_filename: "report.txt".to_string(),
                content_type: "text/plain".to_string(),
                data: b"hello".to_vec(),
            },
        )
        .await
        .expect("upload");
    let ready = exec_ok(&f.schema, f.alice.call(finalize)).await;
    assert_eq!(
        ready["change"],
        json!({
            "id": uploaded["a"], "status": "READY", "filename": "report.txt",
            "contentType": "text/plain", "sizeBytes": 5,
        })
    );

    let message = exec_ok(&f.schema, f.alice.request(MESSAGE_ATTACHMENTS, t.vars())).await;
    assert_eq!(message["message"]["hasAttachments"], true);
    assert_eq!(
        message["message"]["attachments"]
            .as_array()
            .expect("list")
            .len(),
        2
    );
    for vars in [t.vars(), uploaded] {
        let deleted = exec_ok(&f.schema, f.alice.request(ATTACHMENT_CHANGES[1], vars)).await;
        assert_eq!(deleted["change"], true);
    }
    let message = exec_ok(&f.schema, f.alice.request(MESSAGE_ATTACHMENTS, t.vars())).await;
    assert_eq!(
        message["message"],
        json!({ "hasAttachments": false, "attachments": [] })
    );
}
