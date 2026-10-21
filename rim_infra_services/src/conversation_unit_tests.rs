#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::{
        ChannelListFilter, ConversationActivityCursor, ConversationCreatedCursor,
        MockConversationRepository,
    };
    use rim_core::conversation_member::model::{
        ConversationMember, ConversationMemberId, Membership,
    };
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::e2ee::repository::MockE2eeRepository;
    use rim_core::notification::repository::MockNotificationRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{
        MemberStanding, ServerMember, ServerMemberId, ServerMemberRole,
    };
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::conversation::command::{
        AddConversationMembersCmd, ArchiveConversationCmd, ConversationInviteCmd,
        CreateConversationCmd, GetConversationQuery, JoinChannelCmd, LeaveConversationCmd,
        ListChannelsQuery, RemoveConversationMemberCmd, UpdateConversationTopicCmd,
    };
    use rim_services::conversation::service::ConversationService;
    use rim_services::conversation::types::ConversationParticipants;
    use rim_services::{Actor, ServiceError};

    use rim_services::events::{Broadcaster, NoopBroadcaster, Observers, WorkspaceEvent};

    use crate::ConversationRepos;
    use crate::conversation::DefaultConversationService;

    /// A default workspace service that answers `current`.
    fn default_workspace_service(
        current: Option<ServerId>,
    ) -> rim_services::default_workspace::MockDefaultWorkspaceService {
        let mut service = rim_services::default_workspace::MockDefaultWorkspaceService::new();
        service
            .expect_current()
            .returning(move || Box::pin(async move { Ok(current) }));
        service
    }

    fn base_repos() -> (
        MockConversationRepository,
        MockConversationMemberRepository,
        MockServerMemberRepository,
    ) {
        (
            MockConversationRepository::new(),
            MockConversationMemberRepository::new(),
            MockServerMemberRepository::new(),
        )
    }

    fn test_observers() -> Observers {
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
    }

    fn conv(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            server_id,
            conversation_type: ConversationType::Direct,
            title: None,
            topic: None,
            created_at: chrono::Utc::now(),
            last_activity_at: chrono::Utc::now(),
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            encrypted_at: None,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

        let err = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: actor_id,
                },
                title: None,
                is_private: false,
                encrypted: false,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

        let err = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: UserId::new(),
                },
                title: Some("name".to_string()),
                is_private: false,
                encrypted: false,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        conversation_repo
            .expect_exists_channel_by_title()
            .returning(|_sid, _title| Box::pin(async { Ok(false) }));

        let created_for_repo = created.clone();
        conversation_repo
            .expect_create_conversation_with_members()
            .return_once(move |new, members, _pending| {
                assert_eq!(new.conversation_type, ConversationType::Channel);
                assert_eq!(new.server_id, server_id);
                assert_eq!(new.title.as_deref(), Some("Launches"));
                assert!(!new.is_private);
                assert_eq!(new.created_by, Some(actor_id));
                assert_eq!(members, &[actor_id]);
                Box::pin(async move { Ok(created_for_repo) })
            });

        conversation_member_repo
            .expect_list_member_ids()
            .returning(move |_conversation_id| Box::pin(async move { Ok(vec![actor_id]) }));

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

        let dto = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::Channel,
                title: Some("  Launches  ".to_string()),
                is_private: false,
                encrypted: false,
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        conversation_repo
            .expect_find_direct_between_users()
            .returning(move |_sid, _a, _b| {
                let e = existing.clone();
                Box::pin(async move { Ok(Some(e)) })
            });
        conversation_repo.expect_create_conversation().never();
        conversation_repo
            .expect_create_conversation_with_members()
            .never();
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_conversation_id| Box::pin(async { Ok(vec![]) }));

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

        let dto = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor_id),
                server_id,
                participants: ConversationParticipants::DirectMessage {
                    other_user_id: other_id,
                },
                title: None,
                is_private: false,
                encrypted: false,
            })
            .await
            .unwrap();
        assert_eq!(dto.id, existing_id);
    }

    #[tokio::test]
    async fn a_user_starts_a_bounded_number_of_direct_space_conversations_a_day() {
        use crate::conversation::MAX_DIRECT_SPACE_CONVERSATIONS_PER_DAY;

        let actor_id = UserId::new();
        let known = UserId::new();
        let direct = ServerId::new();
        let existing = conv(direct);
        let existing_id = existing.id;

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(true));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Guest))));
        conversation_repo
            .expect_find_direct_between_users()
            .returning(move |_sid, _a, b| {
                let found = (b == known).then(|| existing.clone());
                Box::pin(async move { Ok(found) })
            });
        conversation_repo
            .expect_create_conversation_with_members()
            .returning(move |new, _members, _pending| {
                let mut created = conv(direct);
                created.conversation_type = new.conversation_type;
                Box::pin(async move { Ok(created) })
            });
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_conversation_id| Box::pin(async { Ok(vec![]) }));

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        )
        .with_direct_space(Some(direct));
        let create = |participants| CreateConversationCmd {
            actor: Actor::new(actor_id),
            server_id: direct,
            participants,
            title: None,
            is_private: false,
            encrypted: false,
        };
        let dm = |other_user_id| ConversationParticipants::DirectMessage { other_user_id };

        for n in 0..MAX_DIRECT_SPACE_CONVERSATIONS_PER_DAY {
            let participants = if n % 2 == 0 {
                dm(UserId::new())
            } else {
                ConversationParticipants::Group {
                    member_ids: vec![UserId::new(), UserId::new()],
                }
            };
            svc.create_conversation(create(participants))
                .await
                .expect("within the day's conversations");
        }
        let err = svc
            .create_conversation(create(dm(UserId::new())))
            .await
            .unwrap_err();
        assert!(
            matches!(&err, ServiceError::Validation(m) if m.contains("at most 50 new conversations a day")),
            "{err:?}"
        );
        let reopened = svc
            .create_conversation(create(dm(known)))
            .await
            .expect("an existing DM opens past the limit");
        assert_eq!(reopened.id, existing_id);
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let c_for_repo = c.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = c_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(false) }));

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        let c1_inbox = c1.clone();
        let c2_inbox = c2.clone();
        conversation_repo
            .expect_list_inbox_conversations_for_user()
            .return_once(move |_, _, _, _, _| {
                Box::pin(async move { Ok(Page::new(vec![c1_inbox, c2_inbox], None, None)) })
            });

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
                Box::pin(async move { Ok(out) })
            });

        let c1_channels = c1.clone();
        let c2_channels = c2.clone();
        conversation_repo
            .expect_list_channels_for_user()
            .return_once(move |_, _, _, _, _, _| {
                Box::pin(async move { Ok(Page::new(vec![c1_channels, c2_channels], None, None)) })
            });

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

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
                filter: Default::default(),
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
    async fn add_members_rejected_for_direct_message() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let mut c = conv(server_id);
        c.conversation_type = ConversationType::Direct;
        let c_id = c.id;

        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
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

        // Actor is a member of the Direct conversation — test verifies type check fires anyway.
        conversation_member_repo
            .expect_is_member()
            .returning(|_cid, _uid| Box::pin(async { Ok(true) }));

        let c_for_repo = c.clone();
        conversation_repo.expect_get_by_id().returning(move |_id| {
            let c = c_for_repo.clone();
            Box::pin(async move { Ok(Some(c)) })
        });

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

        let err = svc
            .add_conversation_members(AddConversationMembersCmd {
                actor: Actor::new(actor_id),
                server_id,
                conversation_id: c_id,
                user_ids: vec![UserId::new()],
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
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| {
                Ok(Some(rim_core::server_member::model::MemberStanding::new(
                    rim_core::server_member::model::ServerMemberRole::Member,
                )))
            });

        conversation_repo
            .expect_list_inbox_conversations_for_user()
            .returning(move |_uid, _sid, _limit, _dir, _cursor| {
                let c1 = c1.clone();
                let c2 = c2.clone();
                Box::pin(async move { Ok(Page::new(vec![c1, c2], None, None)) })
            });

        conversation_member_repo
            .expect_list_member_ids_for_conversations()
            .return_once(move |ids| {
                let mut out = std::collections::HashMap::new();
                for id in ids {
                    out.insert(*id, vec![]);
                }
                Box::pin(async move { Ok(out) })
            });

        // Also satisfy channels path even if not called.
        conversation_repo.expect_list_channels_for_user().returning(
            |_uid, _sid, _filter, _limit, _dir, _cursor| {
                Box::pin(async { Ok(Page::new(vec![], None, None)) })
            },
        );

        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );

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

    // ── Join / leave / invite / remove / archive authorization ──
    //
    // `Fx` wires the three repos as small in-memory fakes over one conversation so each test
    // states only the situation (conversation kind, members, actor role) and the outcome.

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    struct Fx {
        server_id: ServerId,
        conversation: Conversation,
        members: Arc<Mutex<Vec<UserId>>>,
        /// Pending conversation members.
        pending: Arc<Mutex<Vec<UserId>>>,
        /// Users with a device ready for encryption.
        ready: Vec<UserId>,
        /// Users whose accounts live on another server.
        remote: Vec<UserId>,
        /// Pending invitations each user holds elsewhere.
        held_invites: Vec<(UserId, usize)>,
        limits: rim_services::limits::InstanceLimits,
        /// Workspace members and their roles; anyone else is not in the workspace.
        roles: Vec<(UserId, ServerMemberRole)>,
        /// Workspace members whose timeout is in force.
        timed_out: Vec<UserId>,
        events: Arc<RecordingBroadcaster>,
        /// `(user, conversation)` pairs whose notifications were deleted.
        cleared_notifications: Arc<Mutex<Vec<(UserId, ConversationId)>>>,
        /// Whether the workspace is the server's default workspace.
        default_workspace: bool,
        /// The unarchived channels the workspace holds.
        channels: u64,
    }

    impl Fx {
        fn new(kind: ConversationType, is_private: bool, members: &[UserId]) -> Self {
            let server_id = ServerId::new();
            let mut conversation = conv(server_id);
            conversation.conversation_type = kind;
            conversation.is_private = is_private;
            Self {
                server_id,
                conversation,
                members: Arc::new(Mutex::new(members.to_vec())),
                pending: Arc::default(),
                ready: vec![],
                remote: vec![],
                held_invites: vec![],
                limits: Default::default(),
                roles: vec![],
                timed_out: vec![],
                events: Arc::new(RecordingBroadcaster::default()),
                cleared_notifications: Arc::default(),
                default_workspace: false,
                channels: 0,
            }
        }

        /// Makes the workspace the server's default workspace.
        fn default_workspace(mut self) -> Self {
            self.default_workspace = true;
            self
        }

        /// Caps channels per workspace at `cap` while the workspace holds `channels`.
        fn channel_cap(mut self, cap: u32, channels: u64) -> Self {
            self.limits.channels_per_workspace = Some(cap);
            self.channels = channels;
            self
        }

        fn with_workspace_member(mut self, user: UserId, role: ServerMemberRole) -> Self {
            self.roles.push((user, role));
            self
        }

        fn timed_out(mut self, user: UserId) -> Self {
            self.timed_out.push(user);
            self
        }

        fn archived(mut self) -> Self {
            self.conversation.archived_at = Some(chrono::Utc::now());
            self
        }

        fn encrypted(mut self) -> Self {
            self.conversation.encrypted_at = Some(chrono::Utc::now());
            self
        }

        fn ready(mut self, users: &[UserId]) -> Self {
            self.ready.extend_from_slice(users);
            self
        }

        fn remote(mut self, users: &[UserId]) -> Self {
            self.remote.extend_from_slice(users);
            self
        }

        /// Caps pending invitations per user at `cap`; `user` already holds `held` elsewhere.
        fn invite_cap(mut self, cap: u32, user: UserId, held: usize) -> Self {
            self.limits.pending_invites_per_user = Some(cap);
            self.held_invites.push((user, held));
            self
        }

        fn with_pending(self, users: &[UserId]) -> Self {
            self.pending.lock().expect("lock").extend_from_slice(users);
            self
        }

        fn members(&self) -> Vec<UserId> {
            self.members.lock().expect("lock").clone()
        }

        fn pending(&self) -> Vec<UserId> {
            self.pending.lock().expect("lock").clone()
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            self.events.0.lock().expect("lock").clone()
        }

        fn cleared_notifications(&self) -> Vec<(UserId, ConversationId)> {
            self.cleared_notifications.lock().expect("lock").clone()
        }

        fn service(&self) -> DefaultConversationService {
            let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
                base_repos();

            let roles = self.roles.clone();
            let server_id = self.server_id;
            server_member_repo
                .expect_is_member()
                .returning(move |sid, uid| {
                    Ok(sid == server_id && roles.iter().any(|(u, _)| *u == uid))
                });
            server_member_repo
                .expect_join_direct_space()
                .returning(|_, _| Ok(false));
            let roles = self.roles.clone();
            let timed_out = self.timed_out.clone();
            server_member_repo
                .expect_get_member_standing()
                .returning(move |sid, uid| {
                    Ok(roles
                        .iter()
                        .find(|(u, _)| sid == server_id && *u == uid)
                        .map(|(_, role)| MemberStanding {
                            role: *role,
                            timed_out_until: timed_out
                                .contains(&uid)
                                .then(|| chrono::Utc::now() + chrono::Duration::minutes(5)),
                        }))
                });
            let roles = self.roles.clone();
            server_member_repo
                .expect_get_member()
                .returning(move |sid, uid| {
                    Ok(roles
                        .iter()
                        .find(|(u, _)| *u == uid)
                        .map(|(u, role)| ServerMember {
                            id: ServerMemberId::new(),
                            server_id: sid,
                            user_id: *u,
                            member_role: *role,
                            joined_at: chrono::Utc::now(),
                        }))
                });

            let stored = Arc::new(Mutex::new(self.conversation.clone()));
            let get = Arc::clone(&stored);
            conversation_repo.expect_get_by_id().returning(move |id| {
                let c = get.lock().expect("lock").clone();
                Box::pin(async move { Ok((c.id == id).then_some(c)) })
            });
            conversation_repo
                .expect_find_direct_between_users()
                .returning(|_, _, _| Box::pin(async { Ok(None) }));
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            let template = self.conversation.clone();
            conversation_repo
                .expect_create_conversation_with_members()
                .returning(move |new, members, pending| {
                    m.lock().expect("lock").extend_from_slice(members);
                    p.lock().expect("lock").extend_from_slice(pending);
                    let mut c = template.clone();
                    c.id = ConversationId::new();
                    c.conversation_type = new.conversation_type;
                    c.encrypted_at = new.encrypted.then(chrono::Utc::now);
                    Box::pin(async move { Ok(c) })
                });
            let channels = self.channels;
            conversation_repo
                .expect_count_channels()
                .returning(move |_| Box::pin(async move { Ok(channels) }));
            conversation_repo
                .expect_exists_channel_by_title()
                .returning(|_, _| Box::pin(async { Ok(false) }));
            let set = Arc::clone(&stored);
            conversation_repo
                .expect_set_archived()
                .returning(move |_id, archived_at| {
                    let mut c = set.lock().expect("lock");
                    c.archived_at = archived_at;
                    let c = c.clone();
                    Box::pin(async move { Ok(c) })
                });
            let topic = Arc::clone(&stored);
            conversation_repo
                .expect_update_topic()
                .returning(move |_id, new_topic| {
                    let mut c = topic.lock().expect("lock");
                    c.topic = new_topic;
                    let c = c.clone();
                    Box::pin(async move { Ok(c) })
                });

            let m = Arc::clone(&self.members);
            conversation_member_repo
                .expect_is_member()
                .returning(move |_cid, uid| {
                    let is = m.lock().expect("lock").contains(&uid);
                    Box::pin(async move { Ok(is) })
                });
            let m = Arc::clone(&self.members);
            conversation_member_repo
                .expect_list_member_ids()
                .returning(move |_cid| {
                    let ids = m.lock().expect("lock").clone();
                    Box::pin(async move { Ok(ids) })
                });
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            conversation_member_repo
                .expect_membership()
                .returning(move |_cid, uid| {
                    let membership = if m.lock().expect("lock").contains(&uid) {
                        Some(Membership::Active)
                    } else if p.lock().expect("lock").contains(&uid) {
                        Some(Membership::Pending)
                    } else {
                        None
                    };
                    Box::pin(async move { Ok(membership) })
                });
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            conversation_member_repo
                .expect_activate_member()
                .returning(move |_cid, uid| {
                    let mut pending = p.lock().expect("lock");
                    let waiting = pending.contains(&uid);
                    if waiting {
                        pending.retain(|u| *u != uid);
                        m.lock().expect("lock").push(uid);
                    }
                    Box::pin(async move { Ok(waiting) })
                });
            let (p, held) = (Arc::clone(&self.pending), self.held_invites.clone());
            conversation_member_repo
                .expect_list_pending_conversation_ids()
                .returning(move |uid| {
                    let elsewhere = held.iter().find(|(u, _)| *u == uid).map_or(0, |(_, n)| *n);
                    let here = usize::from(p.lock().expect("lock").contains(&uid));
                    let ids = vec![ConversationId::new(); elsewhere + here];
                    Box::pin(async move { Ok(ids) })
                });
            let p = Arc::clone(&self.pending);
            conversation_member_repo
                .expect_list_pending_member_ids()
                .returning(move |_cid| {
                    let ids = p.lock().expect("lock").clone();
                    Box::pin(async move { Ok(ids) })
                });
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            conversation_member_repo
                .expect_add_member()
                .returning(move |cid, uid| {
                    let row = insert_member(&m, &p, cid, uid, false);
                    Box::pin(async move { row })
                });
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            conversation_member_repo
                .expect_add_pending_member()
                .returning(move |cid, uid| {
                    let row = insert_member(&m, &p, cid, uid, true);
                    Box::pin(async move { row })
                });
            let (m, p) = (Arc::clone(&self.members), Arc::clone(&self.pending));
            conversation_member_repo
                .expect_remove_member()
                .returning(move |_cid, uid| {
                    let mut members = m.lock().expect("lock");
                    let mut pending = p.lock().expect("lock");
                    let before = members.len() + pending.len();
                    members.retain(|u| *u != uid);
                    pending.retain(|u| *u != uid);
                    let removed = members.len() + pending.len() != before;
                    Box::pin(async move {
                        if removed {
                            Ok(())
                        } else {
                            Err(RepoError::new(RepoErrorKind::NotFound))
                        }
                    })
                });

            let mut notification_repo = MockNotificationRepository::new();
            let cleared = Arc::clone(&self.cleared_notifications);
            notification_repo
                .expect_delete_for_user_in_conversation()
                .returning(move |user_id, conversation_id| {
                    cleared
                        .lock()
                        .expect("lock")
                        .push((user_id, conversation_id));
                    Box::pin(async { Ok(1) })
                });

            let mut e2ee_repo = MockE2eeRepository::new();
            let ready = self.ready.clone();
            e2ee_repo.expect_ready_users().returning(move |ids, _| {
                let found = ids
                    .iter()
                    .copied()
                    .filter(|id| ready.contains(id))
                    .collect();
                Box::pin(async move { Ok(found) })
            });
            let remote = self.remote.clone();
            e2ee_repo.expect_remote_users().returning(move |ids| {
                let found = ids
                    .iter()
                    .copied()
                    .filter(|id| remote.contains(id))
                    .collect();
                Box::pin(async move { Ok(found) })
            });

            DefaultConversationService::new(
                ConversationRepos {
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    server_member_repo: Arc::new(server_member_repo),
                    notification_repo: Arc::new(notification_repo),
                },
                Observers::new(Arc::clone(&self.events) as Arc<dyn Broadcaster>),
            )
            .with_e2ee_repo(Arc::new(e2ee_repo))
            .with_limits(self.limits)
            .with_default_workspace(Arc::new(default_workspace_service(
                self.default_workspace.then_some(self.server_id),
            )))
        }

        fn invite_cmd(&self, actor: UserId) -> ConversationInviteCmd {
            ConversationInviteCmd {
                actor: Actor::new(actor),
                server_id: self.server_id,
                conversation_id: self.conversation.id,
            }
        }

        async fn accept(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .accept_conversation_invite(self.invite_cmd(actor))
                .await
                .map(|_| ())
        }

        async fn decline(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .decline_conversation_invite(self.invite_cmd(actor))
                .await
        }

        /// Creates an encrypted conversation as `actor`.
        async fn create_encrypted(
            &self,
            actor: UserId,
            participants: ConversationParticipants,
        ) -> Result<rim_services::conversation::dto::ConversationDto, ServiceError> {
            self.service()
                .create_conversation(CreateConversationCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    participants,
                    title: None,
                    is_private: false,
                    encrypted: true,
                })
                .await
        }

        async fn join(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .join_channel(JoinChannelCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                })
                .await
                .map(|_| ())
        }

        async fn leave(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .leave_conversation(LeaveConversationCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                })
                .await
        }

        async fn invite(&self, actor: UserId, users: &[UserId]) -> Result<(), ServiceError> {
            self.service()
                .add_conversation_members(AddConversationMembersCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                    user_ids: users.to_vec(),
                })
                .await
                .map(|_| ())
        }

        async fn remove(&self, actor: UserId, user: UserId) -> Result<(), ServiceError> {
            self.service()
                .remove_conversation_member(RemoveConversationMemberCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                    user_id: user,
                })
                .await
        }

        async fn set_archived(&self, actor: UserId, archived: bool) -> Result<bool, ServiceError> {
            self.service()
                .set_conversation_archived(ArchiveConversationCmd {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                    archived,
                })
                .await
                .map(|dto| dto.is_archived())
        }

        async fn get_writable(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .get_writable_conversation(GetConversationQuery {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                })
                .await
                .map(|_| ())
        }

        async fn get(&self, actor: UserId) -> Result<(), ServiceError> {
            self.service()
                .get_conversation(GetConversationQuery {
                    actor: Actor::new(actor),
                    server_id: self.server_id,
                    conversation_id: self.conversation.id,
                })
                .await
                .map(|_| ())
        }
    }

    /// Adds `uid` to the fixture's active or pending members; `Conflict` when already either.
    fn insert_member(
        members: &Mutex<Vec<UserId>>,
        pending: &Mutex<Vec<UserId>>,
        cid: ConversationId,
        uid: UserId,
        as_pending: bool,
    ) -> Result<ConversationMember, RepoError> {
        let mut members = members.lock().expect("lock");
        let mut pending = pending.lock().expect("lock");
        if members.contains(&uid) || pending.contains(&uid) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        if as_pending {
            pending.push(uid);
        } else {
            members.push(uid);
        }
        Ok(ConversationMember {
            id: ConversationMemberId::new(),
            user_id: uid,
            conversation_id: cid,
            joined_at: chrono::Utc::now(),
            pending_since: as_pending.then(chrono::Utc::now),
        })
    }

    fn users<const N: usize>() -> [UserId; N] {
        std::array::from_fn(|_| UserId::new())
    }

    #[tokio::test]
    async fn private_channel_is_not_found_for_non_member_and_readable_for_member() {
        let [alice, carol] = users();
        let fx = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);

        fx.get(alice)
            .await
            .expect("member reads the private channel");
        assert!(matches!(fx.get(carol).await, Err(ServiceError::NotFound)));
        // Even the workspace owner does not see a private channel they are not in.
        let owner = UserId::new();
        let fx = fx.with_workspace_member(owner, ServerMemberRole::Owner);
        assert!(matches!(fx.get(owner).await, Err(ServiceError::NotFound)));
    }

    #[tokio::test]
    async fn create_rejects_private_direct_and_group() {
        let [alice, bob] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member);
        for participants in [
            ConversationParticipants::DirectMessage { other_user_id: bob },
            ConversationParticipants::Group {
                member_ids: vec![bob],
            },
        ] {
            let err = fx
                .service()
                .create_conversation(CreateConversationCmd {
                    actor: Actor::new(alice),
                    server_id: fx.server_id,
                    participants,
                    title: None,
                    is_private: true,
                    encrypted: false,
                })
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
        }
    }

    #[tokio::test]
    async fn join_public_channel_is_idempotent_and_emits_once() {
        let [alice, carol] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);

        fx.join(carol).await.expect("join");
        fx.join(carol).await.expect("join again");
        assert_eq!(fx.members(), vec![alice, carol]);
        let added: Vec<_> = fx
            .events()
            .into_iter()
            .filter(|e| matches!(e, WorkspaceEvent::MemberAddedToConversation { user_id, .. } if *user_id == carol))
            .collect();
        assert_eq!(added.len(), 1);
    }

    #[tokio::test]
    async fn join_rejects_private_archived_and_non_channels() {
        let [alice, carol] = users();
        let private = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(carol, ServerMemberRole::Member);
        assert!(matches!(
            private.join(carol).await,
            Err(ServiceError::NotFound)
        ));
        assert_eq!(private.members(), vec![alice]);

        let archived = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(carol, ServerMemberRole::Member)
            .archived();
        assert!(matches!(
            archived.join(carol).await,
            Err(ServiceError::ConversationArchived)
        ));

        let group = Fx::new(ConversationType::Group, false, &[alice, carol])
            .with_workspace_member(carol, ServerMemberRole::Member);
        assert!(matches!(
            group.join(carol).await,
            Err(ServiceError::Validation(_))
        ));

        let outsider = UserId::new();
        let public = Fx::new(ConversationType::Channel, false, &[alice]);
        assert!(matches!(
            public.join(outsider).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn leave_removes_member_and_notifies_the_leaver() {
        let [alice, bob] = users();
        let fx = Fx::new(ConversationType::Channel, true, &[alice, bob])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member);

        fx.leave(bob).await.expect("leave");
        assert_eq!(fx.members(), vec![alice]);
        assert!(fx.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberRemovedFromConversation { user_id, member_user_ids, .. }
                if *user_id == bob && member_user_ids.contains(&bob) && member_user_ids.contains(&alice)
        )));
        assert_eq!(fx.cleared_notifications(), [(bob, fx.conversation.id)]);
        // Having left a private channel, bob can no longer see it.
        assert!(matches!(fx.get(bob).await, Err(ServiceError::NotFound)));
        assert!(matches!(fx.leave(bob).await, Err(ServiceError::NotFound)));
        assert_eq!(
            fx.cleared_notifications().len(),
            1,
            "a failed leave clears nothing"
        );
    }

    #[tokio::test]
    async fn leave_rejects_direct_and_is_a_no_op_for_unjoined_public_channel() {
        let [alice, bob, carol] = users();
        let dm = Fx::new(ConversationType::Direct, false, &[alice, bob])
            .with_workspace_member(alice, ServerMemberRole::Member);
        assert!(matches!(
            dm.leave(alice).await,
            Err(ServiceError::Validation(_))
        ));

        let public = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(carol, ServerMemberRole::Member);
        public.leave(carol).await.expect("no-op leave");
        assert_eq!(public.members(), vec![alice]);
        assert!(public.events().is_empty());
        assert!(public.cleared_notifications().is_empty());
    }

    #[tokio::test]
    async fn last_member_may_leave_a_private_channel() {
        let [alice] = users();
        let fx = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Owner);
        fx.leave(alice).await.expect("last member leaves");
        assert!(fx.members().is_empty());
    }

    #[tokio::test]
    async fn invite_requires_membership_and_workspace_targets() {
        let [alice, bob, carol] = users();
        let outsider = UserId::new();
        let fx = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);

        // A non-member cannot tell the private channel exists, let alone invite into it.
        assert!(matches!(
            fx.invite(carol, &[carol]).await,
            Err(ServiceError::NotFound)
        ));
        // Targets must be workspace members.
        assert!(matches!(
            fx.invite(alice, &[outsider]).await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            fx.invite(alice, &[]).await,
            Err(ServiceError::Validation(_))
        ));

        // Duplicates and existing members are skipped; one event per added user.
        fx.invite(alice, &[bob, bob, alice, carol])
            .await
            .expect("invite");
        assert_eq!(fx.members(), vec![alice, bob, carol]);
        let added = fx
            .events()
            .into_iter()
            .filter(|e| matches!(e, WorkspaceEvent::MemberAddedToConversation { .. }))
            .count();
        assert_eq!(added, 2);
    }

    #[tokio::test]
    async fn invite_into_public_channel_requires_joining_first() {
        let [alice, bob, carol] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);
        assert!(matches!(
            fx.invite(carol, &[bob]).await,
            Err(ServiceError::PermissionDenied)
        ));
        fx.invite(alice, &[bob]).await.expect("member invites");
        assert_eq!(fx.members(), vec![alice, bob]);
    }

    #[tokio::test]
    async fn invite_rejects_direct_and_archived() {
        let [alice, bob, carol] = users();
        let dm = Fx::new(ConversationType::Direct, false, &[alice, bob])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);
        assert!(matches!(
            dm.invite(alice, &[carol]).await,
            Err(ServiceError::Validation(_))
        ));

        let archived = Fx::new(ConversationType::Group, false, &[alice, bob])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .archived();
        assert!(matches!(
            archived.invite(alice, &[carol]).await,
            Err(ServiceError::ConversationArchived)
        ));
    }

    #[tokio::test]
    async fn remove_member_role_matrix() {
        let [alice, bob, moderator] = users();
        let fresh = || {
            Fx::new(ConversationType::Channel, true, &[alice, bob])
                .with_workspace_member(alice, ServerMemberRole::Member)
                .with_workspace_member(bob, ServerMemberRole::Member)
                .with_workspace_member(moderator, ServerMemberRole::Moderator)
        };

        // A guest member cannot remove someone else.
        let fx = fresh();
        assert!(matches!(
            fx.remove(alice, bob).await,
            Err(ServiceError::PermissionDenied)
        ));
        assert_eq!(fx.members(), vec![alice, bob]);
        assert!(fx.cleared_notifications().is_empty());

        // Anyone may remove themselves.
        let fx = fresh();
        fx.remove(bob, bob).await.expect("self removal");
        assert_eq!(fx.members(), vec![alice]);

        // A moderator removes anyone, even from a private channel they are not in.
        let fx = fresh();
        fx.remove(moderator, bob).await.expect("moderator removal");
        assert_eq!(fx.members(), vec![alice]);
        assert_eq!(fx.cleared_notifications(), [(bob, fx.conversation.id)]);
        assert!(fx.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberRemovedFromConversation { user_id, .. } if *user_id == bob
        )));

        // Removing a non-member is NotFound.
        let fx = fresh();
        assert!(matches!(
            fx.remove(moderator, moderator).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn remove_member_hides_private_channel_from_non_member_guest() {
        let [alice, carol] = users();
        let fx = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);
        assert!(matches!(
            fx.remove(carol, alice).await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn remove_member_rejects_direct_message() {
        let [alice, bob, owner] = users();
        let fx = Fx::new(ConversationType::Direct, false, &[alice, bob])
            .with_workspace_member(owner, ServerMemberRole::Owner);
        assert!(matches!(
            fx.remove(owner, alice).await,
            Err(ServiceError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn archive_requires_owner_or_moderator_and_a_channel() {
        let [alice, owner, moderator] = users();
        let fx = Fx::new(ConversationType::Channel, true, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(owner, ServerMemberRole::Owner)
            .with_workspace_member(moderator, ServerMemberRole::Moderator);

        // A guest — even a member of the channel — gets FORBIDDEN (role failure).
        assert!(matches!(
            fx.set_archived(alice, true).await,
            Err(ServiceError::PermissionDenied)
        ));
        // A moderator archives a private channel they are not in, by id.
        assert!(fx.set_archived(moderator, true).await.expect("archive"));
        assert!(fx.events().iter().any(|e| matches!(
            e,
            WorkspaceEvent::ConversationUpdated { is_private: true, member_user_ids, .. }
                if member_user_ids == &vec![alice]
        )));
        assert!(!fx.set_archived(owner, false).await.expect("unarchive"));

        let group = Fx::new(ConversationType::Group, false, &[alice])
            .with_workspace_member(owner, ServerMemberRole::Owner);
        assert!(matches!(
            group.set_archived(owner, true).await,
            Err(ServiceError::Validation(_))
        ));
    }

    #[tokio::test]
    async fn unarchiving_a_channel_follows_the_channels_per_workspace_cap() {
        let [owner] = users();
        for (channels, allowed) in [(29, true), (30, false)] {
            let fx = Fx::new(ConversationType::Channel, false, &[])
                .with_workspace_member(owner, ServerMemberRole::Owner)
                .archived()
                .channel_cap(30, channels);
            match fx.set_archived(owner, false).await {
                Ok(archived) => assert!(allowed && !archived, "{channels}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{channels}");
                    assert_eq!(message, "this instance allows 30 channels per workspace");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
        // Archiving never counts.
        let fx = Fx::new(ConversationType::Channel, false, &[])
            .with_workspace_member(owner, ServerMemberRole::Owner)
            .channel_cap(30, 30);
        assert!(fx.set_archived(owner, true).await.expect("archive"));
    }

    #[tokio::test]
    async fn in_the_default_workspace_only_owners_and_moderators_create_channels() {
        let [member, guest, moderator, owner] = users();
        let fx = |default: bool| {
            let fx = Fx::new(ConversationType::Channel, false, &[])
                .with_workspace_member(member, ServerMemberRole::Member)
                .with_workspace_member(guest, ServerMemberRole::Guest)
                .with_workspace_member(moderator, ServerMemberRole::Moderator)
                .with_workspace_member(owner, ServerMemberRole::Owner);
            if default { fx.default_workspace() } else { fx }
        };
        let create = |fx: &Fx, actor: UserId, is_private: bool| {
            let svc = fx.service();
            let server_id = fx.server_id;
            async move {
                svc.create_conversation(CreateConversationCmd {
                    actor: Actor::new(actor),
                    server_id,
                    participants: ConversationParticipants::Channel,
                    title: Some("news".to_string()),
                    is_private,
                    encrypted: false,
                })
                .await
            }
        };

        let default = fx(true);
        for (actor, is_private) in [(member, false), (member, true)] {
            assert!(matches!(
                create(&default, actor, is_private).await,
                Err(ServiceError::Forbidden(m)) if m.contains("only the owner and moderators")
            ));
        }
        assert!(matches!(
            create(&default, guest, false).await,
            Err(ServiceError::PermissionDenied)
        ));
        for actor in [moderator, owner] {
            create(&default, actor, true).await.expect("staff creates");
        }
        create(&fx(false), member, true)
            .await
            .expect("a member creates channels in any other workspace");
    }

    #[tokio::test]
    async fn archive_is_idempotent_without_an_event() {
        let [owner] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[])
            .with_workspace_member(owner, ServerMemberRole::Owner)
            .archived();
        assert!(
            fx.set_archived(owner, true)
                .await
                .expect("already archived")
        );
        assert!(fx.events().is_empty());
    }

    #[tokio::test]
    async fn topic_update_on_archived_channel_is_rejected() {
        let [alice] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .archived();
        let err = fx
            .service()
            .update_topic(UpdateConversationTopicCmd {
                actor: Actor::new(alice),
                server_id: fx.server_id,
                conversation_id: fx.conversation.id,
                topic: Some("new".to_string()),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::ConversationArchived));
        // Archived channels stay readable.
        fx.get(alice).await.expect("archived channel is readable");
    }

    #[tokio::test]
    async fn list_channels_passes_the_filter_through() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
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
        let want = ChannelListFilter {
            joined_only: true,
            include_archived: true,
        };
        conversation_repo
            .expect_list_channels_for_user()
            .withf(move |uid, _sid, filter, _limit, _dir, _cursor| {
                *uid == actor_id && *filter == want
            })
            .return_once(|_, _, _, _, _, _| Box::pin(async { Ok(Page::new(vec![], None, None)) }));
        conversation_member_repo
            .expect_list_member_ids_for_conversations()
            .return_once(|_| Box::pin(async { Ok(std::collections::HashMap::new()) }));
        let svc = DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        );
        svc.list_channels(rim_services::conversation::command::ListChannelsQuery {
            actor: Actor::new(actor_id),
            server_id,
            filter: want,
            dir: PageDir::After,
            cursor: None,
            limit: 10,
        })
        .await
        .expect("list");
    }

    // ── Guest restrictions and timeouts ──

    #[tokio::test]
    async fn guest_cannot_create_channels_or_groups() {
        let [guest, member] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[])
            .with_workspace_member(guest, ServerMemberRole::Guest)
            .with_workspace_member(member, ServerMemberRole::Member);
        for participants in [
            ConversationParticipants::Channel,
            ConversationParticipants::Group {
                member_ids: vec![member],
            },
        ] {
            let err = fx
                .service()
                .create_conversation(CreateConversationCmd {
                    actor: Actor::new(guest),
                    server_id: fx.server_id,
                    participants,
                    title: Some("guest-room".to_string()),
                    is_private: false,
                    encrypted: false,
                })
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        }
    }

    #[tokio::test]
    async fn guest_cannot_join_invite_or_read_unjoined_public_channels() {
        let [alice, guest, carol] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(guest, ServerMemberRole::Guest)
            .with_workspace_member(carol, ServerMemberRole::Member);

        assert!(matches!(
            fx.join(guest).await,
            Err(ServiceError::PermissionDenied)
        ));
        assert!(matches!(
            fx.invite(guest, &[carol]).await,
            Err(ServiceError::PermissionDenied)
        ));
        assert!(
            matches!(fx.get(guest).await, Err(ServiceError::NotFound)),
            "a public channel the guest was not added to stays hidden"
        );

        // Once a member adds the guest, the guest reads it like any member, but still cannot
        // invite others.
        fx.invite(alice, &[guest]).await.expect("member adds guest");
        fx.get(guest).await.expect("guest reads the channel");
        assert!(matches!(
            fx.invite(guest, &[carol]).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn timed_out_member_cannot_join_invite_or_create() {
        let [alice, carol] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .timed_out(carol)
            .timed_out(alice);

        assert!(matches!(
            fx.join(carol).await,
            Err(ServiceError::TimedOut { .. })
        ));
        assert!(matches!(
            fx.invite(alice, &[carol]).await,
            Err(ServiceError::TimedOut { .. })
        ));
        let err = fx
            .service()
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(carol),
                server_id: fx.server_id,
                participants: ConversationParticipants::Channel,
                title: Some("muted".to_string()),
                is_private: false,
                encrypted: false,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::TimedOut { .. }), "{err:?}");
        // Reads continue.
        fx.get(carol).await.expect("timed-out member still reads");
    }

    #[tokio::test]
    async fn writable_conversation_refuses_timed_out_members_and_archived_channels() {
        let [alice, carol] = users();
        let fx = Fx::new(ConversationType::Channel, false, &[alice, carol])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .timed_out(carol);
        fx.get_writable(alice).await.expect("member may write");
        assert!(matches!(
            fx.get_writable(carol).await,
            Err(ServiceError::TimedOut { .. })
        ));

        let fx = Fx::new(ConversationType::Channel, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .archived();
        assert!(matches!(
            fx.get_writable(alice).await,
            Err(ServiceError::ConversationArchived)
        ));
    }

    #[tokio::test]
    async fn guest_channel_list_is_always_joined_only() {
        use rim_core::repository::pagination::{Page, PageDir};

        for (role, browse_allowed) in [
            (ServerMemberRole::Guest, false),
            (ServerMemberRole::Member, true),
        ] {
            let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
                base_repos();
            server_member_repo
                .expect_get_member_standing()
                .returning(move |_, _| Ok(Some(MemberStanding::new(role))));
            conversation_repo
                .expect_list_channels_for_user()
                .withf(move |_, _, filter, _, _, _| filter.joined_only != browse_allowed)
                .times(1)
                .returning(|_, _, _, _, _, _| {
                    Box::pin(async { Ok(Page::new(vec![], None, None)) })
                });
            conversation_member_repo
                .expect_list_member_ids_for_conversations()
                .returning(|_| Box::pin(async { Ok(std::collections::HashMap::new()) }));
            let svc = DefaultConversationService::new(
                ConversationRepos {
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    server_member_repo: Arc::new(server_member_repo),
                    notification_repo: Arc::new(MockNotificationRepository::new()),
                },
                test_observers(),
            );
            svc.list_channels(ListChannelsQuery {
                actor: Actor::new(UserId::new()),
                server_id: ServerId::new(),
                filter: rim_core::conversation::repository::ChannelListFilter {
                    joined_only: false,
                    include_archived: false,
                },
                limit: 50,
                dir: PageDir::After,
                cursor: None,
            })
            .await
            .expect("list channels");
        }
    }

    // ── Instance limits ──

    /// A service where a member creates a channel in a workspace holding `channels` channels
    /// under `cap`; `None` channels expects them never to be counted.
    fn channel_service(channels: Option<u64>, cap: Option<u32>) -> DefaultConversationService {
        let (mut conversation_repo, mut conversation_member_repo, mut server_member_repo) =
            base_repos();
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(Some(MemberStanding::new(ServerMemberRole::Member))));
        conversation_repo
            .expect_exists_channel_by_title()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        match channels {
            Some(channels) => {
                conversation_repo
                    .expect_count_channels()
                    .returning(move |_| Box::pin(async move { Ok(channels) }));
            }
            None => {
                conversation_repo.expect_count_channels().never();
            }
        }
        conversation_repo
            .expect_create_conversation_with_members()
            .returning(|new, _, _| {
                let mut created = conv(new.server_id);
                created.conversation_type = ConversationType::Channel;
                created.is_private = new.is_private;
                Box::pin(async move { Ok(created) })
            });
        conversation_member_repo
            .expect_list_member_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        DefaultConversationService::new(
            ConversationRepos {
                conversation_repo: Arc::new(conversation_repo),
                conversation_member_repo: Arc::new(conversation_member_repo),
                server_member_repo: Arc::new(server_member_repo),
                notification_repo: Arc::new(MockNotificationRepository::new()),
            },
            test_observers(),
        )
        .with_limits(rim_services::limits::InstanceLimits {
            channels_per_workspace: cap,
            ..Default::default()
        })
    }

    #[tokio::test]
    async fn creating_a_channel_follows_the_channels_per_workspace_cap() {
        for (cap, channels, is_private, allowed) in [
            (Some(30), Some(29), false, true),
            (Some(30), Some(30), true, false),
            (Some(30), Some(30), false, false),
            (None, None, false, true),
        ] {
            let result = channel_service(channels, cap)
                .create_conversation(CreateConversationCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    participants: ConversationParticipants::Channel,
                    title: Some("news".to_string()),
                    is_private,
                    encrypted: false,
                })
                .await;
            match result {
                Ok(_) => assert!(allowed, "{cap:?} {channels:?}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {channels:?}");
                    assert_eq!(message, "this instance allows 30 channels per workspace");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    // ── Pending members of encrypted conversations ──

    fn dm(other_user_id: UserId) -> ConversationParticipants {
        ConversationParticipants::DirectMessage { other_user_id }
    }

    fn joined(events: &[WorkspaceEvent]) -> Vec<(UserId, Vec<UserId>)> {
        events
            .iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MemberAddedToConversation {
                    user_id,
                    member_user_ids,
                    ..
                } => Some((*user_id, member_user_ids.clone())),
                _ => None,
            })
            .collect()
    }

    fn mls_changes(events: &[WorkspaceEvent]) -> usize {
        events
            .iter()
            .filter(|e| matches!(e, WorkspaceEvent::MlsMembershipChanged { .. }))
            .count()
    }

    #[tokio::test]
    async fn encrypted_dm_with_an_unready_party_makes_them_pending() {
        let [alice, bob] = users();
        let fx = Fx::new(ConversationType::Direct, false, &[])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .ready(&[alice]);

        let dto = fx.create_encrypted(alice, dm(bob)).await.expect("created");

        assert_eq!(dto.member_ids, vec![alice]);
        assert_eq!(dto.pending_member_ids, vec![bob]);
        let events = fx.events();
        // Bob hears of the conversation through his own join.
        assert_eq!(joined(&events), vec![(bob, vec![alice, bob])]);
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::ConversationCreated { member_user_ids, .. } if *member_user_ids == [alice]
        )));
    }

    #[tokio::test]
    async fn encrypted_conversations_refuse_an_unready_creator_or_remote_member() {
        let [alice, bob, remote] = users();
        let fresh = || {
            Fx::new(ConversationType::Group, false, &[])
                .with_workspace_member(alice, ServerMemberRole::Member)
                .with_workspace_member(bob, ServerMemberRole::Member)
                .with_workspace_member(remote, ServerMemberRole::Member)
                .remote(&[remote])
        };
        let not_ready = |r: Result<_, ServiceError>| matches!(r, Err(ServiceError::Validation(m)) if m.contains("needs a device set up"));

        let fx = fresh().ready(&[bob]);
        assert!(not_ready(fx.create_encrypted(alice, dm(bob)).await));
        assert!(fx.members().is_empty() && fx.pending().is_empty());

        let fx = fresh().ready(&[alice]);
        let group = ConversationParticipants::Group {
            member_ids: vec![bob, remote],
        };
        assert!(not_ready(fx.create_encrypted(alice, group).await));
        assert!(fx.members().is_empty() && fx.pending().is_empty());
    }

    #[tokio::test]
    async fn inviting_unready_users_to_an_encrypted_group_adds_them_pending() {
        let [alice, bob, carol] = users();
        let fx = Fx::new(ConversationType::Group, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .ready(&[alice, bob]);

        fx.invite(alice, &[bob, carol]).await.expect("invited");

        assert_eq!(fx.members(), vec![alice, bob]);
        assert_eq!(fx.pending(), vec![carol]);
        let events = fx.events();
        assert_eq!(
            joined(&events),
            vec![(bob, vec![alice, bob]), (carol, vec![alice, bob, carol])]
        );
        assert_eq!(mls_changes(&events), 1);

        // A pending invitee invited again stays pending, with no event.
        fx.invite(alice, &[carol]).await.expect("invited again");
        assert_eq!(fx.pending(), vec![carol]);
        assert!(joined(&fx.events()[events.len()..]).is_empty());
    }

    #[tokio::test]
    async fn plaintext_conversations_never_have_pending_members() {
        let [alice, bob, carol] = users();
        let fx = Fx::new(ConversationType::Group, false, &[alice])
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member);

        fx.invite(alice, &[bob]).await.expect("invited");
        let dto = fx
            .service()
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(alice),
                server_id: fx.server_id,
                participants: ConversationParticipants::Group {
                    member_ids: vec![carol],
                },
                title: None,
                is_private: false,
                encrypted: false,
            })
            .await
            .expect("created");

        assert!(fx.pending().is_empty());
        assert!(dto.pending_member_ids.is_empty());
        assert_eq!(mls_changes(&fx.events()), 0);
    }

    #[tokio::test]
    async fn the_encrypted_member_cap_counts_pending_members() {
        let [alice, carol] = users();
        let waiting: Vec<UserId> = (1..rim_services::e2ee::MAX_ENCRYPTED_MEMBERS)
            .map(|_| UserId::new())
            .collect();
        let fx = Fx::new(ConversationType::Group, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .with_pending(&waiting)
            .ready(&[alice, carol]);

        let err = fx.invite(alice, &[carol]).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(m) if m.contains("at most")));
    }

    #[tokio::test]
    async fn pending_members_may_leave_or_be_removed_without_an_mls_change() {
        let [alice, carol, moderator] = users();
        let fresh = || {
            Fx::new(ConversationType::Group, false, &[alice])
                .encrypted()
                .with_workspace_member(alice, ServerMemberRole::Member)
                .with_workspace_member(carol, ServerMemberRole::Member)
                .with_workspace_member(moderator, ServerMemberRole::Moderator)
                .with_pending(&[carol])
        };

        // A pending member cannot read the conversation, like a non-member.
        let fx = fresh();
        assert!(matches!(
            fx.get(carol).await,
            Err(ServiceError::PermissionDenied)
        ));

        let fx = fresh();
        fx.leave(carol).await.expect("left");
        assert!(fx.pending().is_empty());
        assert_eq!(fx.members(), vec![alice]);
        assert_eq!(fx.cleared_notifications(), [(carol, fx.conversation.id)]);
        let events = fx.events();
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberRemovedFromConversation { user_id, member_user_ids, .. }
                if *user_id == carol && member_user_ids.contains(&carol)
        )));
        assert_eq!(mls_changes(&events), 0);

        let fx = fresh();
        fx.remove(moderator, carol).await.expect("removed");
        assert!(fx.pending().is_empty());
        assert_eq!(mls_changes(&fx.events()), 0);
    }

    #[tokio::test]
    async fn accepting_an_invitation_needs_a_ready_device_and_happens_once() {
        let [alice, carol] = users();
        let mut fx = Fx::new(ConversationType::Group, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(carol, ServerMemberRole::Member)
            .with_pending(&[carol]);

        let err = fx.accept(carol).await.unwrap_err();
        assert!(matches!(err, ServiceError::Validation(m) if m.contains("needs a device set up")));
        assert_eq!(fx.pending(), vec![carol]);

        fx.ready.push(carol);
        fx.accept(carol).await.expect("accepted");
        assert_eq!(fx.members(), vec![alice, carol]);
        assert!(fx.pending().is_empty());
        let events = fx.events();
        assert_eq!(joined(&events), vec![(carol, vec![alice, carol])]);
        assert_eq!(mls_changes(&events), 1);

        fx.accept(carol).await.expect("accepted again");
        assert_eq!(fx.events().len(), events.len());
    }

    #[tokio::test]
    async fn declining_an_invitation_deletes_it_without_an_mls_change() {
        let [alice, bob] = users();
        let fx = Fx::new(ConversationType::Direct, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .with_pending(&[bob]);

        fx.decline(bob).await.expect("declined");

        assert!(fx.pending().is_empty());
        assert_eq!(fx.members(), vec![alice]);
        assert_eq!(fx.cleared_notifications(), [(bob, fx.conversation.id)]);
        let events = fx.events();
        assert!(events.iter().any(|e| matches!(
            e,
            WorkspaceEvent::MemberRemovedFromConversation { user_id, member_user_ids, .. }
                if *user_id == bob && *member_user_ids == [alice, bob]
        )));
        assert_eq!(mls_changes(&events), 0);
    }

    #[tokio::test]
    async fn answering_without_an_invitation_is_not_found() {
        let [alice, bob] = users();
        let fx = Fx::new(ConversationType::Group, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .ready(&[alice, bob]);

        assert!(matches!(fx.accept(bob).await, Err(ServiceError::NotFound)));
        assert!(matches!(fx.decline(bob).await, Err(ServiceError::NotFound)));
        // A member has nothing to decline; accepting changes nothing.
        assert!(matches!(
            fx.decline(alice).await,
            Err(ServiceError::NotFound)
        ));
        fx.accept(alice).await.expect("already a member");
        assert_eq!(fx.members(), vec![alice]);
        assert!(fx.events().is_empty());
    }

    #[tokio::test]
    async fn a_user_holding_the_most_pending_invitations_is_not_invited_again() {
        let [alice, bob, carol] = users();
        let fresh = |held| {
            Fx::new(ConversationType::Group, false, &[alice])
                .encrypted()
                .with_workspace_member(alice, ServerMemberRole::Member)
                .with_workspace_member(bob, ServerMemberRole::Member)
                .with_workspace_member(carol, ServerMemberRole::Member)
                .ready(&[alice, bob])
                .invite_cap(2, carol, held)
        };
        let capped = |r: Result<_, ServiceError>| {
            matches!(r, Err(ServiceError::Validation(m))
                if m == "this instance allows 2 pending invitations per user")
        };

        let fx = fresh(2);
        assert!(capped(fx.invite(alice, &[bob, carol]).await));
        assert!(fx.pending().is_empty());
        assert_eq!(fx.members(), vec![alice]);
        assert!(capped(
            fx.create_encrypted(alice, dm(carol)).await.map(|_| ())
        ));

        let fx = fresh(1);
        fx.invite(alice, &[carol]).await.expect("below the cap");
        assert_eq!(fx.pending(), vec![carol]);
        // A ready user is never capped.
        let fx = Fx::new(ConversationType::Group, false, &[alice])
            .encrypted()
            .with_workspace_member(alice, ServerMemberRole::Member)
            .with_workspace_member(bob, ServerMemberRole::Member)
            .ready(&[alice, bob])
            .invite_cap(2, bob, 5);
        fx.invite(alice, &[bob])
            .await
            .expect("ready joiners are not invitations");
    }
}
