#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::message::model::{MessageId, NewMessage};
use rim_core::message::repository::{ListMessagesParams, MessageCursor, MessageRepository};
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::{PageDir, SortOrder};
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;

#[tokio::test]
async fn create_get_update_text_and_soft_delete() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-1").await;
    let thread = ctx.create_thread(conv.id).await;

    let created = repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
            text: Some("hello".to_string()),
            forwarded_from_message_id: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.text.as_deref(), Some("hello"));

    let updated = repo
        .update_message_text(created.id, "updated".to_string())
        .await
        .unwrap();
    assert_eq!(updated.text.as_deref(), Some("updated"));

    repo.soft_delete_message(created.id).await.unwrap();
    let after = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert!(after.is_deleted);

    let err = repo
        .update_message_text(MessageId::new(), "x".to_string())
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let err = repo
        .soft_delete_message(MessageId::new())
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn list_messages_respects_include_deleted_and_ordering() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-2").await;
    let thread = ctx.create_thread(conv.id).await;

    let m1 = ctx
        .create_thread_message(conv.id, thread.id, user.id, "m1")
        .await;
    let m2 = ctx
        .create_thread_message(conv.id, thread.id, user.id, "m2")
        .await;

    repo.soft_delete_message(m1.id).await.unwrap();

    let page = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            limit: 10,
            dir: PageDir::After,
            cursor: None,
            include_deleted: false,
            order: SortOrder::Asc,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, m2.id);

    let page = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            limit: 10,
            dir: PageDir::After,
            cursor: None,
            include_deleted: true,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.items[0].sent_at >= page.items[1].sent_at);
}

#[tokio::test]
async fn timeline_listing_excludes_thread_messages() {
    let ctx = support::TestCtx::new().await;

    let thread_repo = SeaOrmMessageThreadRepository::new(ctx.db.clone());
    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-3").await;
    let root = ctx.create_message(conv.id, user.id, "root").await;
    let thread = thread_repo
        .create_thread_for_root(conv.id, root.id)
        .await
        .unwrap();

    let _thread_reply = ctx
        .create_thread_message(conv.id, thread.id, user.id, "reply")
        .await;

    let timeline = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: None,
            limit: 10,
            dir: PageDir::After,
            cursor: None,
            include_deleted: false,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(timeline.items.len(), 1);
    assert_eq!(timeline.items[0].id, root.id);

    let thread_page = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            limit: 10,
            dir: PageDir::After,
            cursor: None,
            include_deleted: false,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(thread_page.items.len(), 1);
    assert_eq!(thread_page.items[0].thread_id, Some(thread.id));
}

#[tokio::test]
async fn desc_pagination_advances_to_older_messages() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-4").await;

    let m1 = ctx.create_message(conv.id, user.id, "m1").await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let m2 = ctx.create_message(conv.id, user.id, "m2").await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let m3 = ctx.create_message(conv.id, user.id, "m3").await;

    let page1 = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: None,
            limit: 2,
            dir: PageDir::After,
            cursor: None,
            include_deleted: false,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![m3.id, m2.id]
    );
    let page2 = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: None,
            limit: 2,
            dir: PageDir::After,
            cursor: page1.next,
            include_deleted: false,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(
        page2.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![m1.id]
    );

    let before = repo
        .list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: None,
            limit: 10,
            dir: PageDir::Before,
            cursor: Some(rim_core::message::repository::MessageCursor {
                sent_at: m2.sent_at,
                message_id: m2.id,
            }),
            include_deleted: false,
            order: SortOrder::Desc,
        })
        .await
        .unwrap();
    assert_eq!(
        before.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        vec![m3.id]
    );
}

#[tokio::test]
async fn search_messages_fts_returns_recent_matches() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-5").await;

    let older = ctx
        .create_message(conv.id, user.id, "deploy failed earlier")
        .await;
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    let newer = ctx
        .create_message(conv.id, user.id, "deploy failed now")
        .await;

    let results = repo
        .search_messages_fts(rim_core::message::repository::MessageSearchParams {
            conversation_ids: vec![conv.id],
            tsquery: "deploy failed".to_string(),
            author_id: None,
            has_link: false,
            has_attachment: false,
            after: None,
            before: None,
            limit: 10,
            cursor: None,
            dir: PageDir::After,
        })
        .await
        .unwrap();

    assert_eq!(results.len(), 2);
    assert_eq!(results[0].message.id, newer.id);
    assert_eq!(results[1].message.id, older.id);
    assert!(results[0].rank > 0.0);
}

#[tokio::test]
async fn get_by_ids_returns_matching_messages() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-6").await;

    let m1 = ctx.create_message(conv.id, user.id, "m1").await;
    let m2 = ctx.create_message(conv.id, user.id, "m2").await;
    let m3 = ctx.create_message(conv.id, user.id, "m3").await;

    let map = repo.get_by_ids(&[m1.id, m3.id]).await.unwrap();
    assert_eq!(map.len(), 2);
    assert_eq!(map[&m1.id].text.as_deref(), Some("m1"));
    assert_eq!(map[&m3.id].text.as_deref(), Some("m3"));
    assert!(!map.contains_key(&m2.id));
}

#[tokio::test]
async fn get_by_ids_with_empty_slice_returns_empty() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let map = repo.get_by_ids(&[]).await.unwrap();
    assert!(map.is_empty());
}

#[tokio::test]
async fn set_has_attachments_updates_flag() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-7").await;

    let msg = ctx.create_message(conv.id, user.id, "attach-test").await;
    assert!(!msg.has_attachments);

    let updated = repo.set_has_attachments(msg.id, true).await.unwrap();
    assert!(updated.has_attachments);

    let fetched = repo.get_by_id(msg.id).await.unwrap().unwrap();
    assert!(fetched.has_attachments);

    let updated = repo.set_has_attachments(msg.id, false).await.unwrap();
    assert!(!updated.has_attachments);

    let fetched = repo.get_by_id(msg.id).await.unwrap().unwrap();
    assert!(!fetched.has_attachments);
}

/// `Before` pages hold the rows adjacent to the cursor (finding A-H1): walking `Before` from one
/// end of the list, following `next`, visits every other row exactly once, nearest first. The
/// pre-fix query returned the far end of the range and skipped the rows next to the cursor.
#[tokio::test]
async fn before_pagination_returns_rows_adjacent_to_cursor() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let user = ctx.create_user("msg-user-before").await;

    let mut msgs = Vec::new();
    for i in 0..5 {
        msgs.push(ctx.create_message(conv.id, user.id, &format!("m{i}")).await);
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    msgs.sort_by_key(|m| (m.sent_at, uuid::Uuid::from(m.id)));
    let cursor_of = |m: &rim_core::message::model::Message| MessageCursor {
        sent_at: m.sent_at,
        message_id: m.id,
    };
    let ids = |idx: &[usize]| idx.iter().map(|&i| msgs[i].id).collect::<Vec<_>>();

    let list = |order: SortOrder, cursor: Option<MessageCursor>| {
        repo.list_messages(ListMessagesParams {
            conversation_id: conv.id,
            thread_id: None,
            limit: 2,
            dir: PageDir::Before,
            cursor,
            include_deleted: false,
            order,
        })
    };

    // DESC endpoint: `Before` = newer. From the oldest row: [2,1], then [4,3].
    let page1 = list(SortOrder::Desc, Some(cursor_of(&msgs[0])))
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids(&[2, 1])
    );
    assert_eq!(page1.next, Some(cursor_of(&msgs[2])));
    assert_eq!(page1.prev, Some(cursor_of(&msgs[1])));
    let page2 = list(SortOrder::Desc, page1.next).await.unwrap();
    assert_eq!(
        page2.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids(&[4, 3])
    );
    assert_eq!(page2.next, None, "no row is newer than the newest");
    assert_eq!(page2.prev, Some(cursor_of(&msgs[3])));

    // ASC endpoint: `Before` = older. From the newest row: [2,3], then [0,1].
    let page1 = list(SortOrder::Asc, Some(cursor_of(&msgs[4])))
        .await
        .unwrap();
    assert_eq!(
        page1.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids(&[2, 3])
    );
    assert_eq!(page1.next, Some(cursor_of(&msgs[2])));
    assert_eq!(page1.prev, Some(cursor_of(&msgs[3])));
    let page2 = list(SortOrder::Asc, page1.next).await.unwrap();
    assert_eq!(
        page2.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids(&[0, 1])
    );
    assert_eq!(page2.next, None, "no row is older than the oldest");
    assert_eq!(page2.prev, Some(cursor_of(&msgs[1])));

    // A cursorless `Before` page still starts at the head of the endpoint order (callers use it
    // for "most recent"), unchanged by the fix.
    let head = list(SortOrder::Desc, None).await.unwrap();
    assert_eq!(
        head.items.iter().map(|m| m.id).collect::<Vec<_>>(),
        ids(&[4, 3])
    );
}
