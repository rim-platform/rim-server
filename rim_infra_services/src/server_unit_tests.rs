#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::events::Observers;
    use rim_services::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
    use rim_services::server::service::ServerService;
    use rim_services::{Actor, ServiceError};

    use crate::ServerRepos;
    use crate::server::DefaultServerService;

    fn mock_conversation_repo() -> MockConversationRepository {
        conversation_repo_creating(None)
    }

    /// A conversation repository that creates channels, exactly `times` of them when set.
    fn conversation_repo_creating(times: Option<usize>) -> MockConversationRepository {
        let mut repo = MockConversationRepository::new();
        // New workspace members auto-join public channels; none exist in these fixtures.
        repo.expect_list_public_channel_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let creates = repo.expect_create_conversation_with_members();
        if let Some(times) = times {
            creates.times(times);
        }
        creates.returning(|conv, _member_ids| {
            Box::pin(async move {
                let now = chrono::Utc::now();
                Ok(Conversation {
                    id: ConversationId::new(),
                    conversation_type: conv.conversation_type,
                    title: conv.title,
                    topic: None,
                    created_at: now,
                    last_activity_at: now,
                    server_id: conv.server_id,
                    is_private: conv.is_private,
                    archived_at: None,
                    created_by: conv.created_by,
                    slow_mode_seconds: 0,
                    encrypted_at: None,
                })
            })
        });
        repo
    }

    fn noop_observers() -> Observers {
        use rim_services::events::NoopBroadcaster;
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
    }

    fn service(
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
    ) -> DefaultServerService {
        DefaultServerService::new(
            ServerRepos {
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                conversation_repo: Arc::new(mock_conversation_repo()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            },
            None,
            noop_observers(),
        )
    }

    #[tokio::test]
    async fn create_server_validates_name() {
        let actor_id = UserId::new();
        let (server_repo, server_member_repo) = (
            MockServerRepository::new(),
            MockServerMemberRepository::new(),
        );
        let svc = service(server_repo, server_member_repo);

        let err = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor_id),
                name: "   ".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_server_creates_owner_membership() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let now = chrono::Utc::now();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_find_owned_by_name()
            .withf(move |owner, name| *owner == actor_id && name == "My server")
            .returning(|_, _| Ok(None));
        server_repo.expect_create_server().returning(move |name| {
            Ok(Server {
                id: server_id,
                name: name.to_string(),
                created_at: now,
                updated_at: now,
            })
        });

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_add_member()
            .returning(move |sid, uid, role| {
                assert_eq!(sid, server_id);
                assert_eq!(uid, actor_id);
                assert_eq!(role, ServerMemberRole::Owner);
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: role,
                    joined_at: now,
                })
            });

        let svc = service(server_repo, server_member_repo);

        let dto = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor_id),
                name: "My server".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.id, server_id);
    }

    #[tokio::test]
    async fn create_server_refuses_a_name_the_actor_already_owns() {
        let actor_id = UserId::new();
        let now = chrono::Utc::now();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_find_owned_by_name()
            .withf(move |owner, name| *owner == actor_id && name == "design")
            .returning(move |_, _| {
                Ok(Some(Server {
                    id: ServerId::new(),
                    name: "Design".to_string(),
                    created_at: now,
                    updated_at: now,
                }))
            });
        server_repo.expect_create_server().never();

        let err = service(server_repo, MockServerMemberRepository::new())
            .create_server(CreateServerCmd {
                actor: Actor::new(actor_id),
                name: "  design ".to_string(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Conflict(msg) if msg == "you already own a workspace named \"Design\""),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn get_server_denies_non_member() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));
        server_member_repo
            .expect_join_direct_space()
            .returning(|_, _| Ok(false));

        let mut server_repo = MockServerRepository::new();
        server_repo.expect_get_by_id().never();

        let svc = service(server_repo, server_member_repo);

        let err = svc
            .get_server(GetServerQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_my_servers_returns_dtos() {
        let actor_id = UserId::new();
        let now = chrono::Utc::now();
        let s1 = Server {
            id: ServerId::new(),
            name: "a".to_string(),
            created_at: now,
            updated_at: now,
        };
        let s2 = Server {
            id: ServerId::new(),
            name: "b".to_string(),
            created_at: now,
            updated_at: now,
        };

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_uid| Ok(vec![s1.clone(), s2.clone()]));

        let server_member_repo = MockServerMemberRepository::new();

        let svc = service(server_repo, server_member_repo);

        let list = svc
            .list_my_servers(ListMyServersQuery {
                actor: Actor::new(actor_id),
            })
            .await
            .unwrap();
        assert_eq!(list.len(), 2);
    }

    // ── Instance limits ──

    /// A server repository that lets `actor_id` create a workspace while owning `owned`;
    /// `None` expects the owned workspaces never to be counted.
    fn creatable_server_repo(actor_id: UserId, owned: Option<u64>) -> MockServerRepository {
        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_find_owned_by_name()
            .returning(|_, _| Ok(None));
        match owned {
            Some(owned) => {
                server_repo
                    .expect_count_owned()
                    .withf(move |owner| *owner == actor_id)
                    .returning(move |_| Ok(owned));
            }
            None => {
                server_repo.expect_count_owned().never();
            }
        }
        server_repo.expect_create_server().returning(|name| {
            let now = chrono::Utc::now();
            Ok(Server {
                id: ServerId::new(),
                name: name.to_string(),
                created_at: now,
                updated_at: now,
            })
        });
        server_repo
    }

    /// A member repository that accepts the owner's membership.
    fn accepting_member_repo() -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        repo.expect_add_member().returning(|sid, uid, role| {
            Ok(ServerMember {
                id: ServerMemberId::new(),
                server_id: sid,
                user_id: uid,
                member_role: role,
                joined_at: chrono::Utc::now(),
            })
        });
        repo
    }

    fn limited_service(
        server_repo: MockServerRepository,
        conversation_repo: MockConversationRepository,
        limits: rim_services::limits::InstanceLimits,
    ) -> DefaultServerService {
        DefaultServerService::new(
            ServerRepos {
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(accepting_member_repo()),
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            },
            None,
            noop_observers(),
        )
        .with_limits(limits)
    }

    async fn create(svc: &DefaultServerService, actor_id: UserId) -> Result<(), ServiceError> {
        svc.create_server(CreateServerCmd {
            actor: Actor::new(actor_id),
            name: "Limited".to_string(),
        })
        .await
        .map(|_| ())
    }

    #[tokio::test]
    async fn creating_a_workspace_follows_the_workspaces_per_user_cap() {
        for (cap, owned, allowed) in [
            (Some(5), Some(4), true),
            (Some(5), Some(5), false),
            (None, None, true),
        ] {
            let actor_id = UserId::new();
            let svc = limited_service(
                creatable_server_repo(actor_id, owned),
                conversation_repo_creating(None),
                rim_services::limits::InstanceLimits {
                    workspaces_per_user: cap,
                    ..Default::default()
                },
            );
            match create(&svc, actor_id).await {
                Ok(()) => assert!(allowed, "{cap:?} {owned:?}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {owned:?}");
                    assert_eq!(message, "this instance allows 5 workspaces per user");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn a_new_workspace_gets_only_the_default_channels_its_cap_allows() {
        for (cap, channels) in [(Some(1), 1), (Some(2), 2), (None, 2)] {
            let actor_id = UserId::new();
            let svc = limited_service(
                creatable_server_repo(actor_id, None),
                conversation_repo_creating(Some(channels)),
                rim_services::limits::InstanceLimits {
                    channels_per_workspace: cap,
                    ..Default::default()
                },
            );
            create(&svc, actor_id).await.expect("created");
        }
    }
}
