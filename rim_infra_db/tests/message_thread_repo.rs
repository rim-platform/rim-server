#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;

#[tokio::test]
async fn create_get_and_touch_threads() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;

    let t1 = ctx.create_thread(conv.id).await;
    let t2 = ctx.create_thread(conv.id).await;

    let at1 = chrono::Utc::now() + chrono::Duration::seconds(1);
    let at2 = chrono::Utc::now() + chrono::Duration::seconds(2);
    repo.touch_last_message_at(t1.id, at1).await.unwrap();
    repo.touch_last_message_at(t2.id, at2).await.unwrap();

    assert!(repo.get_by_id(t1.id).await.unwrap().is_some());
    assert!(repo.get_by_id(t2.id).await.unwrap().is_some());
    assert_eq!(
        repo.get_by_id(t1.id)
            .await
            .unwrap()
            .unwrap()
            .last_message_at
            .timestamp_micros(),
        at1.timestamp_micros()
    );
}

#[tokio::test]
async fn create_thread_for_root_is_atomic_and_conflicts_on_duplicate_root() {
    let ctx = support::TestCtx::new().await;

    let thread_repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("testuser").await;
    let msg = ctx.create_message(conv.id, user.id, "root").await;

    let thread = thread_repo
        .create_thread_for_root(conv.id, msg.id)
        .await
        .unwrap();
    assert_eq!(thread.first_message_id, Some(msg.id));

    let err = thread_repo
        .create_thread_for_root(conv.id, msg.id)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}

#[tokio::test]
async fn touch_last_message_at_not_found() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());
    let err = repo
        .touch_last_message_at(
            rim_core::message_thread::model::MessageThreadId::new(),
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn get_thread_ids_by_root_messages_batch() {
    let ctx = support::TestCtx::new().await;

    let thread_repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("thread-batch-user").await;

    let msg1 = ctx.create_message(conv.id, user.id, "root1").await;
    let msg2 = ctx.create_message(conv.id, user.id, "root2").await;

    let t1 = thread_repo
        .create_thread_for_root(conv.id, msg1.id)
        .await
        .unwrap();
    let t2 = thread_repo
        .create_thread_for_root(conv.id, msg2.id)
        .await
        .unwrap();

    let map = thread_repo
        .get_thread_ids_by_root_messages(&[msg1.id, msg2.id])
        .await
        .unwrap();

    assert_eq!(map.len(), 2);
    assert_eq!(map[&msg1.id], t1.id);
    assert_eq!(map[&msg2.id], t2.id);
}
