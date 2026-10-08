#![cfg(feature = "db-tests")]

//! Message change feed (`docs/message-change-feed.md`): the `add_message_change_feed`
//! migration's trigger assigns per-conversation versions on every write path, and
//! `MessageRepository::list_message_changes` reads them back in order.
//!
//! Destructive for the shared test database, like every db-test: run single-threaded.

mod support;

use chrono::{Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement, TransactionTrait};

use migration::{Migrator, MigratorTrait};
use rim_core::conversation::model::ConversationType;
use rim_core::message::repository::MessageRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;

const MIGRATION_NAME: &str = "m20261008_211159_add_message_change_feed";

fn versions(changes: &[rim_core::message::repository::MessageChange]) -> Vec<i64> {
    changes.iter().map(|c| c.version).collect()
}

#[tokio::test]
async fn every_write_bumps_the_conversation_version() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let other_conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-1").await;
    let thread = ctx.create_thread(conv.id).await;

    let m1 = ctx.create_message(conv.id, user.id, "one").await;
    let m2 = ctx.create_message(conv.id, user.id, "two").await;
    let reply = ctx
        .create_thread_message(conv.id, thread.id, user.id, "reply")
        .await;
    // Another conversation keeps its own counter.
    ctx.create_message(other_conv.id, user.id, "elsewhere")
        .await;

    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![1, 2, 3]);
    assert_eq!(
        all.iter().map(|c| c.message.id).collect::<Vec<_>>(),
        vec![m1.id, m2.id, reply.id],
        "thread replies are in the feed"
    );
    assert!(all.iter().all(|c| c.edited_at.is_none()));
    let other = repo
        .list_message_changes(other_conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(versions(&other), vec![1]);

    // Edit, delete and attachment changes each take the next version and move the message to
    // the end of the feed; it appears once, in its newest state.
    repo.update_message_text(m1.id, "one, edited".to_string())
        .await
        .unwrap();
    repo.soft_delete_message(m2.id).await.unwrap();
    repo.set_has_attachments(reply.id, true).await.unwrap();

    let since = repo.list_message_changes(conv.id, 3, 10).await.unwrap();
    assert_eq!(versions(&since), vec![4, 5, 6]);
    assert_eq!(since[0].message.id, m1.id);
    assert_eq!(since[0].message.text.as_deref(), Some("one, edited"));
    assert!(since[0].edited_at.is_some(), "a text change sets edited_at");
    assert_eq!(since[1].message.id, m2.id);
    assert!(since[1].message.is_deleted);
    assert!(since[1].edited_at.is_none(), "a delete is not an edit");
    assert_eq!(since[2].message.id, reply.id);
    assert!(since[2].message.has_attachments);
    assert!(since[2].edited_at.is_none());

    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![4, 5, 6], "one entry per message");
    assert_eq!(repo.latest_change_version(conv.id).await.unwrap(), 6);
    let empty = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    assert_eq!(repo.latest_change_version(empty.id).await.unwrap(), 0);
}

#[tokio::test]
async fn a_write_that_changes_nothing_keeps_the_version() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-2").await;
    let m = ctx.create_message(conv.id, user.id, "same").await;

    repo.update_message_text(m.id, "same".to_string())
        .await
        .unwrap();
    repo.set_has_attachments(m.id, false).await.unwrap();

    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![1]);
    assert!(all[0].edited_at.is_none());
}

#[tokio::test]
async fn returns_up_to_limit_plus_one_rows_after_the_cursor() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-3").await;
    for i in 0..5 {
        ctx.create_message(conv.id, user.id, &format!("m{i}")).await;
    }

    let page = repo.list_message_changes(conv.id, 1, 2).await.unwrap();
    assert_eq!(
        versions(&page),
        vec![2, 3, 4],
        "limit + 1 rows, so the caller sees more"
    );
    let tail = repo.list_message_changes(conv.id, 4, 2).await.unwrap();
    assert_eq!(versions(&tail), vec![5]);
    let none = repo.list_message_changes(conv.id, 5, 2).await.unwrap();
    assert!(none.is_empty());
}

#[tokio::test]
async fn a_rolled_back_write_does_not_leave_a_gap() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-4").await;
    ctx.create_message(conv.id, user.id, "kept").await;

    let conn = support::db_conn().await;
    let txn = conn.begin().await.unwrap();
    txn.execute_raw(insert_message_sql(conv.id, user.id, "rolled back"))
        .await
        .unwrap();
    txn.rollback().await.unwrap();

    ctx.create_message(conv.id, user.id, "next").await;
    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![1, 2]);
}

#[tokio::test]
async fn migration_numbers_existing_messages_per_conversation_by_sent_at() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;

    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let empty_conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-5").await;

    Migrator::down(&conn, Some(steps))
        .await
        .expect("roll back add_message_change_feed");

    // Inserted out of `sent_at` order, the way imported or clock-skewed rows can be.
    let now = Utc::now();
    let seeded = [
        (uuid::Uuid::now_v7(), now - Duration::minutes(1), "second"),
        (uuid::Uuid::now_v7(), now - Duration::minutes(2), "first"),
        (uuid::Uuid::now_v7(), now, "third"),
    ];
    for (id, sent_at, text) in seeded {
        conn.execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO message (id, conversation_id, sent_at, sent_by, text, is_deleted, \
             has_attachments) VALUES ($1, $2, $3, $4, $5, false, false)",
            [
                id.into(),
                uuid::Uuid::from(conv.id).into(),
                sent_at.into(),
                uuid::Uuid::from(user.id).into(),
                text.into(),
            ],
        ))
        .await
        .unwrap();
    }

    Migrator::up(&conn, None)
        .await
        .expect("re-apply add_message_change_feed");

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![1, 2, 3]);
    assert_eq!(
        all.iter()
            .map(|c| c.message.text.as_deref().unwrap())
            .collect::<Vec<_>>(),
        vec!["first", "second", "third"]
    );

    // The counter continues after the back-filled versions, and a conversation that had no
    // messages starts at 1.
    ctx.create_message(conv.id, user.id, "fourth").await;
    ctx.create_message(empty_conv.id, user.id, "hello").await;
    let tail = repo.list_message_changes(conv.id, 3, 10).await.unwrap();
    assert_eq!(versions(&tail), vec![4]);
    let fresh = repo
        .list_message_changes(empty_conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(versions(&fresh), vec![1]);
}

fn insert_message_sql(
    conv: rim_core::conversation::model::ConversationId,
    user: rim_core::user::model::UserId,
    text: &str,
) -> Statement {
    Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO message (id, conversation_id, sent_by, text, is_deleted, has_attachments) \
         VALUES ($1, $2, $3, $4, false, false)",
        [
            uuid::Uuid::now_v7().into(),
            uuid::Uuid::from(conv).into(),
            uuid::Uuid::from(user).into(),
            text.into(),
        ],
    )
}

/// The ordering guarantee the client cursor relies on: while one transaction holds version N
/// uncommitted, a second writer in the same conversation waits instead of committing N+1 first.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_later_version_never_commits_before_an_earlier_one() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;
    let user = ctx.create_user("feed-user-6").await;

    let conn = support::db_conn().await;
    let first = conn.begin().await.unwrap();
    first
        .execute_raw(insert_message_sql(conv.id, user.id, "first"))
        .await
        .unwrap();

    let second_conn = support::db_conn().await;
    let (conv_id, user_id) = (conv.id, user.id);
    let second = tokio::spawn(async move {
        second_conn
            .execute_raw(insert_message_sql(conv_id, user_id, "second"))
            .await
            .unwrap();
    });

    std::thread::sleep(std::time::Duration::from_millis(300));
    assert!(
        !second.is_finished(),
        "the second writer waits for the first"
    );
    assert!(
        repo.list_message_changes(conv.id, 0, 10)
            .await
            .unwrap()
            .is_empty()
    );

    first.commit().await.unwrap();
    second.await.unwrap();

    let all = repo.list_message_changes(conv.id, 0, 10).await.unwrap();
    assert_eq!(versions(&all), vec![1, 2]);
    assert_eq!(all[0].message.text.as_deref(), Some("first"));
    assert_eq!(all[1].message.text.as_deref(), Some("second"));
}
