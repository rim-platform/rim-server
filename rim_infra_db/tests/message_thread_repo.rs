#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::PageDir;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;

#[tokio::test]
async fn create_get_and_list_threads_for_conversation() {
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

    let page = repo
        .list_threads_for_conversation(conv.id, 10, PageDir::After, None)
        .await
        .unwrap();
    assert!(page.items.len() >= 2);

    let page1 = repo
        .list_threads_for_conversation(conv.id, 2, PageDir::After, None)
        .await
        .unwrap();
    assert_eq!(page1.items.len(), 2);
    assert!(page1.next.is_none());
    assert!(page1.prev.is_none());

    let page2 = repo
        .list_threads_for_conversation(
            conv.id,
            1,
            PageDir::After,
            Some(rim_core::message_thread::repository::ThreadCursor {
                last_message_at: page1.items[1].last_message_at,
                thread_id: page1.items[1].id,
            }),
        )
        .await
        .unwrap();
    assert!(page2.items.is_empty());

    let before = repo
        .list_threads_for_conversation(
            conv.id,
            10,
            PageDir::Before,
            Some(rim_core::message_thread::repository::ThreadCursor {
                last_message_at: page1.items[1].last_message_at,
                thread_id: page1.items[1].id,
            }),
        )
        .await
        .unwrap();
    assert_eq!(before.items.len(), 1);
    assert_eq!(before.items[0].id, page1.items[0].id);
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

/// `Before` pages hold the threads adjacent to the cursor (finding A-H1). Endpoint order is
/// `last_message_at DESC`, so `Before` walks toward more recently active threads.
#[tokio::test]
async fn list_threads_before_returns_threads_adjacent_to_cursor() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;

    // threads[i] is the i-th least recently active.
    let base = chrono::Utc::now();
    let mut threads = Vec::new();
    for i in 0..5 {
        let t = ctx.create_thread(conv.id).await;
        repo.touch_last_message_at(t.id, base + chrono::Duration::seconds(i))
            .await
            .unwrap();
        threads.push(repo.get_by_id(t.id).await.unwrap().unwrap());
    }
    let cursor_of = |i: usize| rim_core::message_thread::repository::ThreadCursor {
        last_message_at: threads[i].last_message_at,
        thread_id: threads[i].id,
    };
    let ids = |idx: &[usize]| idx.iter().map(|&i| threads[i].id).collect::<Vec<_>>();

    let page1 = repo
        .list_threads_for_conversation(conv.id, 2, PageDir::Before, Some(cursor_of(0)))
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        ids(&[2, 1])
    );
    assert_eq!(page1.next, Some(cursor_of(2)));
    assert_eq!(page1.prev, Some(cursor_of(1)));

    let page2 = repo
        .list_threads_for_conversation(conv.id, 2, PageDir::Before, page1.next)
        .await
        .unwrap();
    assert_eq!(
        page2.items.iter().map(|t| t.id).collect::<Vec<_>>(),
        ids(&[4, 3])
    );
    assert_eq!(page2.next, None);
    assert_eq!(page2.prev, Some(cursor_of(3)));
}
