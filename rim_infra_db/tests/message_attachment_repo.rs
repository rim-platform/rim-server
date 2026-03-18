#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::model::{MessageAttachmentId, NewMessageAttachment};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::NewUser;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn add_get_and_list_attachments_for_message() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let server_repo = SeaOrmServerRepository::new(db.clone());
    let conv_repo = SeaOrmConversationRepository::new(db.clone());
    let thread_repo = SeaOrmMessageThreadRepository::new(db.clone());
    let msg_repo = SeaOrmMessageRepository::new(db.clone());
    let repo = SeaOrmMessageAttachmentRepository::new(db.clone());

    let user = user_repo
        .create_user(NewUser {
            username: "testuser".to_string(),
            password: "hash".to_string(),
        })
        .await
        .unwrap();
    let server = server_repo.create_server("s").await.unwrap();
    let conv = conv_repo
        .create_conversation(NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Group,
            title: None,
        })
        .await
        .unwrap();
    let thread = thread_repo.create_thread(conv.id).await.unwrap();
    let msg = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
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
        .add_attachment(NewMessageAttachment {
            message_id: msg.id,
            storage_url: "url".to_owned(),
            file_name: "file.txt".to_owned(),
            file_size: 123,
            mime_type: "text/plain".to_owned(),
            metadata: None,
        })
        .await
        .unwrap();

    let fetched = repo.get_by_id(att.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, att.id);

    let list = repo.get_attachments_for_message(msg.id).await.unwrap();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].id, att.id);

    let updated = repo
        .update_attachment(
            att.id,
            "attachment://ready".to_string(),
            Some(serde_json::json!({ "status": "ready" })),
        )
        .await
        .unwrap();
    assert_eq!(updated.storage_url, "attachment://ready");

    repo.delete_attachment(att.id).await.unwrap();
    assert!(repo.get_by_id(att.id).await.unwrap().is_none());
    assert!(
        repo.get_attachments_for_message(msg.id)
            .await
            .unwrap()
            .is_empty()
    );

    assert!(
        repo.get_by_id(MessageAttachmentId::new())
            .await
            .unwrap()
            .is_none()
    );
}
