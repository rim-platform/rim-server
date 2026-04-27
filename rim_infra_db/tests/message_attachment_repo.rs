#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::model::{MessageAttachmentId, NewMessageAttachment};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository;

#[tokio::test]
async fn add_get_and_list_attachments_for_message() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let msg_repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let user = ctx.create_user("testuser").await;
    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let thread = ctx.create_thread(conv.id).await;
    let msg = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
            text: Some("t".to_string()),
            attachments: vec![],
            forwarded_from_message_id: None,
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
            id: MessageAttachmentId::new(),
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

#[tokio::test]
async fn get_attachments_for_messages_batch() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let msg_repo = SeaOrmMessageRepository::new(ctx.db.clone());
    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let user = ctx.create_user("batch-att-user").await;
    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let thread = ctx.create_thread(conv.id).await;

    let msg1 = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
            text: Some("msg1".to_string()),
            attachments: vec![],
            forwarded_from_message_id: None,
        })
        .await
        .unwrap();

    let msg2 = msg_repo
        .create_message(&NewMessage {
            conversation_id: conv.id,
            thread_id: Some(thread.id),
            parent_message_id: None,
            sent_by: user.id,
            text: Some("msg2".to_string()),
            attachments: vec![],
            forwarded_from_message_id: None,
        })
        .await
        .unwrap();

    // Add 2 attachments to msg1
    repo.add_attachment(NewMessageAttachment {
        id: MessageAttachmentId::new(),
        message_id: msg1.id,
        storage_url: "url1".to_owned(),
        file_name: "a.txt".to_owned(),
        file_size: 10,
        mime_type: "text/plain".to_owned(),
        metadata: None,
    })
    .await
    .unwrap();

    repo.add_attachment(NewMessageAttachment {
        id: MessageAttachmentId::new(),
        message_id: msg1.id,
        storage_url: "url2".to_owned(),
        file_name: "b.txt".to_owned(),
        file_size: 20,
        mime_type: "text/plain".to_owned(),
        metadata: None,
    })
    .await
    .unwrap();

    // Add 1 attachment to msg2
    repo.add_attachment(NewMessageAttachment {
        id: MessageAttachmentId::new(),
        message_id: msg2.id,
        storage_url: "url3".to_owned(),
        file_name: "c.txt".to_owned(),
        file_size: 30,
        mime_type: "text/plain".to_owned(),
        metadata: None,
    })
    .await
    .unwrap();

    let map = repo
        .get_attachments_for_messages(&[msg1.id, msg2.id])
        .await
        .unwrap();
    assert_eq!(map.get(&msg1.id).map(|v| v.len()).unwrap_or(0), 2);
    assert_eq!(map.get(&msg2.id).map(|v| v.len()).unwrap_or(0), 1);
    let total: usize = map.values().map(|v| v.len()).sum();
    assert_eq!(total, 3);
}

#[tokio::test]
async fn get_attachments_for_messages_empty_slice() {
    let Some(ctx) = support::TestCtx::new().await else {
        return;
    };

    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let map = repo.get_attachments_for_messages(&[]).await.unwrap();
    assert!(map.is_empty());
}
