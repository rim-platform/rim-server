#![cfg(feature = "db-tests")]

mod support;

use rim_core::conversation::model::ConversationType;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::model::{
    AttachmentUsage, MessageAttachmentId, NewMessageAttachment,
};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository;

#[tokio::test]
async fn add_get_and_list_attachments_for_message() {
    let ctx = support::TestCtx::new().await;

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
            blob_sha256: None,
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
    let ctx = support::TestCtx::new().await;

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
        blob_sha256: None,
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
        blob_sha256: None,
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
        blob_sha256: None,
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
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let map = repo.get_attachments_for_messages(&[]).await.unwrap();
    assert!(map.is_empty());
}

const MAX_FILE: i64 = 25 * 1024 * 1024;

/// An attachment id whose embedded creation time is `minutes_ago` in the past.
fn id_from_minutes_ago(minutes_ago: i64) -> MessageAttachmentId {
    let at = chrono::Utc::now() - chrono::Duration::minutes(minutes_ago);
    MessageAttachmentId::from(uuid::Uuid::new_v7(uuid::Timestamp::from_unix(
        uuid::NoContext,
        at.timestamp() as u64,
        0,
    )))
}

fn window_start() -> chrono::DateTime<chrono::Utc> {
    chrono::Utc::now() - chrono::Duration::minutes(15)
}

fn attachment_for(
    message_id: rim_core::message::model::MessageId,
    storage_url: &str,
    file_size: i64,
    status: &str,
) -> NewMessageAttachment {
    NewMessageAttachment {
        id: MessageAttachmentId::new(),
        message_id,
        storage_url: storage_url.to_owned(),
        file_name: "f.bin".to_owned(),
        file_size,
        mime_type: "application/octet-stream".to_owned(),
        metadata: Some(serde_json::json!({ "status": status })),
        blob_sha256: None,
    }
}

#[tokio::test]
async fn usage_for_server_counts_stored_and_live_pending_only() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let user = ctx.create_user("usage-user").await;
    let server = ctx.create_server("usage").await;
    let other_server = ctx.create_server("other").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let other_conv = ctx
        .create_conversation(other_server.id, ConversationType::Group)
        .await;
    let msg = ctx.create_message(conv.id, user.id, "m").await;
    let other_msg = ctx.create_message(other_conv.id, user.id, "o").await;

    assert_eq!(
        repo.usage_for_server(server.id, window_start(), MAX_FILE)
            .await
            .unwrap(),
        AttachmentUsage::default()
    );

    repo.add_attachment(attachment_for(msg.id, "uploads/a", 100, "ready"))
        .await
        .unwrap();
    repo.add_attachment(attachment_for(msg.id, "pending://t1", 20, "pending_upload"))
        .await
        .unwrap();
    // A reservation the stale cleanup failed before any file arrived holds no storage.
    repo.add_attachment(attachment_for(msg.id, "pending://t2", 5_000, "failed"))
        .await
        .unwrap();
    // A reservation whose ticket lapsed without an upload holds no storage either.
    let mut expired = attachment_for(msg.id, "pending://t3", 7_000, "pending_upload");
    expired.id = id_from_minutes_ago(16);
    repo.add_attachment(expired).await.unwrap();
    // A stored file older than the window still counts, clamped to the per-file maximum.
    let mut legacy = attachment_for(msg.id, "uploads/legacy", i64::MAX, "ready");
    legacy.id = id_from_minutes_ago(60 * 24);
    repo.add_attachment(legacy).await.unwrap();
    repo.add_attachment(attachment_for(other_msg.id, "uploads/b", 7, "ready"))
        .await
        .unwrap();

    assert_eq!(
        repo.usage_for_server(server.id, window_start(), MAX_FILE)
            .await
            .unwrap(),
        AttachmentUsage {
            total_bytes: 120 + MAX_FILE as u64,
            count: 3,
        }
    );
}

#[tokio::test]
async fn try_complete_upload_succeeds_once_per_token() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let user = ctx.create_user("complete-user").await;
    let server = ctx.create_server("complete").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let msg = ctx.create_message(conv.id, user.id, "m").await;
    let att = repo
        .add_attachment(attachment_for(
            msg.id,
            "pending://tok",
            50,
            "pending_upload",
        ))
        .await
        .unwrap();

    let done = repo
        .try_complete_upload(
            att.id,
            "pending://tok".to_string(),
            window_start(),
            "uploads/first".to_string(),
            42,
            Some(serde_json::json!({ "status": "uploaded" })),
        )
        .await
        .unwrap()
        .expect("first completion wins");
    assert_eq!(done.storage_url, "uploads/first");
    assert_eq!(done.file_size, 42);

    let second = repo
        .try_complete_upload(
            att.id,
            "pending://tok".to_string(),
            window_start(),
            "uploads/second".to_string(),
            42,
            None,
        )
        .await
        .unwrap();
    assert!(second.is_none(), "the token is spent after one completion");
    let stored = repo.get_by_id(att.id).await.unwrap().unwrap();
    assert_eq!(stored.storage_url, "uploads/first");
}

#[tokio::test]
async fn try_complete_upload_refuses_an_expired_reservation() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmMessageAttachmentRepository::new(ctx.db.clone());

    let user = ctx.create_user("expired-user").await;
    let server = ctx.create_server("expired").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Group)
        .await;
    let msg = ctx.create_message(conv.id, user.id, "m").await;
    let mut pending = attachment_for(msg.id, "pending://late", 50, "pending_upload");
    pending.id = id_from_minutes_ago(16);
    let att = repo.add_attachment(pending).await.unwrap();

    let res = repo
        .try_complete_upload(
            att.id,
            "pending://late".to_string(),
            window_start(),
            "uploads/late".to_string(),
            50,
            None,
        )
        .await
        .unwrap();
    assert!(res.is_none(), "a lapsed reservation can't be filled");
    let stored = repo.get_by_id(att.id).await.unwrap().unwrap();
    assert_eq!(stored.storage_url, "pending://late");
}
