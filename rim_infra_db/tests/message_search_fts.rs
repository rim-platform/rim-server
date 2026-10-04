//! DB-integration tests for [`SeaOrmMessageRepository::search_messages_fts`] — the Postgres
//! full-text-search backing `searchMessages` (Track B1). Run with:
//! `cargo test -p rim_infra_db --features db-tests -- --test-threads=1` (requires `DATABASE_URL`;
//! panics loudly if unset — see `support::db_conn`).
//!
//! Covers, per the implementation spec:
//! 1. Stemmed matching (proves tsvector, not LIKE).
//! 2. Relevance ranking is present and results are ordered by recency, not rank.
//! 3. **Permission isolation** — the repository never returns a message from a conversation
//!    outside the caller-supplied `conversation_ids` set, even on an exact-term match or a
//!    matching `author_id` filter. This is the repo-level half of the visibility contract
//!    documented on `MessageSearchParams`; the service layer is responsible for computing that
//!    set from the actor's real access (see `DefaultMessageService::search_messages`), but the
//!    property this test locks — that the repo's `WHERE conversation_id = ANY($accessible_set)`
//!    is airtight — is exactly what makes that contract enforceable.
//! 4. Operator filters (`has:link`, `has:attachment`, `before:`/`after:`) each constrain results.
//! 5. Cursor pagination covers the full ordered set with no gap or overlap.
//! 6. Deleted messages are never returned, even on an exact-term match.

#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, TimeZone, Utc};
use rim_core::conversation::model::ConversationType;
use rim_core::message::model::MessageId;
use rim_core::message::repository::{MessageRepository, MessageSearchParams};
use rim_core::repository::pagination::PageDir;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

/// Directly overwrite a message's `sent_at` (the repository/service surface has no way to set
/// this — production messages are always "now" — so tests that need deterministic ordering or
/// `before:`/`after:` boundaries reach into the entity layer, matching the pattern already used
/// by `audit_log_repo.rs` for `created_at`).
async fn set_sent_at(db: &DbHandle, message_id: MessageId, sent_at: chrono::DateTime<Utc>) {
    let model = entity::message::Entity::find_by_id(uuid::Uuid::from(message_id))
        .one(db)
        .await
        .expect("query failed")
        .expect("message must exist");
    let mut active: entity::message::ActiveModel = model.into();
    active.sent_at = Set(sent_at.into());
    active.update(db).await.expect("update failed");
}

fn base_params(
    conversation_ids: Vec<rim_core::conversation::model::ConversationId>,
) -> MessageSearchParams {
    MessageSearchParams {
        conversation_ids,
        tsquery: String::new(),
        author_id: None,
        has_link: false,
        has_attachment: false,
        after: None,
        before: None,
        limit: 50,
        cursor: None,
        dir: PageDir::After,
    }
}

#[tokio::test]
async fn search_messages_fts_matches_stemmed_terms() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("fts-user-1").await;

    let msg = ctx
        .create_message(conv.id, user.id, "we are deploying to staging")
        .await;

    let hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "deploy".to_string(),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();

    assert_eq!(
        hits.len(),
        1,
        "the query `deploy` must match `deploying` via stemming, not exact substring"
    );
    assert_eq!(hits[0].message.id, msg.id);
}

#[tokio::test]
async fn search_messages_fts_ranks_matches_and_orders_by_recency() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("fts-user-2").await;

    let older = ctx
        .create_message(conv.id, user.id, "deploy is the only relevant word here")
        .await;
    let newer = ctx
        .create_message(
            conv.id,
            user.id,
            "deploy deploy deploy — much more relevant, but sent later",
        )
        .await;
    set_sent_at(
        &ctx.db,
        older.id,
        Utc.with_ymd_and_hms(2026, 1, 1, 0, 0, 0).unwrap(),
    )
    .await;
    set_sent_at(
        &ctx.db,
        newer.id,
        Utc.with_ymd_and_hms(2026, 1, 2, 0, 0, 0).unwrap(),
    )
    .await;

    let hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "deploy".to_string(),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();

    assert_eq!(hits.len(), 2);
    assert!(
        hits.iter().all(|h| h.rank > 0.0),
        "every match must have a positive rank: {hits:?}"
    );
    // Ordered by (sent_at DESC, id DESC) — recency — even though `newer` has a much higher
    // rank (repeats "deploy" 3x) than `older`. This is the load-bearing property from spec
    // §6: relevance is informational, never the sort key.
    assert_eq!(
        hits.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![newer.id, older.id],
        "results must be ordered by sent_at DESC regardless of relevance"
    );
}

#[tokio::test]
async fn search_messages_fts_excludes_conversations_outside_accessible_set() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    // `accessible` stands in for a conversation the actor can see; `hidden` stands in for a
    // DM/channel they cannot (a non-member DM, or an `in:` channel outside their access).
    let accessible = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let hidden = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let author = ctx.create_user("fts-user-3").await;

    let visible_msg = ctx
        .create_message(accessible.id, author.id, "quarterly roadmap review")
        .await;
    let hidden_msg = ctx
        .create_message(hidden.id, author.id, "quarterly roadmap review")
        .await;

    // Exact-term match on both conversations, but only `accessible` is in the passed set.
    let hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "roadmap".to_string(),
            ..base_params(vec![accessible.id])
        })
        .await
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![visible_msg.id],
        "message in a conversation outside the accessible set must never appear, even on an exact-term match"
    );

    // `from:` (author_id filter) must not widen visibility: filtering by the author who also
    // posted the hidden message still must not surface it, because `hidden` is still absent
    // from `conversation_ids`.
    let hits_by_author = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "roadmap".to_string(),
            author_id: Some(author.id),
            ..base_params(vec![accessible.id])
        })
        .await
        .unwrap();
    assert_eq!(
        hits_by_author
            .iter()
            .map(|h| h.message.id)
            .collect::<Vec<_>>(),
        vec![visible_msg.id],
        "from: filter must not widen visibility beyond the accessible conversation set"
    );

    // An empty accessible set (e.g. an `in:` channel that resolved outside the actor's access)
    // yields zero rows — never falls back to "all".
    let empty = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "roadmap".to_string(),
            ..base_params(vec![])
        })
        .await
        .unwrap();
    assert!(empty.is_empty());
    let _ = hidden_msg; // referenced only to prove it exists and is still excluded above
}

#[tokio::test]
async fn search_messages_fts_operator_filters_constrain_results() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let author_a = ctx.create_user("fts-user-4a").await;
    let author_b = ctx.create_user("fts-user-4b").await;

    let with_link = ctx
        .create_message(
            conv.id,
            author_a.id,
            "release notes: https://example.com/notes",
        )
        .await;
    let with_attachment = ctx
        .create_message(conv.id, author_a.id, "release notes attached")
        .await;
    repo.set_has_attachments(with_attachment.id, true)
        .await
        .unwrap();
    let plain = ctx
        .create_message(conv.id, author_b.id, "release notes look good")
        .await;
    set_sent_at(
        &ctx.db,
        plain.id,
        Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap(),
    )
    .await;

    // has:link
    let link_hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "release".to_string(),
            has_link: true,
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert_eq!(
        link_hits.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![with_link.id]
    );

    // has:attachment
    let attachment_hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "release".to_string(),
            has_attachment: true,
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert_eq!(
        attachment_hits
            .iter()
            .map(|h| h.message.id)
            .collect::<Vec<_>>(),
        vec![with_attachment.id]
    );

    // from: (author_id)
    let author_hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "release".to_string(),
            author_id: Some(author_b.id),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert_eq!(
        author_hits.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![plain.id]
    );

    // before: / after: — plain is 2026-03-15; before that date excludes it, after includes it.
    let before_hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "release".to_string(),
            before: Some(Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap()),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert!(
        !before_hits.iter().any(|h| h.message.id == plain.id),
        "before: is an exclusive upper bound — the boundary date itself must not match"
    );

    let after_hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "release".to_string(),
            after: Some(Utc.with_ymd_and_hms(2026, 3, 15, 0, 0, 0).unwrap()),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert!(
        after_hits.iter().any(|h| h.message.id == plain.id),
        "after: is an inclusive lower bound — the boundary date itself must match"
    );
}

#[tokio::test]
async fn search_messages_fts_pagination_covers_full_set_with_no_gap_or_overlap() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("fts-user-5").await;

    let base = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let mut ids = Vec::new();
    for i in 0..5u32 {
        let m = ctx
            .create_message(conv.id, user.id, "pagination marker text")
            .await;
        set_sent_at(&ctx.db, m.id, base + Duration::minutes(i64::from(i))).await;
        ids.push(m.id);
    }
    // Most recent first (sent_at DESC).
    ids.reverse();

    let page1 = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "pagination".to_string(),
            limit: 2,
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert_eq!(
        page1.len(),
        3,
        "repo returns up to limit+1 rows to signal a next page exists"
    );
    let page1_items: Vec<_> = page1.iter().take(2).map(|h| h.message.id).collect();
    assert_eq!(page1_items, ids[0..2]);

    let cursor = rim_core::message::repository::MessageCursor {
        sent_at: page1[1].message.sent_at,
        message_id: page1[1].message.id,
    };
    let page2 = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "pagination".to_string(),
            limit: 2,
            cursor: Some(cursor),
            dir: PageDir::After,
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    let page2_items: Vec<_> = page2.iter().take(2).map(|h| h.message.id).collect();
    assert_eq!(page2_items, ids[2..4]);

    let cursor2 = rim_core::message::repository::MessageCursor {
        sent_at: page2[1].message.sent_at,
        message_id: page2[1].message.id,
    };
    let page3 = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "pagination".to_string(),
            limit: 2,
            cursor: Some(cursor2),
            dir: PageDir::After,
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();
    assert_eq!(
        page3.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![ids[4]],
        "final page has no next row, so no limit+1 overflow row"
    );
}

#[tokio::test]
async fn search_messages_fts_excludes_deleted_messages() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("fts-user-6").await;

    let msg = ctx
        .create_message(conv.id, user.id, "tombstoned content should never surface")
        .await;
    repo.soft_delete_message(msg.id).await.unwrap();

    let hits = repo
        .search_messages_fts(MessageSearchParams {
            tsquery: "tombstoned".to_string(),
            ..base_params(vec![conv.id])
        })
        .await
        .unwrap();

    assert!(
        hits.is_empty(),
        "a soft-deleted message must never appear in search results, even on an exact-term match"
    );
}

/// `Before` (toward newer) selects the rows nearest the cursor (finding A-H1), still returned
/// `(sent_at DESC, id DESC)`, so the `limit + 1` overflow row is the **first** one — the newest,
/// farthest from the cursor. The pre-fix query returned the newest rows overall and skipped the
/// ones just after the cursor.
#[tokio::test]
async fn search_messages_fts_before_selects_rows_nearest_the_cursor() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("fts-user-before").await;

    // Oldest first.
    let base = Utc.with_ymd_and_hms(2026, 5, 1, 0, 0, 0).unwrap();
    let mut msgs = Vec::new();
    for i in 0..5u32 {
        let m = ctx
            .create_message(conv.id, user.id, "pagination marker text")
            .await;
        let sent_at = base + Duration::minutes(i64::from(i));
        set_sent_at(&ctx.db, m.id, sent_at).await;
        msgs.push((m.id, sent_at));
    }
    let cursor_at = |i: usize| rim_core::message::repository::MessageCursor {
        sent_at: msgs[i].1,
        message_id: msgs[i].0,
    };
    let before = |cursor| {
        repo.search_messages_fts(MessageSearchParams {
            tsquery: "pagination".to_string(),
            limit: 2,
            cursor: Some(cursor),
            dir: PageDir::Before,
            ..base_params(vec![conv.id])
        })
    };

    let page1 = before(cursor_at(0)).await.unwrap();
    assert_eq!(
        page1.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![msgs[3].0, msgs[2].0, msgs[1].0],
        "the three rows just newer than the cursor, newest first (overflow row first)"
    );

    let page2 = before(cursor_at(2)).await.unwrap();
    assert_eq!(
        page2.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![msgs[4].0, msgs[3].0],
        "final page has no overflow row"
    );
}
