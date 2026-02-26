#![cfg(feature = "db-tests")]

mod support;

use rim_core::message::repository::MessageRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::{PageDir, SortOrder};
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_core::conversation::model::ConversationType;
use rim_core::message::model::{MessageId, NewMessage};
use rim_core::server::repository::ServerRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message_thread::repository::MessageThreadRepository;

#[tokio::test]
async fn create_get_update_text_and_soft_delete() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(conn.clone());
    let repo = SeaOrmMessageRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(ConversationType::Group, server.id)
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();

    let created = repo
        .create_message(&NewMessage {
            message_thread_id: thread.id,
            parent_message_id: None,
            sent_by: rim_core::user::model::UserId::new(),
            text: Some("hello".to_string()),
            attachments: vec![],
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
    assert_eq!(after.is_deleted, true);

    let err = repo
        .update_message_text(MessageId::new(), "x".to_string())
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let err = repo.soft_delete_message(MessageId::new()).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn list_messages_respects_include_deleted_and_ordering() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(conn.clone());
    let repo = SeaOrmMessageRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(ConversationType::Group, server.id)
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();

    let m1 = repo
        .create_message(&NewMessage {
            message_thread_id: thread.id,
            parent_message_id: None,
            sent_by: rim_core::user::model::UserId::new(),
            text: Some("m1".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();
    let m2 = repo
        .create_message(&NewMessage {
            message_thread_id: thread.id,
            parent_message_id: None,
            sent_by: rim_core::user::model::UserId::new(),
            text: Some("m2".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();

    repo.soft_delete_message(m1.id).await.unwrap();

    let page = repo
        .list_messages(thread.id, 10, PageDir::After, None, false, SortOrder::Asc)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 1);
    assert_eq!(page.items[0].id, m2.id);

    let page = repo
        .list_messages(thread.id, 10, PageDir::After, None, true, SortOrder::Desc)
        .await
        .unwrap();
    assert_eq!(page.items.len(), 2);
    assert!(page.items[0].sent_at >= page.items[1].sent_at);
}
