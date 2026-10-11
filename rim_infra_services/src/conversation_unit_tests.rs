#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::{
        ChannelListFilter, ConversationActivityCursor, ConversationCreatedCursor,
        MockConversationRepository,
    };
    use rim_core::conversation_member::model::{ConversationMember, ConversationMemberId};
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::repository::pagination::{Page, PageDir};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{
        MemberStanding, ServerMember, ServerMemberId, ServerMemberRole,
    };
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_services::conversation::command::{
        AddConversationMembersCmd, ArchiveConversationCmd, CreateConversationCmd,
        GetConversationQuery, JoinChannelCmd, LeaveConversationCmd, ListChannelsQuery,
        RemoveConversationMemberCmd, UpdateConversationTopicCmd,
    };
    use rim_services::conversation::service::ConversationService;
    use rim_services::conversation::types::ConversationParticipants;
    use rim_services::{Actor, ServiceError};

    use rim_services::events::{Broadcaster, NoopBroadcaster, Observers, WorkspaceEvent};

    use crate::ConversationRepos;
    use crate::conversation::DefaultConversationService;

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
            .return_once(move |new, members| {
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
        /// Workspace members and their roles; anyone else is not in the workspace.
        roles: Vec<(UserId, ServerMemberRole)>,
        /// Workspace members whose timeout is in force.
        timed_out: Vec<UserId>,
        events: Arc<RecordingBroadcaster>,
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
                roles: vec![],
                timed_out: vec![],
                events: Arc::new(RecordingBroadcaster::default()),
            }
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

        fn members(&self) -> Vec<UserId> {
            self.members.lock().expect("lock").clone()
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            self.events.0.lock().expect("lock").clone()
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
            let m = Arc::clone(&self.members);
            conversation_member_repo
                .expect_add_member()
                .returning(move |cid, uid| {
                    m.lock().expect("lock").push(uid);
                    Box::pin(async move {
                        Ok(ConversationMember {
                            id: ConversationMemberId::new(),
                            user_id: uid,
                            conversation_id: cid,
                            joined_at: chrono::Utc::now(),
                        })
                    })
                });
            let m = Arc::clone(&self.members);
            conversation_member_repo
                .expect_remove_member()
                .returning(move |_cid, uid| {
                    let mut members = m.lock().expect("lock");
                    let before = members.len();
                    members.retain(|u| *u != uid);
                    let removed = members.len() != before;
                    Box::pin(async move {
                        if removed {
                            Ok(())
                        } else {
                            Err(RepoError::new(RepoErrorKind::NotFound))
                        }
                    })
                });

            DefaultConversationService::new(
                ConversationRepos {
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    server_member_repo: Arc::new(server_member_repo),
                },
                Observers::new(Arc::clone(&self.events) as Arc<dyn Broadcaster>),
            )
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
        // Having left a private channel, bob can no longer see it.
        assert!(matches!(fx.get(bob).await, Err(ServiceError::NotFound)));
        assert!(matches!(fx.leave(bob).await, Err(ServiceError::NotFound)));
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

        // Anyone may remove themselves.
        let fx = fresh();
        fx.remove(bob, bob).await.expect("self removal");
        assert_eq!(fx.members(), vec![alice]);

        // A moderator removes anyone, even from a private channel they are not in.
        let fx = fresh();
        fx.remove(moderator, bob).await.expect("moderator removal");
        assert_eq!(fx.members(), vec![alice]);
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
}
