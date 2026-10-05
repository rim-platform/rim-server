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
    use rim_core::message::repository::{
        ListMessagesParams, MessageCursor, MessageSearchHit, MessageSearchParams,
    };
    use rim_core::message_attachment::model::{
        AttachmentUsage, MessageAttachment, MessageAttachmentId, NewMessageAttachment,
    };
    use rim_core::repository::error::RepoResult;
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::storage::StoredFile;
    use rim_core::user::model::UserId;
    use rim_services::message_attachment::MAX_ATTACHMENT_BYTES;
    use rim_services::message_attachment::command::CompleteUploadCmd;
    use rim_services::message_attachment::command::{
        CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
    };
    use rim_services::message_attachment::dto::AttachmentStatus;
    use rim_services::message_attachment::service::MessageAttachmentService;
    use rim_services::storage::service::{MockStorageService, QuotaUsage};
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
            async fn try_complete_upload(
                &self,
                id: MessageAttachmentId,
                expected_storage_url: String,
                reserved_since: chrono::DateTime<chrono::Utc>,
                storage_url: String,
                file_size: i64,
                metadata: Option<serde_json::Value>,
            ) -> RepoResult<Option<MessageAttachment>>;
            async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;
            async fn usage_for_server(
                &self,
                server_id: ServerId,
                pending_since: chrono::DateTime<chrono::Utc>,
                max_file_size: i64,
            ) -> RepoResult<AttachmentUsage>;
            async fn find_ids_with_status_before(
                &self,
                statuses: Vec<String>,
                before: chrono::DateTime<chrono::Utc>,
            ) -> RepoResult<Vec<MessageAttachmentId>>;
        }
    }

    mock! {
        ConversationRepository {}
        #[async_trait]
        impl rim_core::conversation::repository::ConversationRepository for ConversationRepository {
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
            async fn get_by_ids(&self, ids: &[ConversationId]) -> RepoResult<Vec<Conversation>>;
            async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation>;
            async fn create_conversation_with_members(
                &self,
                conv: NewConversation,
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
                filter: rim_core::conversation::repository::ChannelListFilter,
                limit: u32,
                dir: PageDir,
                cursor: Option<ConversationCreatedCursor>,
            ) -> RepoResult<Page<Conversation, ConversationCreatedCursor>>;
            async fn exists_channel_by_title(
                &self,
                server_id: ServerId,
                title: &str,
            ) -> RepoResult<bool>;
            async fn get_channel_by_title(
                &self,
                server_id: ServerId,
                title: &str,
            ) -> RepoResult<Option<Conversation>>;
            async fn list_accessible_conversation_ids(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<Vec<ConversationId>>;
            async fn list_public_channel_ids(&self, server_id: ServerId) -> RepoResult<Vec<ConversationId>>;
            async fn list_joined_conversation_ids(
                &self,
                server_id: ServerId,
                user_id: UserId,
            ) -> RepoResult<Vec<ConversationId>>;
            async fn touch_last_activity_at(
                &self,
                conversation_id: ConversationId,
                at: chrono::DateTime<chrono::Utc>,
            ) -> RepoResult<()>;
            async fn update_topic(
                &self,
                conversation_id: ConversationId,
                topic: Option<String>,
            ) -> RepoResult<Conversation>;
            async fn set_archived(
                &self,
                conversation_id: ConversationId,
                archived_at: Option<chrono::DateTime<chrono::Utc>>,
            ) -> RepoResult<Conversation>;
            async fn set_slow_mode(&self, conversation_id: ConversationId, seconds: i32) -> RepoResult<Conversation>;
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
            async fn search_messages_fts(
                &self,
                params: MessageSearchParams,
            ) -> RepoResult<Vec<MessageSearchHit>>;
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
            async fn last_sent_at_by_author(&self, conversation_id: ConversationId, author: UserId) -> RepoResult<Option<chrono::DateTime<chrono::Utc>>>;
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
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn noop_storage() -> Arc<dyn rim_services::storage::service::StorageService> {
        Arc::new(MockStorageService::new())
    }

    fn conversation(server_id: ServerId, id: ConversationId) -> Conversation {
        Conversation {
            id,
            server_id,
            conversation_type: ConversationType::Group,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
        }
    }

    fn message(id: MessageId, conversation_id: ConversationId) -> Message {
        Message {
            id,
            conversation_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
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

    fn quota(used_bytes: u64, max_bytes: u64) -> QuotaUsage {
        QuotaUsage {
            used_bytes,
            max_bytes: Some(max_bytes),
            file_count: 0,
        }
    }

    /// A service whose create-attachment access checks all pass for `actor_id` on a message they
    /// sent in `conversation_id` of `server_id`.
    fn create_ready_service(
        server_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        actor_id: UserId,
        message_attachment_repo: MockMessageAttachmentRepository,
        storage: MockStorageService,
    ) -> DefaultMessageAttachmentService {
        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;
        let msg_for_set = msg.clone();

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conversation(server_id, conversation_id))));
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(msg.clone())));
        message_repo
            .expect_set_has_attachments()
            .returning(move |_, _| Ok(msg_for_set.clone()));
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            Arc::new(storage),
        )
    }

    fn create_cmd(
        server_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        actor_id: UserId,
        size_bytes: u64,
    ) -> CreateAttachmentCmd {
        CreateAttachmentCmd {
            actor: Actor::new(actor_id),
            server_id,
            conversation_id,
            message_id: Some(message_id),
            filename: "f.txt".to_string(),
            content_type: "text/plain".to_string(),
            size_bytes,
        }
    }

    /// A service whose upload access checks all pass for `actor_id` completing the pending
    /// attachment `attachment_id` (declared at 10 bytes, token `"tok"`).
    fn upload_ready_service(
        attachment_id: MessageAttachmentId,
        actor_id: UserId,
        mut message_attachment_repo: MockMessageAttachmentRepository,
        storage: MockStorageService,
    ) -> DefaultMessageAttachmentService {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut att = attachment(attachment_id);
        att.message_id = message_id;
        att.storage_url = "pending://tok".to_string();
        att.metadata = Some(serde_json::json!({ "status": "pending_upload" }));
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(att.clone())));

        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conversation(server_id, conversation_id))));
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(msg.clone())));
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            Arc::new(storage),
        )
    }

    fn upload_cmd(
        attachment_id: MessageAttachmentId,
        actor_id: UserId,
        data: &[u8],
    ) -> CompleteUploadCmd {
        CompleteUploadCmd {
            actor_user_id: actor_id,
            attachment_id,
            upload_token: "tok".to_string(),
            uploaded_filename: "f.txt".to_string(),
            content_type: "text/plain".to_string(),
            data: data.to_vec(),
        }
    }

    fn storing_storage() -> MockStorageService {
        let mut storage = MockStorageService::new();
        storage.expect_upload_file().returning(|key, data, _| {
            let stored = StoredFile {
                key: key.to_string(),
                size_bytes: data.len() as u64,
            };
            Box::pin(async move { Ok(stored) })
        });
        storage
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn create_attachment_rejects_upload_over_workspace_quota() {
        let (server_id, conversation_id, message_id, actor_id) = (
            ServerId::new(),
            ConversationId::new(),
            MessageId::new(),
            UserId::new(),
        );
        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_add_attachment().never();
        let mut storage = MockStorageService::new();
        storage
            .expect_get_quota_usage()
            .returning(|_| Box::pin(async { Ok(quota(995, 1_000)) }));

        let svc = create_ready_service(
            server_id,
            conversation_id,
            message_id,
            actor_id,
            repo,
            storage,
        );
        let err = svc
            .create_attachment(create_cmd(
                server_id,
                conversation_id,
                message_id,
                actor_id,
                10,
            ))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("quota")),
            "expected quota error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn create_attachment_accepts_upload_that_exactly_fills_quota() {
        let (server_id, conversation_id, message_id, actor_id) = (
            ServerId::new(),
            ConversationId::new(),
            MessageId::new(),
            UserId::new(),
        );
        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_add_attachment().times(1).returning(|new| {
            Ok(MessageAttachment {
                id: new.id,
                message_id: new.message_id,
                storage_url: new.storage_url,
                file_name: new.file_name,
                file_size: new.file_size,
                mime_type: new.mime_type,
                metadata: new.metadata,
            })
        });
        let mut storage = MockStorageService::new();
        storage
            .expect_get_quota_usage()
            .returning(|_| Box::pin(async { Ok(quota(990, 1_000)) }));

        let svc = create_ready_service(
            server_id,
            conversation_id,
            message_id,
            actor_id,
            repo,
            storage,
        );
        svc.create_attachment(create_cmd(
            server_id,
            conversation_id,
            message_id,
            actor_id,
            10,
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn create_attachment_rejects_declared_size_over_upload_limit() {
        let (server_id, conversation_id, message_id, actor_id) = (
            ServerId::new(),
            ConversationId::new(),
            MessageId::new(),
            UserId::new(),
        );
        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_add_attachment().never();

        let svc = create_ready_service(
            server_id,
            conversation_id,
            message_id,
            actor_id,
            repo,
            MockStorageService::new(),
        );
        let err = svc
            .create_attachment(create_cmd(
                server_id,
                conversation_id,
                message_id,
                actor_id,
                MAX_ATTACHMENT_BYTES + 1,
            ))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn complete_upload_stores_file_and_records_actual_size() {
        let attachment_id = MessageAttachmentId::new();
        let actor_id = UserId::new();

        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_try_complete_upload()
            .withf(|_, expected, _, key, size, _| {
                expected == "pending://tok" && key.starts_with("uploads/") && *size == 5
            })
            .times(1)
            .returning(|id, _, _, key, size, metadata| {
                Ok(Some(MessageAttachment {
                    id,
                    message_id: MessageId::new(),
                    storage_url: key,
                    file_name: "f.txt".to_string(),
                    file_size: size,
                    mime_type: "text/plain".to_string(),
                    metadata,
                }))
            });

        let svc = upload_ready_service(attachment_id, actor_id, repo, storing_storage());
        let dto = svc
            .complete_upload(upload_cmd(attachment_id, actor_id, b"hello"))
            .await
            .unwrap();
        assert_eq!(dto.size_bytes, 5);
        assert!(dto.storage_key.is_some_and(|k| k.ends_with("_f.txt")));
    }

    #[tokio::test]
    async fn complete_upload_rejects_file_larger_than_declared_without_storing() {
        let attachment_id = MessageAttachmentId::new();
        let actor_id = UserId::new();

        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_try_complete_upload().never();
        let mut storage = MockStorageService::new();
        storage.expect_upload_file().never();

        let svc = upload_ready_service(attachment_id, actor_id, repo, storage);
        // Declared at 10 bytes; 11 arrive.
        let err = svc
            .complete_upload(upload_cmd(attachment_id, actor_id, b"hello world"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn complete_upload_rejects_expired_ticket_without_storing() {
        // An attachment created 16 minutes ago: its 15-minute ticket has lapsed.
        let created = chrono::Utc::now() - chrono::Duration::minutes(16);
        let attachment_id = MessageAttachmentId::from(uuid::Uuid::new_v7(
            uuid::Timestamp::from_unix(uuid::NoContext, created.timestamp() as u64, 0),
        ));
        let actor_id = UserId::new();

        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_try_complete_upload().never();
        let mut storage = MockStorageService::new();
        storage.expect_upload_file().never();

        let svc = upload_ready_service(attachment_id, actor_id, repo, storage);
        let err = svc
            .complete_upload(upload_cmd(attachment_id, actor_id, b"hello"))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("expired")),
            "expected expiry error, got {err:?}"
        );
    }

    #[tokio::test]
    async fn complete_upload_losing_the_race_deletes_its_file() {
        let attachment_id = MessageAttachmentId::new();
        let actor_id = UserId::new();

        let mut repo = MockMessageAttachmentRepository::new();
        // Another request completed the attachment between our checks and our update.
        repo.expect_try_complete_upload()
            .returning(|_, _, _, _, _, _| Ok(None));
        let mut storage = storing_storage();
        storage
            .expect_delete_file()
            .withf(|key| key.starts_with("uploads/"))
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));

        let svc = upload_ready_service(attachment_id, actor_id, repo, storage);
        let err = svc
            .complete_upload(upload_cmd(attachment_id, actor_id, b"hello"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(None));

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

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
    async fn create_attachment_rejects_traversal_filename() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_cid, uid| Ok(uid == actor_id));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

        // Filenames containing ".." are rejected outright (not stripped).
        let err = svc
            .create_attachment(CreateAttachmentCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id: Some(message_id),
                filename: "..".to_string(),
                content_type: "text/plain".to_string(),
                size_bytes: 10,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));

        // Null bytes in filename should be stripped
        let err = svc
            .create_attachment(CreateAttachmentCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id: Some(message_id),
                filename: "\0\0".to_string(),
                content_type: "text/plain".to_string(),
                size_bytes: 10,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_attachment_requires_message_id_in_v0() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                message_repo: Arc::new(MockMessageRepository::new()),
                message_attachment_repo: Arc::new(MockMessageAttachmentRepository::new()),
                server_member_repo: Arc::new({
                    let mut repo = MockServerMemberRepository::new();
                    repo.expect_is_member().returning(|_, _| Ok(true));
                    repo.expect_get_member_standing().returning(|_, _| {
                        Ok(Some(rim_core::server_member::model::MemberStanding::new(
                            rim_core::server_member::model::ServerMemberRole::Member,
                        )))
                    });
                    repo
                }),
            },
            noop_storage(),
        );

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
        use std::sync::Mutex;

        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

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

        // Capture the service-generated attachment ID from add_attachment.
        let captured_id: Arc<Mutex<Option<MessageAttachmentId>>> = Arc::new(Mutex::new(None));
        let captured_id_for_add = Arc::clone(&captured_id);

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo.expect_add_attachment().return_once(
            move |att: NewMessageAttachment| {
                assert_eq!(att.message_id, message_id);
                assert!(att.storage_url.starts_with("pending://"));
                assert_eq!(att.file_name, "f.txt");
                assert_eq!(att.file_size, 10);
                assert_eq!(att.mime_type, "text/plain");
                assert_eq!(
                    att.metadata.as_ref().and_then(|v| v.get("status").cloned()),
                    Some(serde_json::json!("pending_upload"))
                );
                let service_id = att.id;
                *captured_id_for_add.lock().unwrap() = Some(service_id);
                Ok(MessageAttachment {
                    id: service_id,
                    message_id: att.message_id,
                    storage_url: att.storage_url,
                    file_name: att.file_name,
                    file_size: att.file_size,
                    mime_type: att.mime_type,
                    metadata: att.metadata,
                })
            },
        );

        let captured_id_for_get = Arc::clone(&captured_id);
        let get_call_count = Arc::new(std::sync::atomic::AtomicU32::new(0));
        message_attachment_repo
            .expect_get_by_id()
            .times(3)
            .returning(move |_req_id| {
                let att_id = captured_id_for_get.lock().unwrap().unwrap();
                let call_num = get_call_count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                // First call (from get_attachment): return pending status
                // Second/third calls (from finalize/delete): return uploaded status
                let (status, storage_url) = if call_num == 0 {
                    ("pending_upload", format!("pending://upload-{att_id}"))
                } else {
                    ("uploaded", format!("uploads/{att_id}/f.txt"))
                };
                Ok(Some(MessageAttachment {
                    id: att_id,
                    message_id,
                    storage_url,
                    file_name: "f.txt".to_string(),
                    file_size: 10,
                    mime_type: "text/plain".to_string(),
                    metadata: Some(serde_json::json!({ "status": status })),
                }))
            });

        let captured_id_for_update = Arc::clone(&captured_id);
        message_attachment_repo
            .expect_update_attachment()
            .return_once(move |id, storage_url, metadata| {
                let att_id = captured_id_for_update.lock().unwrap().unwrap();
                assert_eq!(id, att_id);
                assert_eq!(
                    metadata.and_then(|v| v.get("status").cloned()),
                    Some(serde_json::json!("ready"))
                );
                Ok(MessageAttachment {
                    id: att_id,
                    message_id,
                    storage_url,
                    file_name: "f.txt".to_string(),
                    file_size: 10,
                    mime_type: "text/plain".to_string(),
                    metadata: Some(serde_json::json!({ "status": "ready" })),
                })
            });

        let captured_id_for_delete = Arc::clone(&captured_id);
        message_attachment_repo
            .expect_delete_attachment()
            .return_once(move |id| {
                let att_id = captured_id_for_delete.lock().unwrap().unwrap();
                assert_eq!(id, att_id);
                Ok(())
            });
        message_attachment_repo
            .expect_get_attachments_for_message()
            .return_once(move |_mid| Ok(vec![]));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .times(1)
            .returning(|_sid, _uid| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .times(4)
            .returning(|_sid, _uid| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let mut storage = MockStorageService::new();
        storage
            .expect_delete_file()
            .returning(|_key| Box::pin(async { Ok(()) }));
        storage
            .expect_get_quota_usage()
            .returning(|_| Box::pin(async { Ok(quota(0, 1_000)) }));

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            Arc::new(storage),
        );

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

        let attachment_id = ticket.attachment_id;
        assert!(!ticket.upload_token.is_empty());

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

    #[tokio::test]
    async fn complete_upload_rejects_non_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let attachment_id = MessageAttachmentId::new();
        let upload_token = "valid-token".to_string();

        let mut att = attachment(attachment_id);
        att.message_id = message_id;
        att.storage_url = format!("pending://{upload_token}");
        att.metadata = Some(serde_json::json!({ "status": "pending_upload" }));

        let mut msg = message(message_id, conversation_id);
        msg.sent_by = actor_id;

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conversation(server_id, conversation_id))));

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(att.clone())));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(msg.clone())));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(false));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(None));

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

        let err = svc
            .complete_upload(
                rim_services::message_attachment::command::CompleteUploadCmd {
                    actor_user_id: actor_id,
                    attachment_id,
                    upload_token,
                    uploaded_filename: "f.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    data: b"hello".to_vec(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn complete_upload_rejects_wrong_owner() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let other_user = UserId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let attachment_id = MessageAttachmentId::new();
        let upload_token = "valid-token".to_string();

        let mut att = attachment(attachment_id);
        att.message_id = message_id;
        att.storage_url = format!("pending://{upload_token}");
        att.metadata = Some(serde_json::json!({ "status": "pending_upload" }));

        let mut msg = message(message_id, conversation_id);
        msg.sent_by = other_user;

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conversation(server_id, conversation_id))));

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(att.clone())));

        let mut message_repo = MockMessageRepository::new();
        message_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(msg.clone())));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_, _| Ok(true));

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                message_repo: Arc::new(message_repo),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(server_member_repo),
            },
            noop_storage(),
        );

        let err = svc
            .complete_upload(
                rim_services::message_attachment::command::CompleteUploadCmd {
                    actor_user_id: actor_id,
                    attachment_id,
                    upload_token,
                    uploaded_filename: "f.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    data: b"hello".to_vec(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn complete_upload_rejects_invalid_token() {
        let attachment_id = MessageAttachmentId::new();

        let mut att = attachment(attachment_id);
        att.storage_url = "pending://correct-token".to_string();

        let mut message_attachment_repo = MockMessageAttachmentRepository::new();
        message_attachment_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(att.clone())));

        let svc = DefaultMessageAttachmentService::new(
            AttachmentRepos {
                conversation_repo: Arc::new(MockConversationRepository::new()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                message_repo: Arc::new(MockMessageRepository::new()),
                message_attachment_repo: Arc::new(message_attachment_repo),
                server_member_repo: Arc::new(MockServerMemberRepository::new()),
            },
            noop_storage(),
        );

        let err = svc
            .complete_upload(
                rim_services::message_attachment::command::CompleteUploadCmd {
                    actor_user_id: UserId::new(),
                    attachment_id,
                    upload_token: "wrong-token".to_string(),
                    uploaded_filename: "f.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    data: b"hello".to_vec(),
                },
            )
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }
}
