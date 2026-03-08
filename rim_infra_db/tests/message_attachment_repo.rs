#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::model::MessageAttachmentId;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::server::repository::ServerRepository;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;

#[tokio::test]
async fn add_get_and_list_attachments_for_message() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let conv_repo = SeaOrmConversationRepository::new(conn.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(conn.clone());
    let msg_repo = SeaOrmMessageRepository::new(conn.clone());
    let repo = SeaOrmMessageAttachmentRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(ConversationType::Group, server.id)
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();
    let msg = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: rim_core::user::model::UserId::new(),
            text: Some("t".to_string()),
            attachments: vec![],
        })
        .await
        .unwrap();

    assert_eq!(
        repo.get_attachments_for_message(msg.id)
            .await
            .unwrap()
            .len(),
        0
    );

    let att = repo
        .add_attachment(msg.id, "url", "file.txt", 123, "text/plain", None)
        .await
        .unwrap();

    let fetched = repo.get_by_id(att.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, att.id);

    let list = repo.get_attachments_for_message(msg.id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, att.id);

    assert!(
        repo.get_by_id(MessageAttachmentId::new())
            .await
            .unwrap()
            .is_none()
    );
}
