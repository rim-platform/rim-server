#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::{
        ConversationActivityCursor, ConversationCreatedCursor,
    };
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;
    use rim_services::conversation::command::{
        AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery,
    };
    use rim_services::conversation::service::ConversationService;
    use rim_services::conversation::types::ConversationParticipants;
    use rim_services::{Actor, ServiceError};

    use crate::conversation::DefaultConversationService;
    use crate::test_support::conversation::{
        ConversationReposPorts, MockConversationMemberRepo, MockConversationRepo,
        MockServerMemberRepo,
    };

    fn base_repos() -> (
        MockConversationRepo,
        MockConversationMemberRepo,
        MockServerMemberRepo,
    ) {
        (
            MockConversationRepo::new(),
            MockConversationMemberRepo::new(),
            MockServerMemberRepo::new(),
        )
    }

    fn conv(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: ConversationType::Direct,
            title: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_dm_denies_self_dm() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let (conversation_repo, conversation_member_repo, mut server_member_repo) = base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: actor_id,
                },
                title: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_dm_rejects_title() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let (conversation_repo, conversation_member_repo, mut server_member_repo) = base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: UserId::new(),
                },
                title: Some("name".to_string()),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_channel_persists_trimmed_title() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let mut created = conv(server_id);
        created.conversation_type = ConversationType::Channel;
        created.title = Some("Launches".to_string());

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let created_for_repo = created.clone();
        conversation_repo
            .expect_create_conversation_with_members()
            .return_once(move |kind, sid, title, members| {
                assert_eq!(kind, ConversationType::Channel);
                assert_eq!(sid, server_id);
                assert_eq!(title.as_deref(), Some("Launches"));
                assert_eq!(members, &[actor_id]);
                Ok(created_for_repo)
            });

        conversation_member_repo
            .expect_list_member_ids()
            .returning(move |_conversation_id| Ok(vec![actor_id]));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::Channel,
                title: Some("  Launches  ".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(dto.title.as_deref(), Some("Launches"));
    }

    #[tokio::test]
    async fn create_dm_returns_existing_conversation() {
        let actor_id = UserId::new();
        let other_id = UserId::new();
        let server_id = ServerId::new();
        let existing = conv(server_id);
        let existing_id = existing.id;

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        conversation_repo
            .expect_find_direct_between_users()
            .returning(move |_sid, _a, _b| Ok(Some(existing.clone())));
        conversation_repo.expect_create_conversation().never();
        conversation_repo
            .expect_create_conversation_with_members()
            .never();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_conversation_id| Ok(vec![]));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: other_id,
                },
                title: None,
            })
            .await
            .unwrap();
        assert_eq!(dto.id, existing_id);
    }

    #[tokio::test]
    async fn get_conversation_denies_if_not_conversation_member() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let c = conv(server_id);
        let c_id = c.id;

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let c_for_repo = c.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(c_for_repo.clone())));

        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Ok(false));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_conversation(GetConversationQuery {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: c_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_methods_batch_member_ids() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let c1 = conv(server_id);
        let c2 = conv(server_id);

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let c1_inbox = c1.clone();
        let c2_inbox = c2.clone();
        conversation_repo
            .expect_list_inbox_conversations_for_user()
            .return_once(move |_, _, _, _, _| Ok(Page::new(vec![c1_inbox, c2_inbox], None, None)));

        let c1_id = c1.id;
        let c2_id = c2.id;
        conversation_member_repo
            .expect_list_member_ids_for_conversations()
            .times(2)
            .returning(move |ids| {
                let mut out = std::collections::HashMap::new();
                for id in ids {
                    out.insert(*id, vec![actor_id]);
                }
                Ok(out)
            });

        let c1_channels = c1.clone();
        let c2_channels = c2.clone();
        conversation_repo
            .expect_list_channels_for_user()
            .return_once(move |_, _, _, _, _| {
                Ok(Page::new(vec![c1_channels, c2_channels], None, None))
            });

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let inbox = svc
            .list_inbox_conversations(
                rim_services::conversation::command::ListInboxConversationsQuery {
                    actor: Actor::new(actor_id),
                    server_id,
                    dir: PageDir::After,
                    cursor: None,
                    limit: 10,
                },
            )
            .await
            .unwrap();
        assert_eq!(inbox.items.len(), 2);
        assert_eq!(inbox.items[0].member_ids, vec![actor_id]);

        let channels = svc
            .list_channels(rim_services::conversation::command::ListChannelsQuery {
                actor: Actor::new(actor_id),
                server_id,
                dir: PageDir::After,
                cursor: None,
                limit: 10,
            })
            .await
            .unwrap();
        assert_eq!(channels.items.len(), 2);
        assert_eq!(channels.items[1].member_ids, vec![actor_id]);

        assert_ne!(c1_id, c2_id);
    }

    #[tokio::test]
    async fn add_member_only_allowed_for_group() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let mut c = conv(server_id);
        c.conversation_type = ConversationType::Direct;
        let c_id = c.id;

        let (mut conversation_repo, conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        let c_for_repo = c.clone();
        conversation_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(c_for_repo.clone())));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .add_member(AddConversationMemberCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: c_id,
                user_id: UserId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn list_methods_map_page_and_call_to_dto_per_item() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let c1 = conv(server_id);
        let c2 = conv(server_id);
        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();

        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));

        conversation_repo
            .expect_list_inbox_conversations_for_user()
            .returning(move |_uid, _sid, _limit, _dir, _cursor| {
                Ok(Page::new(vec![c1.clone(), c2.clone()], None, None))
            });

        conversation_member_repo
            .expect_list_member_ids_for_conversations()
            .return_once(move |ids| {
                let mut out = std::collections::HashMap::new();
                for id in ids {
                    out.insert(*id, vec![]);
                }
                Ok(out)
            });

        // Also satisfy channels path even if not called.
        conversation_repo
            .expect_list_channels_for_user()
            .returning(|_uid, _sid, _limit, _dir, _cursor| Ok(Page::new(vec![], None, None)));

        let svc = DefaultConversationService::new_for_tests(ConversationReposPorts {
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let page = svc
            .list_inbox_conversations(
                rim_services::conversation::command::ListInboxConversationsQuery {
                    actor: Actor::new(actor_id),
                    server_id,
                    limit: 10,
                    dir: PageDir::After,
                    cursor: None,
                },
            )
            .await
            .unwrap();
        assert_eq!(page.items.len(), 2);

        let _ = (
            ConversationActivityCursor {
                last_activity_at: chrono::Utc::now(),
                conversation_id: ConversationId::new(),
            },
            ConversationCreatedCursor {
                created_at: chrono::Utc::now(),
                conversation_id: ConversationId::new(),
            },
        );
    }
}
