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
    use rim_core::message_reaction::model::{
        MessageReaction, MessageReactionId, NewMessageReaction,
    };
    use rim_core::repository::error::RepoResult;
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::UserId;
    use rim_services::message_reaction::command::{GetMessageReactionsQuery, ToggleReactionCmd};
    use rim_services::message_reaction::service::MessageReactionService;
    use rim_services::{Actor, ServiceError};

    use rim_services::events::{NoopBroadcaster, Observers};

    use crate::message_reaction::DefaultMessageReactionService;
    use crate::repos::ReactionRepos;

    // ── Inline mock definitions ──────────────────────────────────────────

    mock! {
        MessageReactionRepository {}
        #[async_trait]
        impl rim_core::message_reaction::repository::MessageReactionRepository for MessageReactionRepository {
            async fn add_reaction(&self, reaction: NewMessageReaction) -> RepoResult<MessageReaction>;
            async fn remove_reaction(
                &self,
                message_id: MessageId,
                user_id: UserId,
                emoji: &str,
            ) -> RepoResult<()>;
            async fn find_reaction(
                &self,
                message_id: MessageId,
                user_id: UserId,
                emoji: &str,
            ) -> RepoResult<Option<MessageReaction>>;
            async fn get_reactions_for_message(
                &self,
                message_id: MessageId,
            ) -> RepoResult<Vec<MessageReaction>>;
            async fn get_reactions_for_messages(
                &self,
                message_ids: &[MessageId],
            ) -> RepoResult<std::collections::HashMap<MessageId, Vec<MessageReaction>>>;
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
            async fn exists_channel_by_title(
                &self,
                server_id: ServerId,
                title: &str,
            ) -> RepoResult<bool>;
            async fn list_accessible_conversation_ids(
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
            async fn search_messages_across_conversations(
                &self,
                conversation_ids: &[ConversationId],
                query: &str,
                limit: u32,
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

    fn test_observers() -> Observers {
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
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
        }
    }

    fn message(id: MessageId, conversation_id: ConversationId, sent_by: UserId) -> Message {
        Message {
            id,
            conversation_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_by,
            text: Some("hello".to_string()),
            has_attachments: false,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    fn reaction(message_id: MessageId, user_id: UserId, emoji: &str) -> MessageReaction {
        MessageReaction {
            id: MessageReactionId::new(),
            message_id,
            user_id,
            emoji: emoji.to_string(),
            created_at: chrono::Utc::now(),
        }
    }

    fn make_repos(
        server_member_repo: MockServerMemberRepository,
        conversation_repo: MockConversationRepository,
        conversation_member_repo: MockConversationMemberRepository,
        message_repo: MockMessageRepository,
        message_reaction_repo: MockMessageReactionRepository,
    ) -> ReactionRepos {
        ReactionRepos {
            server_member_repo: Arc::new(server_member_repo),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            message_repo: Arc::new(message_repo),
            message_reaction_repo: Arc::new(message_reaction_repo),
        }
    }

    fn setup_happy_path_mocks(
        server_id: ServerId,
        conversation_id: ConversationId,
        message_id: MessageId,
        actor_id: UserId,
    ) -> (
        MockServerMemberRepository,
        MockConversationRepository,
        MockConversationMemberRepository,
        MockMessageRepository,
    ) {
        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_cid, uid| Ok(uid == actor_id));
        conversation_member_repo
            .expect_list_member_ids()
            .returning(move |_cid| Ok(vec![actor_id]));

        let mut message_repo = MockMessageRepository::new();
        let msg = message(message_id, conversation_id, UserId::new());
        message_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(msg.clone())));

        (
            server_member_repo,
            conversation_repo,
            conversation_member_repo,
            message_repo,
        )
    }

    // ── Tests ────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn toggle_adds_reaction_when_not_existing() {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let actor_id = UserId::new();

        let (smr, cr, cmr, mr) =
            setup_happy_path_mocks(server_id, conversation_id, message_id, actor_id);

        let mut reaction_repo = MockMessageReactionRepository::new();
        reaction_repo
            .expect_find_reaction()
            .returning(|_mid, _uid, _emoji| Ok(None));
        reaction_repo
            .expect_add_reaction()
            .returning(move |new| Ok(reaction(new.message_id, new.user_id, &new.emoji)));
        let added_reaction = reaction(message_id, actor_id, "\u{1f44d}");
        reaction_repo
            .expect_get_reactions_for_message()
            .returning(move |_mid| Ok(vec![added_reaction.clone()]));

        let svc = DefaultMessageReactionService::new(
            make_repos(smr, cr, cmr, mr, reaction_repo),
            None,
            test_observers(),
        );

        let groups = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id,
                emoji: "\u{1f44d}".to_string(),
            })
            .await
            .unwrap();

        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].emoji, "\u{1f44d}");
        assert_eq!(groups[0].count, 1);
        assert!(groups[0].reacted_by_me);
    }

    #[tokio::test]
    async fn toggle_removes_existing_reaction() {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let actor_id = UserId::new();

        let (smr, cr, cmr, mr) =
            setup_happy_path_mocks(server_id, conversation_id, message_id, actor_id);

        let existing = reaction(message_id, actor_id, "\u{1f44d}");
        let mut reaction_repo = MockMessageReactionRepository::new();
        reaction_repo
            .expect_find_reaction()
            .returning(move |_mid, _uid, _emoji| Ok(Some(existing.clone())));
        reaction_repo
            .expect_remove_reaction()
            .returning(|_mid, _uid, _emoji| Ok(()));
        reaction_repo
            .expect_get_reactions_for_message()
            .returning(|_mid| Ok(vec![]));

        let svc = DefaultMessageReactionService::new(
            make_repos(smr, cr, cmr, mr, reaction_repo),
            None,
            test_observers(),
        );

        let groups = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id,
                emoji: "\u{1f44d}".to_string(),
            })
            .await
            .unwrap();

        assert!(groups.is_empty());
    }

    #[tokio::test]
    async fn toggle_denies_non_server_member() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let svc = DefaultMessageReactionService::new(
            make_repos(
                server_member_repo,
                MockConversationRepository::new(),
                MockConversationMemberRepository::new(),
                MockMessageRepository::new(),
                MockMessageReactionRepository::new(),
            ),
            None,
            test_observers(),
        );

        let err = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                message_id: MessageId::new(),
                emoji: "\u{1f44d}".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn toggle_denies_non_conversation_member() {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(false));

        let svc = DefaultMessageReactionService::new(
            make_repos(
                server_member_repo,
                conversation_repo,
                conversation_member_repo,
                MockMessageRepository::new(),
                MockMessageReactionRepository::new(),
            ),
            None,
            test_observers(),
        );

        let err = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id: MessageId::new(),
                emoji: "\u{1f44d}".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn toggle_returns_not_found_for_missing_message() {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let actor_id = UserId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(conversation(server_id, conversation_id))));

        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_is_member()
            .returning(move |_cid, uid| Ok(uid == actor_id));

        let mut message_repo = MockMessageRepository::new();
        message_repo.expect_get_by_id().returning(|_id| Ok(None));

        let svc = DefaultMessageReactionService::new(
            make_repos(
                server_member_repo,
                conversation_repo,
                conversation_member_repo,
                message_repo,
                MockMessageReactionRepository::new(),
            ),
            None,
            test_observers(),
        );

        let err = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id: MessageId::new(),
                emoji: "\u{1f44d}".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn toggle_rejects_empty_emoji() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let svc = DefaultMessageReactionService::new(
            make_repos(
                MockServerMemberRepository::new(),
                MockConversationRepository::new(),
                MockConversationMemberRepository::new(),
                MockMessageRepository::new(),
                MockMessageReactionRepository::new(),
            ),
            None,
            test_observers(),
        );

        let err = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: ConversationId::new(),
                message_id: MessageId::new(),
                emoji: "   ".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn get_reactions_returns_grouped_results() {
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let actor_id = UserId::new();
        let other_user = UserId::new();

        let (smr, cr, cmr, mr) =
            setup_happy_path_mocks(server_id, conversation_id, message_id, actor_id);

        let mut reaction_repo = MockMessageReactionRepository::new();
        let r1 = reaction(message_id, actor_id, "\u{1f44d}");
        let r2 = reaction(message_id, other_user, "\u{1f44d}");
        let r3 = reaction(message_id, other_user, "\u{2764}");
        reaction_repo
            .expect_get_reactions_for_message()
            .returning(move |_mid| Ok(vec![r1.clone(), r2.clone(), r3.clone()]));

        let svc = DefaultMessageReactionService::new(
            make_repos(smr, cr, cmr, mr, reaction_repo),
            None,
            test_observers(),
        );

        let groups = svc
            .get_reactions(GetMessageReactionsQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id,
                message_id,
            })
            .await
            .unwrap();

        assert_eq!(groups.len(), 2);

        let thumbs_up = groups.iter().find(|g| g.emoji == "\u{1f44d}").unwrap();
        assert_eq!(thumbs_up.count, 2);
        assert!(thumbs_up.reacted_by_me);

        let heart = groups.iter().find(|g| g.emoji == "\u{2764}").unwrap();
        assert_eq!(heart.count, 1);
        assert!(!heart.reacted_by_me);
    }
}
