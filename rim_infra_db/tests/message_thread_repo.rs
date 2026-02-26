#![cfg(feature = "db-tests")]

mod support;

use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::repository::pagination::{PageDir};
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_core::conversation::model::ConversationType;
use rim_core::message::model::{MessageId, NewMessage};
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::repository::MessageRepository;

#[tokio::test]
async fn create_get_and_list_threads_for_conversation() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let repo = SeaOrmMessageThreadRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(ConversationType::Group, server.id)
        .await
        .unwrap();

    let t1 = repo.create_thread(conv.id).await.unwrap();
    let t2 = repo.create_thread(conv.id).await.unwrap();

    assert!(repo.get_by_id(t1.id).await.unwrap().is_some());
    assert!(repo.get_by_id(t2.id).await.unwrap().is_some());

    let page = repo
        .list_threads_for_conversation(conv.id, 10, PageDir::After, None)
        .await
        .unwrap();
    assert!(page.items.len() >= 2);
}

#[tokio::test]
async fn set_first_message_conflict_and_not_found() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(conn.clone());
    let msg_repo = SeaOrmMessageRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(ConversationType::Group, server.id)
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();

    let msg = msg_repo
        .create_message(&NewMessage {
            message_thread_id: thread.id,
            parent_message_id: None,
            sent_by: rim_core::user::model::UserId::new(),
            text: Some("t".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();

    thread_repo
        .set_first_message(thread.id, msg.id)
        .await
        .unwrap();

    let err = thread_repo
        .set_first_message(thread.id, MessageId::new())
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));

    let err = thread_repo
        .set_first_message(rim_core::message_thread::model::MessageThreadId::new(), msg.id)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}

#[tokio::test]
async fn touch_last_message_at_not_found() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let repo = SeaOrmMessageThreadRepository::new(conn.clone());
    let err = repo
        .touch_last_message_at(
            rim_core::message_thread::model::MessageThreadId::new(),
            chrono::Utc::now(),
        )
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));
}
