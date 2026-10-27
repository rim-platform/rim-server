#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;
    use mockall::mock;

    use rim_core::conversation::model::{
        Conversation, ConversationId, ConversationType, NewConversation, PostingPolicy,
    };
    use rim_core::conversation::repository::{
        ConversationActivityCursor, ConversationCreatedCursor,
    };
    use rim_core::conversation_member::model::ConversationMember;
    use rim_core::message::model::{Message, MessageId, NewMessage};
    use rim_core::message::repository::{
        ListMessagesParams, MessageCursor, MessageSearchHit, MessageSearchParams,
    };
    use rim_core::message_thread::model::{MessageThread, MessageThreadId};
    use rim_core::repository::error::RepoResult;
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberRole};
    use rim_core::user::model::UserId;
    use rim_services::message_thread::command::{GetThreadByRootMessageQuery, GetThreadQuery};
    use rim_services::message_thread::service::ThreadService;
    use rim_services::{Actor, ServiceError};

    use crate::repos::ThreadRepos;
    use crate::thread::DefaultThreadService;

    // ── Inline mock definitions ──────────────────────────────────────────

    mock! {
        ConversationRepository {}
        #[async_trait]
        impl rim_core::conversation::repository::ConversationRepository for ConversationRepository {
            async fn count_channels(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
            async fn count_all_channels(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
            async fn get_by_id(&self, id: ConversationId) -> RepoResult<Option<Conversation>>;
            async fn get_by_ids(&self, ids: &[ConversationId]) -> RepoResult<Vec<Conversation>>;
            async fn create_conversation(&self, conv: NewConversation) -> RepoResult<Conversation>;
            async fn create_conversation_with_members(
                &self,
                conv: NewConversation,
                member_ids: &[UserId],
                pending_ids: &[UserId],
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
            async fn set_posting(&self, conversation_id: ConversationId, policy: PostingPolicy, allows_threads: bool) -> RepoResult<Conversation>;
        }
    }

    mock! {
        ConversationMemberRepository {}
        #[async_trait]
        impl rim_core::conversation_member::repository::ConversationMemberRepository for ConversationMemberRepository {
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
            async fn add_pending_member(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<ConversationMember>;
            async fn membership(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<Option<rim_core::conversation_member::model::Membership>>;
            async fn list_pending_member_ids(
                &self,
                conversation_id: ConversationId,
            ) -> RepoResult<Vec<UserId>>;
            async fn list_pending_member_ids_for_conversations(
                &self,
                conversation_ids: &[ConversationId],
            ) -> RepoResult<std::collections::HashMap<ConversationId, Vec<UserId>>>;
            async fn list_pending_conversation_ids(
                &self,
                user_id: UserId,
            ) -> RepoResult<Vec<ConversationId>>;
            async fn delete_pending_before(
                &self,
                cutoff: chrono::DateTime<chrono::Utc>,
            ) -> RepoResult<u64>;
            async fn activate_member(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<bool>;
            async fn set_invited_by(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
                invited_by: UserId,
            ) -> RepoResult<bool>;
            async fn invited_by(
                &self,
                conversation_id: ConversationId,
                user_id: UserId,
            ) -> RepoResult<Option<UserId>>;
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
            async fn list_message_changes(&self, conversation_id: ConversationId, after_version: i64, limit: u32) -> RepoResult<Vec<rim_core::message::repository::MessageChange>>;
            async fn latest_change_version(&self, conversation_id: ConversationId) -> RepoResult<i64>;
            async fn last_sent_at_by_author(&self, conversation_id: ConversationId, author: UserId) -> RepoResult<Option<chrono::DateTime<chrono::Utc>>>;
            async fn list_thread_participant_ids(&self, thread_id: rim_core::message_thread::model::MessageThreadId) -> RepoResult<Vec<UserId>>;
        }
    }

    mock! {
        MessageThreadRepository {}
        #[async_trait]
        impl rim_core::message_thread::repository::MessageThreadRepository for MessageThreadRepository {
            async fn get_by_id(&self, id: MessageThreadId) -> RepoResult<Option<MessageThread>>;
            async fn get_by_ids(&self, ids: &[MessageThreadId]) -> RepoResult<Vec<MessageThread>>;
            async fn get_by_root_message(&self, message_id: MessageId) -> RepoResult<Option<MessageThread>>;
            async fn get_thread_ids_by_root_messages(
                &self,
                message_ids: &[MessageId],
            ) -> RepoResult<std::collections::HashMap<MessageId, MessageThreadId>>;
            async fn create_thread_for_root(
                &self,
                conversation_id: ConversationId,
                message_id: MessageId,
            ) -> RepoResult<MessageThread>;
            async fn touch_last_message_at(
                &self,
                thread_id: MessageThreadId,
                at: chrono::DateTime<chrono::Utc>,
            ) -> RepoResult<()>;
            async fn count_live_replies(
                &self,
                thread_ids: &[MessageThreadId],
            ) -> RepoResult<std::collections::HashMap<MessageThreadId, i64>>;
        }
    }

    mock! {
        ServerMemberRepository {}
        #[async_trait]
        impl rim_core::server_member::repository::ServerMemberRepository for ServerMemberRepository {
            async fn count_members(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<u64>;
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
            async fn get_members(&self, server_id: ServerId, user_ids: &[UserId]) -> RepoResult<Vec<ServerMember>>;
            async fn is_member(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn join_direct_space(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;
            async fn get_member_standing(&self, server_id: rim_core::server::model::ServerId, user_id: rim_core::user::model::UserId) -> RepoResult<Option<rim_core::server_member::model::MemberStanding>>;
            async fn get_owner_id(&self, server_id: rim_core::server::model::ServerId) -> RepoResult<Option<rim_core::user::model::UserId>>;
        }
    }

    // ── Helpers ──────────────────────────────────────────────────────────

    fn conversation(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: ConversationType::Channel,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            posting_policy: PostingPolicy::Everyone,
            posting_allows_threads: true,
            encrypted_at: None,
        }
    }

    fn thread(conv_id: ConversationId) -> MessageThread {
        let now = chrono::Utc::now();
        MessageThread {
            id: MessageThreadId::new(),
            conversation_id: conv_id,
            first_message_id: None,
            created_at: now,
            last_message_at: now,
        }
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn count_replies_returns_the_repository_counts() {
        let counted = MessageThreadId::new();
        let mut message_thread_repo = MockMessageThreadRepository::new();
        message_thread_repo
            .expect_count_live_replies()
            .withf(move |ids| ids == [counted])
            .return_once(move |_| Ok(std::collections::HashMap::from([(counted, 3)])));

        let svc = DefaultThreadService::new(ThreadRepos {
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            message_repo: Arc::new(MockMessageRepository::new()),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
        });

        let counts = svc.count_replies(&[counted]).await.unwrap();
        assert_eq!(counts, std::collections::HashMap::from([(counted, 3)]));
    }

    #[tokio::test]
    async fn get_thread_denies_non_member() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let conversation_repo = MockConversationRepository::new();
        let conversation_member_repo = MockConversationMemberRepository::new();
        let message_repo = MockMessageRepository::new();
        let message_thread_repo = MockMessageThreadRepository::new();
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));
        server_member_repo
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(None));

        let svc = DefaultThreadService::new(ThreadRepos {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                thread_id: MessageThreadId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn get_thread_by_root_message_returns_existing_thread() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);
        let conv_id = conv.id;
        let root_message_id = MessageId::new();
        let root_message = Message {
            id: root_message_id,
            conversation_id: conv_id,
            thread_id: None,
            sent_by: actor_id,
            text: Some("root".to_string()),
            has_attachments: false,
            encryption: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
            edited_at: None,
        };
        let existing_thread = thread(conv_id);

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_, _| Ok(true));

        let mut message_repo = MockMessageRepository::new();
        let root_for_repo = root_message.clone();
        message_repo
            .expect_get_by_id()
            .return_once(move |_| Ok(Some(root_for_repo.clone())));

        let mut message_thread_repo = MockMessageThreadRepository::new();
        let existing_for_repo = existing_thread.clone();
        message_thread_repo
            .expect_get_by_root_message()
            .return_once(move |_| Ok(Some(existing_for_repo.clone())));

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

        let svc = DefaultThreadService::new(ThreadRepos {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .get_thread_by_root_message(GetThreadByRootMessageQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv_id,
                root_message_id,
            })
            .await
            .unwrap()
            .unwrap();

        assert_eq!(dto.id, existing_thread.id);
    }

    #[tokio::test]
    async fn get_thread_not_found_when_wrong_conversation() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let conv = conversation(server_id);

        let mut conversation_repo = MockConversationRepository::new();
        let conv_for_repo = conv.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conv_for_repo.clone())));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(true));

        let message_repo = MockMessageRepository::new();

        let other_conv_id = ConversationId::new();
        let now = chrono::Utc::now();
        let mut message_thread_repo = MockMessageThreadRepository::new();
        message_thread_repo
            .expect_get_by_id()
            .returning(move |_id| {
                Ok(Some(MessageThread {
                    id: MessageThreadId::new(),
                    conversation_id: other_conv_id,
                    first_message_id: None,
                    created_at: now,
                    last_message_at: now,
                }))
            });

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

        let svc = DefaultThreadService::new(ThreadRepos {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_thread_repo: Arc::new(message_thread_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: conv.id,
                thread_id: MessageThreadId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));

        let _ = Message {
            // keep Message type referenced so imports stay stable under coverage
            id: MessageId::new(),
            conversation_id: conv.id,
            thread_id: Some(MessageThreadId::new()),
            sent_by: UserId::new(),
            text: None,
            has_attachments: false,
            encryption: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: now,
            edited_at: None,
        };
    }
}
