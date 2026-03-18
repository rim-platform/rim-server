#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::message::model::Message;
    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::message_attachment::command::{
        CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
    };
    use rim_services::message_attachment::dto::AttachmentStatus;
    use rim_services::message_attachment::service::MessageAttachmentService;
    use rim_services::{Actor, ServiceError};

    use crate::message_attachment::DefaultMessageAttachmentService;
    use crate::test_support::attachment::{
        AttachmentReposPorts, MockConversationMemberRepo, MockConversationRepo,
        MockMessageAttachmentRepo, MockMessageRepo, MockServerMemberRepo,
    };

    fn conversation(server_id: ServerId, id: ConversationId) -> Conversation {
        Conversation {
            id,
            server_id,
            conversation_type: ConversationType::Group,
            title: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    fn message(
        id: rim_core::message::model::MessageId,
        conversation_id: ConversationId,
    ) -> Message {
        Message {
            id,
            conversation_id,
            thread_id: None,
            parent_message_id: None,
            sent_by: UserId::new(),
            text: Some("hello".to_string()),
            has_attachments: true,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    fn attachment(id: MessageAttachmentId) -> MessageAttachment {
        MessageAttachment {
            id,
            message_id: rim_core::message::model::MessageId::new(),
            storage_url: "u".to_string(),
            file_name: "f.txt".to_string(),
            file_size: 10,
            mime_type: "text/plain".to_string(),
            metadata: None,
        }
    }

    #[tokio::test]
    async fn get_attachment_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let message_attachment_repo = MockMessageAttachmentRepo::new();
        let conversation_repo = MockConversationRepo::new();
        let conversation_member_repo = MockConversationMemberRepo::new();
        let message_repo = MockMessageRepo::new();
        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: MessageAttachmentId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn get_attachment_returns_not_found_when_missing() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(|_id| Ok(None));

        let conversation_repo = MockConversationRepo::new();
        let conversation_member_repo = MockConversationMemberRepo::new();
        let message_repo = MockMessageRepo::new();

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: MessageAttachmentId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn get_attachment_maps_fields() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let id = MessageAttachmentId::new();
        let att = attachment(id);
        let conversation_id = ConversationId::new();
        let msg = message(att.message_id, conversation_id);

        let mut conversation_repo = MockConversationRepo::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_conversation_id, user_id| Ok(user_id == actor_id));

        let mut message_repo = MockMessageRepo::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(att.clone())));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: id,
            })
            .await
            .unwrap();
        assert_eq!(dto.id, id);
        assert_eq!(dto.filename, "f.txt");
        assert_eq!(dto.size_bytes, 10);
        assert_eq!(dto.status, AttachmentStatus::Ready);
    }

    #[tokio::test]
    async fn get_attachment_denies_non_conversation_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let id = MessageAttachmentId::new();
        let att = attachment(id);
        let conversation_id = ConversationId::new();
        let msg = message(att.message_id, conversation_id);

        let mut conversation_repo = MockConversationRepo::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_conversation_id, _user_id| Ok(false));

        let mut message_repo = MockMessageRepo::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(att.clone())));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id: id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_attachment_requires_message_id_in_v0() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(MockConversationRepo::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepo::new()),
            message_repo: Arc::new(MockMessageRepo::new()),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepo::new()),
            server_member_repo: Arc::new({
                let mut repo = MockServerMemberRepo::new();
                repo.expect_is_member().returning(|_, _| Ok(true));
                repo
            }),
        });

        let err = svc
            .create_attachment(CreateAttachmentCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                message_id: None,
                filename: "f.txt".to_string(),
                content_type: "text/plain".to_string(),
                size_bytes: 10,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_finalize_and_delete_attachment_manage_message_state() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let message_id = rim_core::message::model::MessageId::new();
        let attachment_id = MessageAttachmentId::new();

        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;
        msg.has_attachments = false;

        let mut conversation_repo = MockConversationRepo::new();
        conversation_repo
            .expect_get_by_id()
            .times(4)
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepo::new();
        conversation_member_repo
            .expect_is_member()
            .times(4)
            .returning(move |_conversation_id, user_id| Ok(user_id == actor_id));

        let msg_for_create = msg.clone();
        let msg_for_finalize = msg.clone();
        let msg_for_delete = msg.clone();
        let mut message_repo = MockMessageRepo::new();
        message_repo
            .expect_get_by_id()
            .times(4)
            .returning(move |_id| {
                let value = if _id == message_id {
                    Some(msg_for_create.clone())
                } else {
                    Some(msg_for_finalize.clone())
                };
                Ok(value)
            });
        let msg_after_set_true = {
            let mut m = msg.clone();
            m.has_attachments = true;
            m
        };
        message_repo
            .expect_set_has_attachments()
            .withf(move |id, has| *id == message_id && *has)
            .return_once(move |_, _| Ok(msg_after_set_true));
        message_repo
            .expect_set_has_attachments()
            .withf(move |id, has| *id == message_id && !*has)
            .return_once(move |_, _| Ok(msg_for_delete.clone()));

        let pending_att = MessageAttachment {
            id: attachment_id,
            message_id,
            storage_url: "pending://upload".to_string(),
            file_name: "f.txt".to_string(),
            file_size: 10,
            mime_type: "text/plain".to_string(),
            metadata: Some(serde_json::json!({ "status": "pending_upload" })),
        };
        let ready_att = MessageAttachment {
            metadata: Some(serde_json::json!({ "status": "ready" })),
            storage_url: format!("attachment://{attachment_id}"),
            ..pending_att.clone()
        };

        let add_result = pending_att.clone();
        let pending_for_get = pending_att.clone();
        let pending_for_finalize = pending_att.clone();
        let ready_for_update = ready_att.clone();

        let mut message_attachment_repo = MockMessageAttachmentRepo::new();
        message_attachment_repo.expect_add_attachment().return_once(
            move |att: rim_core::message_attachment::model::NewMessageAttachment| {
                assert_eq!(att.message_id, message_id);
                assert!(att.storage_url.starts_with("pending://upload-"));
                assert_eq!(att.file_name, "f.txt");
                assert_eq!(att.file_size, 10);
                assert_eq!(att.mime_type, "text/plain");
                assert_eq!(
                    att.metadata.and_then(|v| v.get("status").cloned()),
                    Some(serde_json::json!("pending_upload"))
                );
                Ok(add_result)
            },
        );
        message_attachment_repo
            .expect_get_by_id()
            .times(3)
            .returning(move |_id| {
                if _id == attachment_id {
                    Ok(Some(pending_for_get.clone()))
                } else {
                    Ok(Some(pending_for_finalize.clone()))
                }
            });
        message_attachment_repo
            .expect_update_attachment()
            .return_once(move |id, storage_url, metadata| {
                assert_eq!(id, attachment_id);
                assert_eq!(storage_url, format!("attachment://{attachment_id}"));
                assert_eq!(
                    metadata.and_then(|v| v.get("status").cloned()),
                    Some(serde_json::json!("ready"))
                );
                Ok(ready_for_update)
            });
        message_attachment_repo
            .expect_delete_attachment()
            .return_once(move |id| {
                assert_eq!(id, attachment_id);
                Ok(())
            });
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_mid| Ok(vec![]));

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .times(4)
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new_for_tests(AttachmentReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_attachment_repo: Arc::new(message_attachment_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let ticket = svc
            .create_attachment(CreateAttachmentCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id: Some(message_id),
                filename: "f.txt".to_string(),
                content_type: "text/plain".to_string(),
                size_bytes: 10,
            })
            .await
            .unwrap();
        assert_eq!(ticket.attachment_id, attachment_id);
        assert!(ticket.upload_token.starts_with("upload-"));

        let pending = svc
            .get_attachment(GetAttachmentQuery {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id,
            })
            .await
            .unwrap();
        assert_eq!(pending.status, AttachmentStatus::PendingUpload);

        let finalized = svc
            .finalize_attachment(FinalizeAttachmentCmd {
                actor: Actor::new(actor_id),
                server_id,
                attachment_id,
            })
            .await
            .unwrap();
        assert_eq!(finalized.status, AttachmentStatus::Ready);

        svc.delete_attachment(DeleteAttachmentCmd {
            actor: Actor::new(actor_id),
            server_id,
            attachment_id,
        })
        .await
        .unwrap();
    }
}
