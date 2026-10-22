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
    use rim_core::user::repository::MockUserRepository;
    use rim_services::events::Observers;
    use rim_services::server::command::{
        CreateServerCmd, DeleteWorkspaceCmd, GetServerQuery, ListMyServersQuery,
        TransferWorkspaceOwnershipCmd,
    };
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
        creates.returning(|conv, _member_ids, _pending_ids| {
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
                user_repo: Arc::new(MockUserRepository::new()),
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
        limited_service_for(server_repo, conversation_repo, lockable_users(), limits)
    }

    /// A user repository whose every row can be locked.
    fn lockable_users() -> MockUserRepository {
        let mut repo = MockUserRepository::new();
        repo.expect_lock_user().returning(|_| Ok(true));
        repo
    }

    fn limited_service_for(
        server_repo: MockServerRepository,
        conversation_repo: MockConversationRepository,
        user_repo: MockUserRepository,
        limits: rim_services::limits::InstanceLimits,
    ) -> DefaultServerService {
        DefaultServerService::new(
            ServerRepos {
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(accepting_member_repo()),
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                user_repo: Arc::new(user_repo),
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

    #[tokio::test]
    async fn an_account_in_probation_cannot_create_a_workspace_until_it_ends() {
        let created = chrono::Utc::now();
        let probation = rim_services::limits::InstanceLimits {
            probation_hours: Some(24),
            ..Default::default()
        };
        for (now, refusal) in [
            (
                created + chrono::Duration::minutes(30),
                Some("a new account can create a workspace in 1410 minutes"),
            ),
            (created + chrono::Duration::hours(24), None),
        ] {
            let actor_id = UserId::new();
            let svc = limited_service_for(
                creatable_server_repo(actor_id, None),
                conversation_repo_creating(None),
                crate::probation_fixtures::users(vec![crate::probation_fixtures::account(
                    actor_id, created, None,
                )]),
                probation,
            )
            .with_clock(crate::clock::Clock::fixed(now));
            match (create(&svc, actor_id).await, refusal) {
                (Ok(()), None) => {}
                (Err(ServiceError::Validation(message)), Some(expected)) => {
                    assert_eq!(message, expected)
                }
                (other, expected) => panic!("expected {expected:?}, got {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn an_instance_operator_creates_a_workspace_however_new_the_account() {
        let created = chrono::Utc::now();
        let actor_id = UserId::new();
        let svc = limited_service_for(
            creatable_server_repo(actor_id, None),
            conversation_repo_creating(None),
            crate::probation_fixtures::users_and_operators(
                vec![crate::probation_fixtures::account(actor_id, created, None)],
                vec![actor_id],
            ),
            rim_services::limits::InstanceLimits {
                probation_hours: Some(24),
                ..Default::default()
            },
        )
        .with_clock(crate::clock::Clock::fixed(created));
        create(&svc, actor_id).await.expect("created");
    }

    /// Every event broadcast through it, in order.
    #[derive(Default)]
    struct Recorder(std::sync::Mutex<Vec<rim_services::events::WorkspaceEvent>>);

    impl rim_services::events::Broadcaster for Recorder {
        fn broadcast(&self, event: rim_services::events::WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    fn member(server_id: ServerId, user_id: UserId, role: ServerMemberRole) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: role,
            joined_at: chrono::Utc::now(),
        }
    }

    /// Members answering `get_member` from `roles`; anyone else is no member.
    fn members(roles: Vec<(UserId, ServerMemberRole)>) -> MockServerMemberRepository {
        let mut repo = MockServerMemberRepository::new();
        let all = roles.clone();
        repo.expect_get_member().returning(move |sid, uid| {
            Ok(roles
                .iter()
                .find(|(id, _)| *id == uid)
                .map(|(id, role)| member(sid, *id, *role)))
        });
        repo.expect_get_members_for_server().returning(move |sid| {
            Ok(all
                .iter()
                .map(|(id, role)| member(sid, *id, *role))
                .collect())
        });
        repo
    }

    fn server_named(id: ServerId, name: &str) -> Server {
        let now = chrono::Utc::now();
        Server {
            id,
            name: name.to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    fn owner_service(
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        storage: rim_services::storage::service::MockStorageService,
        events: Arc<Recorder>,
        direct_space: Option<ServerId>,
    ) -> DefaultServerService {
        DefaultServerService::new(
            ServerRepos {
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                conversation_repo: Arc::new(mock_conversation_repo()),
                conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                user_repo: Arc::new(MockUserRepository::new()),
            },
            None,
            Observers::new(events),
        )
        .with_storage(Arc::new(storage))
        .with_direct_space(direct_space)
    }

    fn transfer(server_id: ServerId, actor: UserId, to: UserId) -> TransferWorkspaceOwnershipCmd {
        TransferWorkspaceOwnershipCmd {
            actor: Actor::new(actor),
            server_id,
            user_id: to,
        }
    }

    #[tokio::test]
    async fn the_owner_hands_ownership_to_a_member_and_becomes_a_moderator() {
        let (server_id, owner, bob) = (ServerId::new(), UserId::new(), UserId::new());
        let mut repo = members(vec![
            (owner, ServerMemberRole::Owner),
            (bob, ServerMemberRole::Member),
        ]);
        let changes = Arc::new(std::sync::Mutex::new(Vec::new()));
        let c = Arc::clone(&changes);
        repo.expect_change_member_role()
            .times(2)
            .returning(move |_, uid, role| {
                c.lock().expect("lock").push((uid, role));
                Ok(())
            });
        let mut servers = MockServerRepository::new();
        servers
            .expect_get_by_id()
            .returning(|id| Ok(Some(server_named(id, "Team"))));
        let events = Arc::new(Recorder::default());
        let dto = owner_service(
            servers,
            repo,
            rim_services::storage::service::MockStorageService::new(),
            Arc::clone(&events),
            None,
        )
        .transfer_ownership(transfer(server_id, owner, bob))
        .await
        .expect("transferred");

        assert_eq!(dto.id, server_id);
        assert_eq!(
            *changes.lock().expect("lock"),
            vec![
                (bob, ServerMemberRole::Owner),
                (owner, ServerMemberRole::Moderator),
            ]
        );
        assert_eq!(events.0.lock().expect("lock").len(), 2);
    }

    #[tokio::test]
    async fn ownership_moves_only_from_the_owner_to_a_member_outside_the_direct_space() {
        let (server_id, owner, mona, gina, stranger) = (
            ServerId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
            UserId::new(),
        );
        let roles = vec![
            (owner, ServerMemberRole::Owner),
            (mona, ServerMemberRole::Moderator),
            (gina, ServerMemberRole::Guest),
        ];
        let cases: Vec<(UserId, UserId, Option<ServerId>, &str)> = vec![
            (mona, owner, None, "PermissionDenied"),
            (owner, gina, None, "Validation"),
            (owner, owner, None, "Validation"),
            (owner, stranger, None, "NotFound"),
            (owner, mona, Some(server_id), "Validation"),
        ];
        for (actor, to, direct, want) in cases {
            let mut repo = members(roles.clone());
            repo.expect_change_member_role().never();
            let err = owner_service(
                MockServerRepository::new(),
                repo,
                rim_services::storage::service::MockStorageService::new(),
                Arc::new(Recorder::default()),
                direct,
            )
            .transfer_ownership(transfer(server_id, actor, to))
            .await
            .expect_err("refused");
            let got = match err {
                ServiceError::PermissionDenied => "PermissionDenied",
                ServiceError::Validation(_) => "Validation",
                ServiceError::NotFound => "NotFound",
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(got, want, "{actor} -> {to}");
        }
    }

    #[tokio::test]
    async fn ownership_moves_only_to_a_member_below_the_workspaces_per_user_cap() {
        for (owned, allowed) in [(4, true), (5, false)] {
            let (server_id, owner, bob) = (ServerId::new(), UserId::new(), UserId::new());
            let mut repo = members(vec![
                (owner, ServerMemberRole::Owner),
                (bob, ServerMemberRole::Member),
            ]);
            repo.expect_change_member_role()
                .times(if allowed { 2 } else { 0 })
                .returning(|_, _, _| Ok(()));
            let mut servers = MockServerRepository::new();
            servers
                .expect_count_owned()
                .withf(move |user| *user == bob)
                .returning(move |_| Ok(owned));
            servers
                .expect_get_by_id()
                .returning(|id| Ok(Some(server_named(id, "Team"))));
            let mut users = MockUserRepository::new();
            users
                .expect_lock_user()
                .withf(move |user| *user == bob)
                .times(1)
                .returning(|_| Ok(true));
            let svc = DefaultServerService::new(
                ServerRepos {
                    server_repo: Arc::new(servers),
                    server_member_repo: Arc::new(repo),
                    conversation_repo: Arc::new(mock_conversation_repo()),
                    conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
                    user_repo: Arc::new(users),
                },
                None,
                noop_observers(),
            )
            .with_limits(rim_services::limits::InstanceLimits {
                workspaces_per_user: Some(5),
                ..Default::default()
            });
            match svc
                .transfer_ownership(transfer(server_id, owner, bob))
                .await
            {
                Ok(_) => assert!(allowed, "{owned}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{owned}");
                    assert_eq!(
                        message,
                        "this instance allows 5 workspaces per user, and the new owner already \
                         owns 5"
                    );
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    fn delete(server_id: ServerId, actor: UserId, name: &str) -> DeleteWorkspaceCmd {
        DeleteWorkspaceCmd {
            actor: Actor::new(actor),
            server_id,
            confirm_name: name.to_string(),
        }
    }

    #[tokio::test]
    async fn the_owner_deletes_the_workspace_by_name_its_files_go_and_every_member_is_told() {
        let (server_id, owner, bob) = (ServerId::new(), UserId::new(), UserId::new());
        let repo = members(vec![
            (owner, ServerMemberRole::Owner),
            (bob, ServerMemberRole::Member),
        ]);
        let mut servers = MockServerRepository::new();
        servers
            .expect_get_by_id()
            .returning(|id| Ok(Some(server_named(id, "Team"))));
        servers
            .expect_delete_workspaces()
            .withf(move |ids| ids == [server_id])
            .times(1)
            .returning(|_| {
                Ok(rim_core::server::model::DeletedWorkspaces {
                    deleted: 1,
                    storage_keys: vec!["uploads/a".to_string()],
                })
            });
        let mut storage = rim_services::storage::service::MockStorageService::new();
        storage
            .expect_delete_file()
            .withf(|key| key == "uploads/a")
            .times(1)
            .returning(|_| Box::pin(async { Ok(()) }));
        let events = Arc::new(Recorder::default());
        let deleted = owner_service(servers, repo, storage, Arc::clone(&events), None)
            .delete_workspace(delete(server_id, owner, " Team "))
            .await
            .expect("deleted");
        assert!(deleted);
        let left: Vec<UserId> = events
            .0
            .lock()
            .expect("lock")
            .iter()
            .filter_map(|e| match e {
                rim_services::events::WorkspaceEvent::MemberLeft { user_id, .. } => Some(*user_id),
                _ => None,
            })
            .collect();
        assert_eq!(left, vec![owner, bob]);
    }

    #[tokio::test]
    async fn a_workspace_is_deleted_only_by_its_owner_with_its_name_outside_the_direct_space() {
        let (server_id, owner, mona) = (ServerId::new(), UserId::new(), UserId::new());
        let cases: Vec<(UserId, &str, Option<ServerId>, &str)> = vec![
            (owner, "team", None, "Validation"),
            (mona, "Team", None, "PermissionDenied"),
            (owner, "Team", Some(server_id), "Validation"),
        ];
        for (actor, name, direct, want) in cases {
            let repo = members(vec![
                (owner, ServerMemberRole::Owner),
                (mona, ServerMemberRole::Moderator),
            ]);
            let mut servers = MockServerRepository::new();
            servers
                .expect_get_by_id()
                .returning(|id| Ok(Some(server_named(id, "Team"))));
            servers.expect_delete_workspaces().never();
            let err = owner_service(
                servers,
                repo,
                rim_services::storage::service::MockStorageService::new(),
                Arc::new(Recorder::default()),
                direct,
            )
            .delete_workspace(delete(server_id, actor, name))
            .await
            .expect_err("refused");
            let got = match err {
                ServiceError::PermissionDenied => "PermissionDenied",
                ServiceError::Validation(_) => "Validation",
                other => panic!("unexpected {other:?}"),
            };
            assert_eq!(got, want, "{name}");
        }
    }
}
