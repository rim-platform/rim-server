#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use mockall::mock;

    use rim_core::conversation::model::{
        Conversation, ConversationId, ConversationType, NewConversation,
    };
    use rim_core::conversation::repository::{
        ConversationActivityCursor, ConversationCreatedCursor,
    };
    use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message::repository::{ListMessagesParams, MessageCursor};
    use rim_core::message_attachment::model::{
        MessageAttachment, MessageAttachmentId, NewMessageAttachment,
    };
    use rim_core::repository::error::RepoResult;
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::UserId;
    use rim_services::message_attachment::command::{
        CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
    };
    use rim_services::message_attachment::dto::AttachmentStatus;
    use rim_services::message_attachment::service::MessageAttachmentService;
    use rim_services::{Actor, ServiceError};

    use crate::message_attachment::DefaultMessageAttachmentService;
    use crate::repos::AttachmentRepos;

    // ── Inline mock definitions ──────────────────────────────────────────

    mock! {
        MessageAttachmentRepository {}
        #[async_trait]
        impl rim_core::message_attachment::repository::MessageAttachmentRepository for MessageAttachmentRepository {
            async fn get_by_id(&self, id: MessageAttachmentId) -> RepoResult<Option<MessageAttachment>>;
            async fn add_attachment(
                &self,
                attachment: NewMessageAttachment,
            ) -> RepoResult<MessageAttachment>;
            async fn get_attachments_for_message(
                &self,
                message_id: MessageId,
            ) -> RepoResult<Vec<MessageAttachment>>;
            async fn get_attachments_for_messages(
                &self,
                message_ids: &[MessageId],
            ) -> RepoResult<std::collections::HashMap<MessageId, Vec<MessageAttachment>>>;
            async fn update_attachment(
                &self,
                id: MessageAttachmentId,
                storage_url: String,
                metadata: Option<serde_json::Value>,
            ) -> RepoResult<MessageAttachment>;
            async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;
        }
    }

    mock! {
        ConversationRepository {}
        #[async_trait]
        impl rim_core::conversation::repository::ConversationRepository for ConversationRepository {
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
            async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation>;
            async fn create_conversation_with_members(
                &self,
                conversation_type: ConversationType,
                server_id: ServerId,
                title: Option<String>,
                member_ids: &[UserId],
            ) -> RepoResult<Conversation>;
            async fn find_direct_between_users(
                &self,
                server_id: ServerId,
                user_a: UserId,
                user_b: UserId,
            ) -> RepoResult<Option<Conversation>>;
            async fn list_inbox_conversations_for_user(
                &self,
                user_id: UserId,
                server_id: ServerId,
                limit: u32,
                dir: PageDir,
                cursor: Option<ConversationActivityCursor>,
            ) -> RepoResult<Page<Conversation, ConversationActivityCursor>>;
            async fn list_channels_for_user(
                &self,
                user_id: UserId,
                server_id: ServerId,
                limit: u32,
                dir: PageDir,
                cursor: Option<ConversationCreatedCursor>,
            ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;
            async fn touch_last_activity_at(
                &self,
                conversation_id: ConversationId,
                at: chrono::DateTime<chrono::Utc>,
            ) -> RepoResult<()>;
        }
    }

    mock! {
        ConversationMemberRepository {}
        #[async_trait]
        impl rim_core::conversation_member::repository::ConversationMemberRepository for ConversationMemberRepository {
            async fn get_by_id(&self, id: ConversationMemberId) -> RepoResult<Option<ConversationMember>>;
            async fn add_member(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<ConversationMember>;
            async fn remove_member(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<()>;
            async fn get_members_for_conversation(
                &self,
                conversation_id: ConversationId,
            ) -> RepoResult<Vec<ConversationMember>>;
            async fn is_member(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<bool>;
            async fn list_member_ids(
                &self,
                conversation_id: ConversationId,
            ) -> RepoResult<Vec<UserId>>;
            async fn list_member_ids_for_conversations(
                &self,
                conversation_ids: &[ConversationId],
            ) -> RepoResult<std::collections::HashMap<ConversationId, Vec<UserId>>>;
        }
    }

    mock! {
        MessageRepository {}
        #[async_trait]
        impl rim_core::message::repository::MessageRepository for MessageRepository {
            async fn get_by_id(&self, id: MessageId) -> RepoResult<Option<Message>>;
            async fn get_by_ids(&self, ids: &[MessageId]) -> RepoResult<std::collections::HashMap<MessageId, Message>>;
            async fn create_message(&self, new_message: &NewMessage) -> RepoResult<Message>;
            async fn list_messages(
                &self,
                params: ListMessagesParams,
            ) -> RepoResult<Page<Message, MessageCursor>>;
            async fn search_messages(
                &self,
                conversation_id: ConversationId,
                query: &str,
                limit: u32,
                include_deleted: bool,
            ) -> RepoResult<Vec<Message>>;
            async fn update_message_text(
                &self,
                message_id: MessageId,
                new_text: String,
            ) -> RepoResult<Message>;
            async fn set_has_attachments(
                &self,
                message_id: MessageId,
                has_attachments: bool,
            ) -> RepoResult<Message>;
            async fn soft_delete_message(&self, message_id: MessageId) -> RepoResult<()>;
        }
    }

    mock! {
        ServerMemberRepository {}
        #[async_trait]
        impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepository {
            async fn get_by_id(&self, id: ServerMemberId) -> RepoResult<Option<ServerMember>>;
            async fn get_member(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<Option<ServerMember>>;
            async fn add_member(
                &self,
                server_id: ServerId,
                user_id: UserId,
                role: ServerMemberRole,
            ) -> RepoResult<ServerMember>;
            async fn change_member_role(
                &self,
                server_id: ServerId,
                user_id: UserId,
                role: ServerMemberRole,
            ) -> RepoResult<()>;
            async fn remove_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<()>;
            async fn get_members_for_server(&self, server_id: ServerId) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

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

    fn message(id: MessageId, conversation_id: ConversationId) -> Message {
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
            message_id: MessageId::new(),
            storage_url: "u".to_string(),
            file_name: "f.txt".to_string(),
            file_size: 10,
            mime_type: "text/plain".to_string(),
            metadata: None,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn get_attachment_denies_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let message_attachment_repo = MockMessageAttachmentRepository::new();
        let conversation_repo = MockConversationRepository::new();
        let conversation_member_repo = MockConversationMemberRepository::new();
        let message_repo = MockMessageRepository::new();
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
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

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(|_id| Ok(None));

        let conversation_repo = MockConversationRepository::new();
        let conversation_member_repo = MockConversationMemberRepository::new();
        let message_repo = MockMessageRepository::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
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

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_conversation_id, user_id| Ok(user_id == actor_id));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(att.clone())));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
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

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_conversation_id, _user_id| Ok(false));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(att.clone())));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
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

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            message_repo: Arc::new(MockMessageRepository::new()),
            message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
            server_member_repo: Arc::new({
                let mut repo = MockServerMemberRepository::new();
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
        let message_id = MessageId::new();
        let attachment_id = MessageAttachmentId::new();

        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;
        msg.has_attachments = false;

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .times(4)
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .times(4)
            .returning(move |_conversation_id, user_id| Ok(user_id == actor_id));

        let msg_for_create = msg.clone();
        let msg_for_finalize = msg.clone();
        let msg_for_delete = msg.clone();
        let mut message_repo = MockMessageRepository::new();
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

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo.expect_add_attachment().return_once(
            move |att: NewMessageAttachment| {
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

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .times(4)
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultMessageAttachmentService::new(AttachmentRepos {
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
