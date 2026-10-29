#[cfg(test)]
mod tests {
    use std::collections::HashMap;
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{Duration, Utc};

    use rim_core::conversation::model::{
        Conversation, ConversationId, ConversationType, PostingPolicy,
    };
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message::repository::{
        AuthorChannelCounts, AuthorChannelScope, DeletedMessage, MessageTally,
        MockMessageRepository,
    };
    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::message_attachment::repository::MockMessageAttachmentRepository;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::moderation::model::{
        FrankedReport, MemberTimeout, MessageReport, MessageReportId, MessageReportStatus,
    };
    use rim_core::moderation::repository::MockModerationRepository;
    use rim_core::notification::repository::MockNotificationRepository;
    use rim_core::public_join_link::repository::MockPublicJoinLinkRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::ServerId;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::{User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::audit::dto::{AuditLogEntryDto, AuditLogPage, ListAuditLogQuery};
    use rim_services::audit::service::AuditLogService;
    use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
    use rim_services::moderation::command::{
        AuthorizeReportAttachmentQuery, BanMemberCmd, ClearTimeoutCmd, DeleteMemberMessagesCmd,
        GetReportQuery, KickMemberCmd, LeaveWorkspaceCmd, MemberMessageCountsQuery,
        ReportMessageCmd, ReportOutcome, ResolveReportCmd, SetChannelPostingCmd, SetMemberRoleCmd,
        SetSlowModeCmd, TimeoutMemberCmd, UnbanMemberCmd,
    };
    use rim_services::moderation::service::ModerationService;
    use rim_services::{Actor, ServiceError, ServiceResult};

    use crate::moderation::{
        DefaultModerationService, ModerationRepos, audit_ids, ensure_can_moderate,
        ensure_can_set_role,
    };

    use ServerMemberRole::{Guest, Member, Moderator, Owner};

    // ── Pure role matrices ──────────────────────────────────────────────

    #[test]
    fn moderation_target_matrix() {
        let roles = [Owner, Moderator, Member, Guest];
        for actor in roles {
            for target in roles {
                let allowed = ensure_can_moderate(actor, target).is_ok();
                let expected = matches!(
                    (actor, target),
                    (Owner, Moderator | Member | Guest) | (Moderator, Member | Guest)
                );
                assert_eq!(allowed, expected, "actor={actor:?} target={target:?}");
            }
        }
    }

    #[test]
    fn set_role_matrix() {
        let roles = [Owner, Moderator, Member, Guest];
        for actor in roles {
            for current in roles {
                for new in roles {
                    let res = ensure_can_set_role(actor, current, new);
                    let expected_ok = match (actor, current, new) {
                        (_, _, Owner) => false,
                        (Owner, c, _) => c != Owner,
                        (Moderator, Member | Guest, Member | Guest) => true,
                        _ => false,
                    };
                    assert_eq!(
                        res.is_ok(),
                        expected_ok,
                        "actor={actor:?} current={current:?} new={new:?}"
                    );
                    if new == Owner {
                        assert!(matches!(res, Err(ServiceError::Validation(_))));
                    }
                }
            }
        }
    }

    // ── Fixture ──────────────────────────────────────────────────────────

    #[derive(Default)]
    struct RecordingAudit(Mutex<Vec<AuditLogEntryDto>>);

    #[async_trait]
    impl AuditLogService for RecordingAudit {
        async fn write_entry(&self, entry: AuditLogEntryDto) -> ServiceResult<()> {
            self.0.lock().expect("lock").push(entry);
            Ok(())
        }

        async fn list_entries(
            &self,
            _caller: Actor,
            _query: ListAuditLogQuery,
        ) -> ServiceResult<AuditLogPage> {
            Err(ServiceError::NotFound)
        }
    }

    #[derive(Default)]
    struct RecordingBroadcaster(Mutex<Vec<WorkspaceEvent>>);

    impl Broadcaster for RecordingBroadcaster {
        fn broadcast(&self, event: WorkspaceEvent) {
            self.0.lock().expect("lock").push(event);
        }
    }

    fn user(id: UserId) -> User {
        User {
            id,
            username: format!("u{}", &id.to_string()[..8]),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: Some("u@example.com".to_string()),
            timezone: None,
            principal_id: None,
            deletion_scheduled_for: None,
            profile_locked_until: None,
            handle: rim_core::user::model::UserHandle::Local,
        }
    }

    /// The principal id the fixture's users hold when listed by id.
    fn principal_of(id: UserId) -> String {
        format!("rimp1_{id}")
    }

    fn channel(server_id: ServerId) -> Conversation {
        Conversation {
            id: ConversationId::new(),
            conversation_type: ConversationType::Channel,
            title: Some("general".to_string()),
            topic: None,
            created_at: Utc::now(),
            last_activity_at: Utc::now(),
            server_id,
            is_private: false,
            archived_at: None,
            created_by: None,
            slow_mode_seconds: 0,
            posting_policy: PostingPolicy::Everyone,
            posting_allows_threads: true,
            encrypted_at: None,
        }
    }

    /// What a removal deleted: `macros` or `pipelines`, of whom, in which workspace.
    type Cleared = (&'static str, UserId, ServerId);

    struct Fx {
        server_id: ServerId,
        roles: Vec<(UserId, ServerMemberRole)>,
        audit: Arc<RecordingAudit>,
        events: Arc<RecordingBroadcaster>,
        /// `(user, workspace)` pairs whose notifications were deleted.
        cleared_notifications: Arc<Mutex<Vec<(UserId, ServerId)>>>,
        /// `(kind, user, workspace)` for each user's macros and pipelines deleted.
        cleared_automations: Arc<Mutex<Vec<Cleared>>>,
        conversation: Option<Conversation>,
        message: Option<Message>,
        /// Whether every moderator is a member of every conversation.
        conversation_member: bool,
        /// Attachments of the fixture's message.
        attachments: Vec<MessageAttachment>,
        /// Stored attachments anywhere in the workspace, found by blob hash.
        workspace_attachments: Vec<MessageAttachment>,
        /// The instance operators.
        operators: Vec<UserId>,
        /// Whether the fixture's workspace is the relay's direct space.
        direct_space: bool,
        /// Users with no account: `get_by_id` finds nobody.
        unknown_users: Vec<UserId>,
        /// What `count_author_channel_messages` answers.
        author_counts: AuthorChannelCounts,
        /// The channels holding the target's messages, each with its messages newest first;
        /// a private one counts as a channel the actor is not in.
        author_channels: Vec<(Conversation, Vec<DeletedMessage>)>,
        /// The scopes `count_author_channel_messages` was asked for.
        count_scopes: Arc<Mutex<Vec<AuthorChannelScope>>>,
        /// The `include_unjoined_private` flags `list_author_channels` was asked with.
        include_flags: Arc<Mutex<Vec<bool>>>,
        /// `(channel, limit)` of every bulk delete statement.
        bulk_deletes: Arc<Mutex<Vec<(ConversationId, u32)>>>,
        /// Messages whose unread notifications were deleted.
        forgotten: Arc<Mutex<Vec<MessageId>>>,
    }

    impl Fx {
        fn new(roles: &[(UserId, ServerMemberRole)]) -> Self {
            Self {
                server_id: ServerId::new(),
                roles: roles.to_vec(),
                audit: Arc::default(),
                events: Arc::default(),
                cleared_notifications: Arc::default(),
                cleared_automations: Arc::default(),
                conversation: None,
                message: None,
                conversation_member: true,
                attachments: Vec::new(),
                workspace_attachments: Vec::new(),
                operators: Vec::new(),
                direct_space: false,
                unknown_users: Vec::new(),
                author_counts: AuthorChannelCounts::default(),
                author_channels: Vec::new(),
                count_scopes: Arc::default(),
                include_flags: Arc::default(),
                bulk_deletes: Arc::default(),
                forgotten: Arc::default(),
            }
        }

        fn audited(&self) -> Vec<String> {
            self.audit
                .0
                .lock()
                .expect("lock")
                .iter()
                .map(|e| e.command_id.clone())
                .collect()
        }

        fn events(&self) -> Vec<WorkspaceEvent> {
            self.events.0.lock().expect("lock").clone()
        }

        fn cleared_notifications(&self) -> Vec<(UserId, ServerId)> {
            self.cleared_notifications.lock().expect("lock").clone()
        }

        fn cleared_automations(&self) -> Vec<Cleared> {
            self.cleared_automations.lock().expect("lock").clone()
        }

        fn service(&self, moderation_repo: MockModerationRepository) -> DefaultModerationService {
            let mut server_member_repo = MockServerMemberRepository::new();
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
                            joined_at: Utc::now(),
                        }))
                });
            server_member_repo
                .expect_remove_member()
                .returning(|_, _| Ok(()));
            server_member_repo
                .expect_change_member_role()
                .returning(|_, _, _| Ok(()));

            let mut user_repo = MockUserRepository::new();
            let unknown = self.unknown_users.clone();
            user_repo
                .expect_get_by_id()
                .returning(move |id| Ok((!unknown.contains(&id)).then(|| user(id))));
            user_repo.expect_get_by_ids().returning(|ids| {
                Ok(ids
                    .iter()
                    .map(|id| User {
                        principal_id: Some(principal_of(*id)),
                        ..user(*id)
                    })
                    .collect())
            });
            let operators = self.operators.clone();
            user_repo
                .expect_is_instance_operator()
                .returning(move |id| Ok(operators.contains(&id)));

            let mut conversation_repo = MockConversationRepository::new();
            conversation_repo
                .expect_list_joined_conversation_ids()
                .returning(|_, _| Box::pin(async { Ok(vec![]) }));
            let conversation = self.conversation.clone();
            conversation_repo.expect_get_by_id().returning(move |_| {
                let c = conversation.clone();
                Box::pin(async move { Ok(c) })
            });
            let conversation = self.conversation.clone();
            let author_conversations: Vec<Conversation> = self
                .author_channels
                .iter()
                .map(|(c, _)| c.clone())
                .collect();
            conversation_repo.expect_get_by_ids().returning(move |ids| {
                let found: Vec<Conversation> = conversation
                    .clone()
                    .into_iter()
                    .chain(
                        author_conversations
                            .iter()
                            .filter(|c| ids.contains(&c.id))
                            .cloned(),
                    )
                    .collect();
                Box::pin(async move { Ok(found) })
            });
            conversation_repo
                .expect_set_slow_mode()
                .returning(|id, seconds| {
                    Box::pin(async move {
                        let mut c = channel(ServerId::new());
                        c.id = id;
                        c.slow_mode_seconds = seconds;
                        Ok(c)
                    })
                });
            conversation_repo
                .expect_set_posting()
                .returning(|id, policy, allows_threads| {
                    Box::pin(async move {
                        let mut c = channel(ServerId::new());
                        c.id = id;
                        c.posting_policy = policy;
                        c.posting_allows_threads = allows_threads;
                        Ok(c)
                    })
                });

            let mut conversation_member_repo = MockConversationMemberRepository::new();
            conversation_member_repo
                .expect_list_member_ids()
                .returning(|_| Box::pin(async { Ok(vec![]) }));
            let is_member = self.conversation_member;
            conversation_member_repo
                .expect_is_member()
                .returning(move |_, _| Box::pin(async move { Ok(is_member) }));
            conversation_member_repo
                .expect_list_pending_conversation_ids()
                .returning(|_| Box::pin(async { Ok(vec![]) }));

            let mut message_attachment_repo = MockMessageAttachmentRepository::new();
            let attachments = self.attachments.clone();
            message_attachment_repo
                .expect_get_attachments_for_message()
                .returning(move |_| {
                    let a = attachments.clone();
                    Box::pin(async move { Ok(a) })
                });
            let stored = self.workspace_attachments.clone();
            message_attachment_repo
                .expect_find_stored_by_blob_sha256()
                .returning(move |_, hash| {
                    let found = stored
                        .iter()
                        .find(|a| a.blob_sha256.as_deref() == Some(hash.as_slice()))
                        .cloned();
                    Box::pin(async move { Ok(found) })
                });

            let mut message_repo = MockMessageRepository::new();
            let message = self.message.clone();
            message_repo.expect_get_by_id().returning(move |_| {
                let m = message.clone();
                Box::pin(async move { Ok(m) })
            });
            let message = self.message.clone();
            message_repo.expect_get_by_ids().returning(move |_| {
                let m = message.clone();
                Box::pin(async move { Ok(m.into_iter().map(|m| (m.id, m)).collect()) })
            });
            let counts = self.author_counts;
            let scopes = Arc::clone(&self.count_scopes);
            message_repo
                .expect_count_author_channel_messages()
                .returning(move |scope| {
                    scopes.lock().expect("lock").push(scope);
                    Box::pin(async move { Ok(counts) })
                });
            let channels: Vec<(ConversationId, bool)> = self
                .author_channels
                .iter()
                .map(|(c, _)| (c.id, c.is_private))
                .collect();
            let flags = Arc::clone(&self.include_flags);
            message_repo
                .expect_list_author_channels()
                .returning(move |_, include| {
                    flags.lock().expect("lock").push(include);
                    let ids: Vec<ConversationId> = channels
                        .iter()
                        .filter(|(_, private)| include || !private)
                        .map(|(id, _)| *id)
                        .collect();
                    Box::pin(async move { Ok(ids) })
                });
            let queues: Arc<Mutex<HashMap<ConversationId, Vec<DeletedMessage>>>> =
                Arc::new(Mutex::new(
                    self.author_channels
                        .iter()
                        .map(|(c, m)| (c.id, m.clone()))
                        .collect(),
                ));
            let bulk_deletes = Arc::clone(&self.bulk_deletes);
            message_repo
                .expect_soft_delete_author_channel_messages()
                .returning(move |id, _, _, limit| {
                    bulk_deletes.lock().expect("lock").push((id, limit));
                    let mut queues = queues.lock().expect("lock");
                    let queue = queues.entry(id).or_default();
                    let take = queue.len().min(limit as usize);
                    let deleted: Vec<DeletedMessage> = queue.drain(..take).collect();
                    Box::pin(async move { Ok(deleted) })
                });

            let roles = self.roles.clone();
            server_member_repo
                .expect_get_member_standing()
                .returning(move |_, uid| {
                    Ok(roles.iter().find(|(u, _)| *u == uid).map(|(_, role)| {
                        rim_core::server_member::model::MemberStanding::new(*role)
                    }))
                });

            let mut notification_repo = MockNotificationRepository::new();
            let forgotten = Arc::clone(&self.forgotten);
            notification_repo
                .expect_delete_unread_for_messages()
                .returning(move |ids| {
                    forgotten.lock().expect("lock").extend_from_slice(ids);
                    let n = ids.len() as u64;
                    Box::pin(async move { Ok(n) })
                });
            let cleared = Arc::clone(&self.cleared_notifications);
            notification_repo
                .expect_delete_for_user_in_server()
                .returning(move |user_id, server_id| {
                    cleared.lock().expect("lock").push((user_id, server_id));
                    Box::pin(async { Ok(1) })
                });

            let mut macro_repo = rim_core::macro_def::repository::MockMacroRepository::new();
            let cleared = Arc::clone(&self.cleared_automations);
            macro_repo
                .expect_delete_for_user()
                .returning(move |server_id, user_id| {
                    cleared
                        .lock()
                        .expect("lock")
                        .push(("macros", user_id, server_id));
                    Box::pin(async { Ok(1) })
                });
            let mut pipeline_repo = rim_core::pipeline::repository::MockPipelineRepository::new();
            let cleared = Arc::clone(&self.cleared_automations);
            pipeline_repo
                .expect_delete_for_user()
                .returning(move |server_id, user_id| {
                    cleared
                        .lock()
                        .expect("lock")
                        .push(("pipelines", user_id, server_id));
                    Box::pin(async { Ok(1) })
                });

            let mut public_join_link_repo = MockPublicJoinLinkRepository::new();
            public_join_link_repo
                .expect_cancel_open_requests()
                .returning(|_, _, _| Box::pin(async { Ok(Vec::new()) }));

            DefaultModerationService::new(
                ModerationRepos {
                    server_member_repo: Arc::new(server_member_repo),
                    user_repo: Arc::new(user_repo),
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    message_repo: Arc::new(message_repo),
                    message_attachment_repo: Arc::new(message_attachment_repo),
                    moderation_repo: Arc::new(moderation_repo),
                    notification_repo: Arc::new(notification_repo),
                    macro_repo: Arc::new(macro_repo),
                    pipeline_repo: Arc::new(pipeline_repo),
                    public_join_link_repo: Arc::new(public_join_link_repo),
                },
                Observers::new(Arc::clone(&self.events) as Arc<dyn Broadcaster>),
            )
            .with_audit_log_service(Arc::clone(&self.audit) as Arc<dyn AuditLogService>)
            .with_direct_space(self.direct_space.then_some(self.server_id))
        }
    }

    fn users<const N: usize>() -> [UserId; N] {
        std::array::from_fn(|_| UserId::new())
    }

    fn kick(fx: &Fx, actor: UserId, target: UserId) -> KickMemberCmd {
        KickMemberCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            user_id: target,
            reason: None,
        }
    }

    // ── kick ─────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn kick_role_matrix() {
        let [owner, mod_a, mod_b, member, guest, outsider] = users();
        let fx = Fx::new(&[
            (owner, Owner),
            (mod_a, Moderator),
            (mod_b, Moderator),
            (member, Member),
            (guest, Guest),
        ]);
        let svc = fx.service(MockModerationRepository::new());

        // (actor, target) => allowed
        let denied = [
            (mod_a, mod_b),
            (mod_a, owner),
            (member, guest),
            (guest, member),
        ];
        for (actor, target) in denied {
            let err = svc.kick(kick(&fx, actor, target)).await.unwrap_err();
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        }
        assert!(matches!(
            svc.kick(kick(&fx, owner, owner)).await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            svc.kick(kick(&fx, owner, outsider)).await,
            Err(ServiceError::NotFound)
        ));
        assert!(fx.audited().is_empty(), "refused kicks are not audited");
        assert!(
            fx.cleared_notifications().is_empty(),
            "refused kicks keep notifications"
        );
        assert!(
            fx.cleared_automations().is_empty(),
            "refused kicks keep macros and pipelines"
        );

        svc.kick(kick(&fx, mod_a, member))
            .await
            .expect("moderator kicks member");
        svc.kick(kick(&fx, owner, mod_b))
            .await
            .expect("owner kicks moderator");
        assert_eq!(fx.audited(), [audit_ids::KICK, audit_ids::KICK]);
        let left: Vec<_> = fx
            .events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MemberLeft { user_id, .. } => Some(user_id),
                _ => None,
            })
            .collect();
        assert_eq!(left, [member, mod_b]);
        assert_eq!(
            fx.cleared_notifications(),
            [(member, fx.server_id), (mod_b, fx.server_id)]
        );
        assert_eq!(
            fx.cleared_automations(),
            [
                ("macros", member, fx.server_id),
                ("pipelines", member, fx.server_id),
                ("macros", mod_b, fx.server_id),
                ("pipelines", mod_b, fx.server_id),
            ]
        );
    }

    // ── leave ────────────────────────────────────────────────────────────

    fn leave(fx: &Fx, actor: UserId) -> LeaveWorkspaceCmd {
        LeaveWorkspaceCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
        }
    }

    /// The `(user, cause, reason)` of every `MemberLeft` the fixture recorded.
    fn departures(fx: &Fx) -> Vec<(UserId, Option<String>, Option<String>)> {
        fx.events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MemberLeft {
                    user_id,
                    cause,
                    reason,
                    ..
                } => Some((user_id, cause, reason)),
                _ => None,
            })
            .collect()
    }

    /// A default-workspace service whose current default workspace is `current`.
    fn default_workspace_at(
        current: ServerId,
    ) -> rim_services::default_workspace::MockDefaultWorkspaceService {
        let mut default = rim_services::default_workspace::MockDefaultWorkspaceService::new();
        default
            .expect_current()
            .returning(move || Box::pin(async move { Ok(Some(current)) }));
        default
    }

    #[tokio::test]
    async fn a_member_or_a_guest_leaves_with_the_kick_cleanup_and_a_workspace_audit_row() {
        let [owner, member, guest] = users();
        let fx = Fx::new(&[(owner, Owner), (member, Member), (guest, Guest)]);
        let svc = fx.service(MockModerationRepository::new());

        svc.leave(leave(&fx, member))
            .await
            .expect("a member leaves");
        svc.leave(leave(&fx, guest)).await.expect("a guest leaves");

        assert_eq!(fx.audited(), [audit_ids::LEAVE, audit_ids::LEAVE]);
        let rows = fx.audit.0.lock().expect("lock").clone();
        assert!(rows.iter().all(|row| row.initiator == "workspace"));
        assert_eq!(rows[0].actor_user_id, uuid::Uuid::from(member));
        assert_eq!(rows[0].args_redacted["role"], "member");
        assert_eq!(rows[1].actor_user_id, uuid::Uuid::from(guest));
        assert_eq!(rows[1].args_redacted["role"], "guest");
        assert_eq!(
            departures(&fx),
            [
                (member, Some("left".to_string()), None),
                (guest, Some("left".to_string()), None),
            ]
        );
        assert_eq!(
            fx.cleared_notifications(),
            [(member, fx.server_id), (guest, fx.server_id)]
        );
        assert_eq!(
            fx.cleared_automations(),
            [
                ("macros", member, fx.server_id),
                ("pipelines", member, fx.server_id),
                ("macros", guest, fx.server_id),
                ("pipelines", guest, fx.server_id),
            ]
        );
    }

    #[tokio::test]
    async fn the_owner_an_outsider_the_direct_space_and_the_default_workspace_stay() {
        let [owner, member, outsider] = users();
        let fx = Fx::new(&[(owner, Owner), (member, Member)]);
        let svc = fx.service(MockModerationRepository::new());
        match svc.leave(leave(&fx, owner)).await {
            Err(ServiceError::Validation(m)) => assert_eq!(
                m,
                "the owner cannot leave a workspace; transfer ownership or delete the workspace first"
            ),
            other => panic!("the owner leaves: {other:?}"),
        }
        assert!(matches!(
            svc.leave(leave(&fx, outsider)).await,
            Err(ServiceError::PermissionDenied)
        ));

        let mut direct = Fx::new(&[(member, Member)]);
        direct.direct_space = true;
        let svc = direct.service(MockModerationRepository::new());
        match svc.leave(leave(&direct, member)).await {
            Err(ServiceError::Validation(m)) => {
                assert!(m.contains("not available in the direct space"), "{m}");
            }
            other => panic!("a member leaves the direct space: {other:?}"),
        }

        let defaulted = Fx::new(&[(member, Member)]);
        let svc = defaulted
            .service(MockModerationRepository::new())
            .with_default_workspace(Arc::new(default_workspace_at(defaulted.server_id)));
        match svc.leave(leave(&defaulted, member)).await {
            Err(ServiceError::Validation(m)) => assert_eq!(
                m,
                "you cannot leave this server's default workspace; every account here belongs to it"
            ),
            other => panic!("a member leaves the default workspace: {other:?}"),
        }

        for refused in [&fx, &direct, &defaulted] {
            assert!(
                refused.audited().is_empty(),
                "refused leaves are not audited"
            );
            assert!(
                departures(refused).is_empty(),
                "refused leaves announce nothing"
            );
            assert!(refused.cleared_notifications().is_empty());
            assert!(refused.cleared_automations().is_empty());
        }

        let elsewhere = Fx::new(&[(member, Member)]);
        let svc = elsewhere
            .service(MockModerationRepository::new())
            .with_default_workspace(Arc::new(default_workspace_at(ServerId::new())));
        svc.leave(leave(&elsewhere, member))
            .await
            .expect("another workspace is the default");
        assert_eq!(elsewhere.audited(), [audit_ids::LEAVE]);
    }

    // ── ban ──────────────────────────────────────────────────────────────

    #[tokio::test]
    async fn ban_records_removes_and_audits() {
        let [moderator, member] = users();
        let fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let mut repo = MockModerationRepository::new();
        repo.expect_upsert_ban()
            .withf(move |b| b.user_id == member && b.banned_by == Some(moderator))
            .times(1)
            .returning(|b| Box::pin(async move { Ok(b) }));
        let svc = fx.service(repo);

        let ban = svc
            .ban(BanMemberCmd {
                actor: Actor::new(moderator),
                server_id: fx.server_id,
                user_id: member,
                reason: Some("  spam  ".to_string()),
                expires_at: None,
            })
            .await
            .expect("ban");
        assert_eq!(ban.reason.as_deref(), Some("spam"));
        let removed = fx
            .events()
            .into_iter()
            .find(|event| matches!(event, WorkspaceEvent::MemberLeft { .. }))
            .unwrap();
        assert_eq!(
            removed.json_for_user(&member.to_string()).unwrap()["reason"],
            "spam"
        );
        assert!(
            removed
                .json_for_user(&moderator.to_string())
                .unwrap()
                .get("reason")
                .is_none()
        );
        assert_eq!(fx.audited(), [audit_ids::BAN]);
        let entry = fx.audit.0.lock().expect("lock")[0].clone();
        assert!(entry.dangerous);
        assert_eq!(entry.initiator, "moderation");
        assert!(
            !entry.args_redacted.to_string().contains("spam"),
            "the reason text never reaches the audit log"
        );
        assert!(fx.events().iter().any(
            |e| matches!(e, WorkspaceEvent::MemberLeft { user_id, .. } if *user_id == member)
        ));
        assert_eq!(fx.cleared_notifications(), [(member, fx.server_id)]);
        assert_eq!(
            fx.cleared_automations(),
            [
                ("macros", member, fx.server_id),
                ("pipelines", member, fx.server_id),
            ]
        );
    }

    #[tokio::test]
    async fn members_see_ban_status_but_only_moderators_see_the_reason() {
        let [moderator, member, banned] = users();
        let fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_ban().returning(move |sid, uid, _| {
            Box::pin(async move {
                Ok(Some(rim_core::moderation::model::ServerBan {
                    server_id: sid,
                    user_id: uid,
                    reason: Some("private".to_string()),
                    banned_by: Some(moderator),
                    created_at: Utc::now(),
                    expires_at: None,
                }))
            })
        });
        let service = fx.service(repo);
        let status = service
            .member_ban(Actor::new(member), fx.server_id, banned)
            .await
            .unwrap()
            .unwrap();
        assert!(status.reason.is_none());
        assert!(status.expires_at.is_none());
        assert_eq!(
            service
                .member_ban(Actor::new(moderator), fx.server_id, banned)
                .await
                .unwrap()
                .unwrap()
                .reason
                .as_deref(),
            Some("private")
        );
        assert!(
            service
                .member_ban(Actor::new(UserId::new()), fx.server_id, banned)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn unban_restores_only_the_current_default_workspace() {
        for is_default in [true, false] {
            let [moderator, target] = users();
            let fx = Fx::new(&[(moderator, Moderator)]);
            let workspace = fx.server_id;
            let mut repo = MockModerationRepository::new();
            repo.expect_delete_ban()
                .returning(|_, _| Box::pin(async { Ok(true) }));
            let mut default = rim_services::default_workspace::MockDefaultWorkspaceService::new();
            default.expect_current().returning(move || {
                Box::pin(async move {
                    Ok(Some(if is_default {
                        workspace
                    } else {
                        ServerId::new()
                    }))
                })
            });
            default
                .expect_join()
                .times(usize::from(is_default))
                .withf(move |uid| *uid == target)
                .returning(move |_| Box::pin(async move { Ok(Some(workspace)) }));
            let service = fx.service(repo).with_default_workspace(Arc::new(default));
            assert!(
                service
                    .unban(UnbanMemberCmd {
                        actor: Actor::new(moderator),
                        server_id: workspace,
                        user_id: target
                    })
                    .await
                    .unwrap()
            );
            assert_eq!(
                fx.audit.0.lock().unwrap()[0].args_redacted["restored_default_workspace"],
                is_default
            );
        }
    }

    #[tokio::test]
    async fn ban_rejects_past_expiry_and_protected_targets() {
        let [owner, moderator, other_mod] = users();
        let fx = Fx::new(&[
            (owner, Owner),
            (moderator, Moderator),
            (other_mod, Moderator),
        ]);
        let svc = fx.service(MockModerationRepository::new());
        let ban = |actor, target, expires_at| BanMemberCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            user_id: target,
            reason: None,
            expires_at,
        };
        assert!(matches!(
            svc.ban(ban(
                owner,
                moderator,
                Some(Utc::now() - Duration::minutes(1))
            ))
            .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            svc.ban(ban(moderator, other_mod, None)).await,
            Err(ServiceError::PermissionDenied)
        ));
        assert!(matches!(
            svc.ban(ban(moderator, owner, None)).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    #[tokio::test]
    async fn unban_audits_only_when_a_ban_existed() {
        let [moderator, user_a, user_b] = users();
        let fx = Fx::new(&[(moderator, Moderator)]);
        let mut repo = MockModerationRepository::new();
        repo.expect_delete_ban()
            .returning(move |_, uid| Box::pin(async move { Ok(uid == user_a) }));
        let svc = fx.service(repo);
        let unban = |target| UnbanMemberCmd {
            actor: Actor::new(moderator),
            server_id: fx.server_id,
            user_id: target,
        };
        assert!(svc.unban(unban(user_a)).await.expect("unban"));
        assert!(!svc.unban(unban(user_b)).await.expect("unban"));
        assert_eq!(fx.audited(), [audit_ids::UNBAN]);
    }

    // ── timeout ──────────────────────────────────────────────────────────

    #[tokio::test]
    async fn timeout_bounds_and_audit() {
        let [moderator, member] = users();
        let fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let mut repo = MockModerationRepository::new();
        repo.expect_upsert_timeout()
            .times(1)
            .returning(|t| Box::pin(async move { Ok(t) }));
        let svc = fx.service(repo);
        let timeout = |until| TimeoutMemberCmd {
            actor: Actor::new(moderator),
            server_id: fx.server_id,
            user_id: member,
            until,
            reason: None,
        };

        assert!(matches!(
            svc.timeout(timeout(Utc::now() - Duration::seconds(1)))
                .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            svc.timeout(timeout(Utc::now() + Duration::days(31))).await,
            Err(ServiceError::Validation(_))
        ));
        let until = Utc::now() + Duration::days(30) - Duration::minutes(1);
        let set = svc.timeout(timeout(until)).await.expect("timeout");
        assert_eq!(set.until, until);
        assert_eq!(set.set_by, Some(moderator));
        assert_eq!(fx.audited(), [audit_ids::TIMEOUT]);
    }

    #[tokio::test]
    async fn timeout_is_refused_for_moderator_on_moderator() {
        let [mod_a, mod_b] = users();
        let fx = Fx::new(&[(mod_a, Moderator), (mod_b, Moderator)]);
        let svc = fx.service(MockModerationRepository::new());
        let err = svc
            .timeout(TimeoutMemberCmd {
                actor: Actor::new(mod_a),
                server_id: fx.server_id,
                user_id: mod_b,
                until: Utc::now() + Duration::hours(1),
                reason: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn clear_timeout_audits_only_an_active_timeout() {
        let [owner, member] = users();
        let fx = Fx::new(&[(owner, Owner), (member, Member)]);
        let server_id = fx.server_id;
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_timeout()
            .returning(move |_, uid, _| {
                Box::pin(async move {
                    Ok(Some(MemberTimeout {
                        server_id,
                        user_id: uid,
                        until: Utc::now() + Duration::hours(1),
                        reason: None,
                        set_by: None,
                        created_at: Utc::now(),
                    }))
                })
            });
        repo.expect_delete_timeout()
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(true) }));
        let svc = fx.service(repo);
        assert!(
            svc.clear_timeout(ClearTimeoutCmd {
                actor: Actor::new(owner),
                server_id,
                user_id: member,
            })
            .await
            .expect("clear")
        );
        assert_eq!(fx.audited(), [audit_ids::CLEAR_TIMEOUT]);
    }

    // ── set_member_role ──────────────────────────────────────────────────

    #[tokio::test]
    async fn set_member_role_matrix_events_and_audit() {
        let [owner, moderator, member, guest] = users();
        let fx = Fx::new(&[
            (owner, Owner),
            (moderator, Moderator),
            (member, Member),
            (guest, Guest),
        ]);
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_timeout()
            .returning(|_, _, _| Box::pin(async { Ok(None) }));
        let svc = fx.service(repo);
        let set = |actor, target, role| SetMemberRoleCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            user_id: target,
            role,
        };

        let dto = svc
            .set_member_role(set(owner, member, Moderator))
            .await
            .expect("owner promotes");
        assert_eq!(dto.member_role, Moderator);
        svc.set_member_role(set(moderator, guest, Member))
            .await
            .expect("moderator lifts guest");
        for (actor, target, role) in [
            (moderator, member, Moderator),
            (moderator, moderator, Guest),
            (member, guest, Member),
            (guest, member, Guest),
        ] {
            let res = svc.set_member_role(set(actor, target, role)).await;
            assert!(
                matches!(
                    res,
                    Err(ServiceError::PermissionDenied) | Err(ServiceError::Validation(_))
                ),
                "{actor:?} -> {target:?} {role:?}: {res:?}"
            );
        }
        assert!(matches!(
            svc.set_member_role(set(owner, owner, Member)).await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            svc.set_member_role(set(owner, member, Owner)).await,
            Err(ServiceError::Validation(_))
        ));

        assert_eq!(
            fx.audited(),
            [audit_ids::SET_MEMBER_ROLE, audit_ids::SET_MEMBER_ROLE]
        );
        let changes: Vec<_> = fx
            .events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MemberRoleChanged { user_id, role, .. } => Some((user_id, role)),
                _ => None,
            })
            .collect();
        assert_eq!(
            changes,
            [
                (member, "moderator".to_string()),
                (guest, "member".to_string())
            ]
        );
    }

    // ── slow mode ────────────────────────────────────────────────────────

    #[tokio::test]
    async fn set_slow_mode_validates_and_audits() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.conversation = Some(channel(fx.server_id));
        let conversation_id = fx.conversation.as_ref().map(|c| c.id).expect("channel");
        let svc = fx.service(MockModerationRepository::new());
        let cmd = |actor, seconds| SetSlowModeCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            conversation_id,
            seconds,
        };

        for seconds in [-1, 21_601] {
            assert!(matches!(
                svc.set_slow_mode(cmd(moderator, seconds)).await,
                Err(ServiceError::Validation(_))
            ));
        }
        assert!(matches!(
            svc.set_slow_mode(cmd(member, 30)).await,
            Err(ServiceError::PermissionDenied)
        ));
        let dto = svc
            .set_slow_mode(cmd(moderator, 30))
            .await
            .expect("slow mode");
        assert_eq!(dto.slow_mode_seconds, 30);
        assert_eq!(fx.audited(), [audit_ids::SET_SLOW_MODE]);
        assert!(
            fx.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::ConversationUpdated { .. }))
        );
    }

    #[tokio::test]
    async fn set_slow_mode_is_channels_only() {
        let [moderator] = users();
        let mut fx = Fx::new(&[(moderator, Moderator)]);
        let mut group = channel(fx.server_id);
        group.conversation_type = ConversationType::Group;
        fx.conversation = Some(group.clone());
        let svc = fx.service(MockModerationRepository::new());
        let err = svc
            .set_slow_mode(SetSlowModeCmd {
                actor: Actor::new(moderator),
                server_id: fx.server_id,
                conversation_id: group.id,
                seconds: 10,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // ── channel posting ──────────────────────────────────────────────────

    #[tokio::test]
    async fn set_channel_posting_needs_a_moderator_and_audits() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.conversation = Some(channel(fx.server_id));
        let conversation_id = fx.conversation.as_ref().map(|c| c.id).expect("channel");
        let svc = fx.service(MockModerationRepository::new());
        let cmd = |actor, policy, allows_threads| SetChannelPostingCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            conversation_id,
            policy,
            allows_threads,
        };

        assert!(matches!(
            svc.set_channel_posting(cmd(member, PostingPolicy::Moderators, true))
                .await,
            Err(ServiceError::PermissionDenied)
        ));
        // The channel already lets everyone post: nothing is written or audited.
        let dto = svc
            .set_channel_posting(cmd(moderator, PostingPolicy::Everyone, true))
            .await
            .expect("unchanged");
        assert_eq!(dto.posting_policy, PostingPolicy::Everyone);
        assert!(fx.audited().is_empty());

        let dto = svc
            .set_channel_posting(cmd(moderator, PostingPolicy::Moderators, false))
            .await
            .expect("channel posting");
        assert_eq!(dto.posting_policy, PostingPolicy::Moderators);
        assert!(!dto.posting_allows_threads);
        assert_eq!(fx.audited(), [audit_ids::SET_CHANNEL_POSTING]);
        assert!(
            fx.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::ConversationUpdated { .. }))
        );
    }

    #[tokio::test]
    async fn set_channel_posting_is_channels_only_and_not_archived() {
        let [moderator] = users();
        let mut fx = Fx::new(&[(moderator, Moderator)]);
        let mut group = channel(fx.server_id);
        group.conversation_type = ConversationType::Group;
        let mut archived = channel(fx.server_id);
        archived.archived_at = Some(Utc::now());
        for (conversation, archived) in [(group, false), (archived, true)] {
            fx.conversation = Some(conversation.clone());
            let svc = fx.service(MockModerationRepository::new());
            let err = svc
                .set_channel_posting(SetChannelPostingCmd {
                    actor: Actor::new(moderator),
                    server_id: fx.server_id,
                    conversation_id: conversation.id,
                    policy: PostingPolicy::Moderators,
                    allows_threads: true,
                })
                .await
                .unwrap_err();
            if archived {
                assert!(matches!(err, ServiceError::ConversationArchived), "{err:?}");
            } else {
                assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
            }
        }
    }

    // ── reports ──────────────────────────────────────────────────────────

    fn message_in(conversation_id: ConversationId, author: UserId) -> Message {
        Message {
            id: MessageId::new(),
            conversation_id,
            thread_id: None,
            sent_by: author,
            text: Some("bad words".to_string()),
            has_attachments: false,
            encryption: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            is_deleted: false,
            sent_at: Utc::now(),
            edited_at: None,
            flagged_at: None,
        }
    }

    fn report_of(fx: &Fx, message: &Message, reporter: UserId) -> MessageReport {
        MessageReport {
            id: MessageReportId::new(),
            server_id: fx.server_id,
            conversation_id: message.conversation_id,
            message_id: message.id,
            reporter_user_id: Some(reporter),
            reason: "spam".to_string(),
            status: MessageReportStatus::Open,
            resolved_by: None,
            resolved_at: None,
            created_at: Utc::now(),
            franked: None,
        }
    }

    #[tokio::test]
    async fn report_message_once_per_reporter() {
        let [author, reporter] = users();
        let mut fx = Fx::new(&[(author, Member), (reporter, Member)]);
        let conversation = channel(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation.clone());
        fx.message = Some(message.clone());
        let first = report_of(&fx, &message, reporter);

        let mut repo = MockModerationRepository::new();
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        repo.expect_create_report().returning(move |_| {
            let mut n = counter.lock().expect("lock");
            *n += 1;
            let res = if *n == 1 {
                Ok(first.clone())
            } else {
                Err(RepoError::new(RepoErrorKind::Conflict))
            };
            Box::pin(async move { res })
        });
        let svc = fx.service(repo);
        let cmd = |reason: &str| ReportMessageCmd {
            actor: Actor::new(reporter),
            server_id: fx.server_id,
            conversation_id: conversation.id,
            message_id: message.id,
            reason: reason.to_string(),
        };

        assert!(matches!(
            svc.report_message(cmd("   ")).await,
            Err(ServiceError::Validation(_))
        ));
        let report = svc.report_message(cmd("spam")).await.expect("report");
        assert_eq!(report.status, MessageReportStatus::Open);
        assert!(matches!(
            svc.report_message(cmd("spam again")).await,
            Err(ServiceError::Conflict(_))
        ));
        assert_eq!(fx.audited(), [audit_ids::REPORT_MESSAGE]);
    }

    #[tokio::test]
    async fn resolve_report_requires_moderator_and_open_report() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = channel(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        let report = report_of(&fx, &message, member);
        let report_id = report.id;

        let mut repo = MockModerationRepository::new();
        let stored = report.clone();
        repo.expect_get_report().returning(move |_| {
            let r = stored.clone();
            Box::pin(async move { Ok(Some(r)) })
        });
        let resolved = MessageReport {
            status: MessageReportStatus::Dismissed,
            resolved_by: Some(moderator),
            resolved_at: Some(Utc::now()),
            ..report
        };
        let first = Arc::new(Mutex::new(true));
        repo.expect_resolve_report().returning(move |_, _, _, _| {
            let mut is_first = first.lock().expect("lock");
            let res = if *is_first {
                Ok(resolved.clone())
            } else {
                Err(RepoError::new(RepoErrorKind::PreconditionFailed))
            };
            *is_first = false;
            Box::pin(async move { res })
        });
        let svc = fx.service(repo);
        let cmd = |actor| ResolveReportCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            report_id,
            outcome: ReportOutcome::Dismissed,
        };

        assert!(matches!(
            svc.resolve_report(cmd(member)).await,
            Err(ServiceError::PermissionDenied)
        ));
        let dto = svc.resolve_report(cmd(moderator)).await.expect("resolve");
        assert_eq!(dto.status, MessageReportStatus::Dismissed);
        assert!(dto.message_readable, "public channel: body visible");
        assert_eq!(dto.message_text.as_deref(), Some("bad words"));
        assert!(matches!(
            svc.resolve_report(cmd(moderator)).await,
            Err(ServiceError::Conflict(_))
        ));
        assert_eq!(fx.audited(), [audit_ids::RESOLVE_REPORT]);
    }

    #[tokio::test]
    async fn active_timeout_reads_the_repository() {
        let [member] = users();
        let fx = Fx::new(&[(member, Member)]);
        let until = Utc::now() + Duration::minutes(5);
        let server_id = fx.server_id;
        let mut repo = MockModerationRepository::new();
        repo.expect_get_active_timeout()
            .returning(move |_, uid, _| {
                Box::pin(async move {
                    Ok(Some(MemberTimeout {
                        server_id,
                        user_id: uid,
                        until,
                        reason: None,
                        set_by: None,
                        created_at: Utc::now(),
                    }))
                })
            });
        let svc = fx.service(repo);
        assert_eq!(
            svc.active_timeout(server_id, member).await.expect("lookup"),
            Some(until)
        );
    }

    // ── Franked reports ─────────────────────────────────────────────────

    /// The verified content of appendix A.4, which names the A.3 attachment.
    const FRANKED_CONTENT: &str = r#"{"v":1,"type":"message","id":"0199d1a0-0000-7000-8000-000000000002","rev":0,"threadRootId":null,"body":"hi <@rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo>","mentions":["rimp1_nz1POml8JmCMcFDECrtu60ibohPKnywhcExmOPMvTUo"],"mentionAll":false,"replyTo":null,"forwardedFrom":null,"attachments":[{"blobSha256":"EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac","secret":"oKGio6SlpqeoqaqrrK2ur7CxsrO0tba3uLm6u7y9vr8","name":"hello.txt","mime":"text/plain","size":18}],"linkPreviews":[]}"#;

    /// SHA-256 of the A.3 blob.
    fn a3_blob_sha256() -> Vec<u8> {
        use base64::Engine;
        base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode("EHFcZ6hUOIdDi32ptdrCXM2PTHQwKF46-0Ls56yQrac")
            .expect("base64url")
    }

    fn encrypted_dm(server_id: ServerId) -> Conversation {
        Conversation {
            conversation_type: ConversationType::Direct,
            title: None,
            encrypted_at: Some(Utc::now()),
            ..channel(server_id)
        }
    }

    fn franked_report_of(fx: &Fx, message: &Message, reporter: UserId) -> MessageReport {
        MessageReport {
            franked: Some(FrankedReport {
                verified_at: Utc::now(),
                content: Some(FRANKED_CONTENT.as_bytes().to_vec()),
                sender_device_id: Some(uuid::Uuid::from_u128(1)),
            }),
            ..report_of(fx, message, reporter)
        }
    }

    fn attachment(message: &Message, blob_sha256: Vec<u8>, storage_url: &str) -> MessageAttachment {
        MessageAttachment {
            id: MessageAttachmentId::new(),
            storage_url: storage_url.to_string(),
            message_id: message.id,
            file_size: 34,
            file_name: "encrypted".to_string(),
            mime_type: "application/octet-stream".to_string(),
            metadata: None,
            blob_sha256: Some(blob_sha256),
        }
    }

    /// A moderation repository that holds one report.
    fn holding(report: &MessageReport) -> MockModerationRepository {
        let mut repo = MockModerationRepository::new();
        let stored = report.clone();
        repo.expect_get_report().returning(move |id| {
            let r = Some(stored.clone()).filter(|r| r.id == id);
            Box::pin(async move { Ok(r) })
        });
        repo
    }

    #[tokio::test]
    async fn a_moderator_outside_the_conversation_sees_a_verified_report() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = encrypted_dm(fx.server_id);
        let mut message = message_in(conversation.id, author);
        message.text = None;
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        let report = franked_report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));
        let get = |actor, server_id| GetReportQuery {
            actor: Actor::new(actor),
            server_id,
            report_id: report.id,
        };

        assert!(matches!(
            svc.get_report(get(member, fx.server_id)).await,
            Err(ServiceError::PermissionDenied)
        ));
        let other = ServerId::new();
        assert!(matches!(
            svc.get_report(get(moderator, other)).await,
            Err(ServiceError::PermissionDenied | ServiceError::NotFound)
        ));
        assert!(fx.audited().is_empty());

        let dto = svc
            .get_report(get(moderator, fx.server_id))
            .await
            .expect("report");
        assert!(!dto.message_readable);
        assert_eq!(dto.message_text, None);
        assert_eq!(dto.message_author_id, Some(author));
        assert_eq!(dto.message_sent_at, Some(message.sent_at));
        assert!(dto.franking_verified_at.is_some());
        assert_eq!(dto.franked_sender_device_id, Some(uuid::Uuid::from_u128(1)));
        assert_eq!(
            dto.franked_content.as_deref(),
            Some(FRANKED_CONTENT.as_bytes())
        );
        assert_eq!(dto.franked_content_expires_at, None, "open: kept");
        assert_eq!(fx.audited(), [audit_ids::REPORT_VIEWED]);
        let entry = fx.audit.0.lock().expect("lock")[0].clone();
        assert_eq!(entry.initiator, "e2ee");
        assert_eq!(
            entry.args_redacted,
            serde_json::json!({
                "report_id": report.id.to_string(),
                "conversation_id": message.conversation_id.to_string(),
                "message_id": message.id.to_string(),
            })
        );
    }

    #[tokio::test]
    async fn lists_leave_out_the_verified_content_and_resolution_starts_its_retention() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = encrypted_dm(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        let report = franked_report_of(&fx, &message, member);
        let resolved_at = Utc::now();
        let resolved = MessageReport {
            status: MessageReportStatus::Resolved,
            resolved_by: Some(moderator),
            resolved_at: Some(resolved_at),
            ..report.clone()
        };
        let mut repo = holding(&report);
        repo.expect_resolve_report().returning(move |_, _, _, _| {
            let r = resolved.clone();
            Box::pin(async move { Ok(r) })
        });
        let svc = fx.service(repo);

        let dto = svc
            .resolve_report(ResolveReportCmd {
                actor: Actor::new(moderator),
                server_id: fx.server_id,
                report_id: report.id,
                outcome: ReportOutcome::Resolved,
            })
            .await
            .expect("resolve");
        assert_eq!(dto.franked_content, None);
        assert_eq!(dto.message_author_id, Some(author));
        assert!(dto.franking_verified_at.is_some());
        assert_eq!(
            dto.franked_content_expires_at,
            Some(resolved_at + Duration::days(90))
        );
        assert_eq!(fx.audited(), [audit_ids::RESOLVE_REPORT]);
    }

    #[tokio::test]
    async fn a_plaintext_report_is_not_audited_as_viewed() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = channel(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        let report = report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));

        let dto = svc
            .get_report(GetReportQuery {
                actor: Actor::new(moderator),
                server_id: fx.server_id,
                report_id: report.id,
            })
            .await
            .expect("report");
        assert_eq!(dto.message_text.as_deref(), Some("bad words"));
        assert_eq!(dto.franking_verified_at, None);
        assert_eq!(dto.franked_content, None);
        assert!(fx.audited().is_empty());
    }

    #[tokio::test]
    async fn a_reported_attachment_is_served_only_to_moderators_and_only_when_named() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = encrypted_dm(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        fx.attachments = vec![
            attachment(&message, vec![7; 32], "uploads/own"),
            attachment(&message, vec![8; 32], "pending://upload"),
        ];
        let elsewhere = message_in(message.conversation_id, author);
        fx.workspace_attachments = vec![attachment(&elsewhere, a3_blob_sha256(), "uploads/a3")];
        let report = franked_report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));
        let ask = |actor, blob_sha256| AuthorizeReportAttachmentQuery {
            actor_user_id: actor,
            report_id: report.id,
            blob_sha256,
        };

        let own = svc
            .authorize_report_attachment(ask(moderator, vec![7; 32]))
            .await
            .expect("own attachment");
        assert_eq!(own.storage_key, "uploads/own");
        let named = svc
            .authorize_report_attachment(ask(moderator, a3_blob_sha256()))
            .await
            .expect("named by the verified content");
        assert_eq!(named.storage_key, "uploads/a3");
        for (actor, blob) in [
            (member, vec![7; 32]),
            (author, a3_blob_sha256()),
            (moderator, vec![8; 32]),
            (moderator, vec![9; 32]),
        ] {
            assert!(matches!(
                svc.authorize_report_attachment(ask(actor, blob)).await,
                Err(ServiceError::NotFound)
            ));
        }
    }

    #[tokio::test]
    async fn a_plaintext_report_serves_no_attachment() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = channel(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.attachments = vec![attachment(&message, vec![7; 32], "uploads/own")];
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        let report = report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));
        assert!(matches!(
            svc.authorize_report_attachment(AuthorizeReportAttachmentQuery {
                actor_user_id: moderator,
                report_id: MessageReportId::new(),
                blob_sha256: vec![7; 32],
            })
            .await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            svc.authorize_report_attachment(AuthorizeReportAttachmentQuery {
                actor_user_id: moderator,
                report_id: report.id,
                blob_sha256: vec![7; 32],
            })
            .await,
            Err(ServiceError::NotFound)
        ));
    }

    #[test]
    fn content_blob_hashes_reads_the_attachment_entries() {
        assert_eq!(
            crate::moderation::content_blob_hashes(FRANKED_CONTENT.as_bytes()),
            vec![a3_blob_sha256()]
        );
        assert!(crate::moderation::content_blob_hashes(b"not json").is_empty());
        assert!(
            crate::moderation::content_blob_hashes(br#"{"attachments":[{"blobSha256":"!"}]}"#)
                .is_empty()
        );
    }

    // ── The direct space ────────────────────────────────────────────────

    #[tokio::test]
    async fn an_operator_handles_the_reports_of_the_direct_space_and_nothing_else() {
        let [operator, member, author] = users();
        let mut fx = Fx::new(&[(member, Guest), (author, Guest)]);
        fx.direct_space = true;
        fx.operators = vec![operator];
        let conversation = encrypted_dm(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        fx.attachments = vec![attachment(&message, vec![7; 32], "uploads/own")];
        let report = franked_report_of(&fx, &message, member);
        let resolved = MessageReport {
            status: MessageReportStatus::Resolved,
            resolved_by: Some(operator),
            resolved_at: Some(Utc::now()),
            ..report.clone()
        };
        let mut repo = holding(&report);
        let listed = report.clone();
        repo.expect_list_reports().returning(move |_, _, _, _, _| {
            let items = vec![listed.clone()];
            Box::pin(async move {
                Ok(rim_core::repository::pagination::Page::new(
                    items, None, None,
                ))
            })
        });
        repo.expect_resolve_report().returning(move |_, _, _, _| {
            let r = resolved.clone();
            Box::pin(async move { Ok(r) })
        });
        let svc = fx.service(repo);
        let list = |actor| rim_services::moderation::command::ListReportsQuery {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            status: None,
            limit: 20,
            dir: rim_core::repository::pagination::PageDir::After,
            cursor: None,
        };
        let get = |actor| GetReportQuery {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            report_id: report.id,
        };

        // Everyone in the direct space is a guest: no one there handles its reports.
        assert!(matches!(
            svc.list_reports(list(member)).await,
            Err(ServiceError::Forbidden(_))
        ));
        assert!(matches!(
            svc.get_report(get(author)).await,
            Err(ServiceError::Forbidden(_))
        ));
        assert!(matches!(
            svc.authorize_report_attachment(AuthorizeReportAttachmentQuery {
                actor_user_id: member,
                report_id: report.id,
                blob_sha256: vec![7; 32],
            })
            .await,
            Err(ServiceError::NotFound)
        ));
        assert!(fx.audited().is_empty());

        // The operator lists, opens (audited), downloads and resolves it.
        let page = svc.list_reports(list(operator)).await.expect("list");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].franked_content, None);
        let dto = svc.get_report(get(operator)).await.expect("report");
        assert_eq!(
            dto.franked_content.as_deref(),
            Some(FRANKED_CONTENT.as_bytes())
        );
        assert_eq!(dto.message_author_id, Some(author));
        assert_eq!(dto.message_author_principal_id, Some(principal_of(author)));
        let served = svc
            .authorize_report_attachment(AuthorizeReportAttachmentQuery {
                actor_user_id: operator,
                report_id: report.id,
                blob_sha256: vec![7; 32],
            })
            .await
            .expect("served");
        assert_eq!(served.storage_key, "uploads/own");
        svc.resolve_report(ResolveReportCmd {
            actor: Actor::new(operator),
            server_id: fx.server_id,
            report_id: report.id,
            outcome: ReportOutcome::Resolved,
        })
        .await
        .expect("resolve");
        assert_eq!(
            fx.audited(),
            [audit_ids::REPORT_VIEWED, audit_ids::RESOLVE_REPORT]
        );

        // No other moderation: the operator kicks no one there.
        assert!(matches!(
            svc.kick(kick(&fx, operator, author)).await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    /// The report of `message`, without franking, as `operator` sees it in a list and opened,
    /// in a DM of the direct space (or of a workspace, when not `direct_space`) they are not in.
    async fn unfranked_report_seen_by_operator(
        message_encrypted: bool,
        direct_space: bool,
    ) -> (
        rim_services::moderation::dto::MessageReportDto,
        rim_services::moderation::dto::MessageReportDto,
        UserId,
    ) {
        let [operator, member, author] = users();
        let mut fx = Fx::new(&[(member, Guest), (author, Guest)]);
        fx.direct_space = direct_space;
        fx.operators = vec![operator];
        let conversation = Conversation {
            conversation_type: ConversationType::Direct,
            title: None,
            is_private: true,
            ..channel(fx.server_id)
        };
        let mut message = message_in(conversation.id, author);
        if message_encrypted {
            message.text = None;
            message.encryption = Some(rim_core::message::model::MessageEncryption {
                incarnation: 1,
                epoch: 1,
                sender_device_key_id: None,
                ciphertext: Some(vec![1]),
                franking_commitment: vec![2],
                franking_receipt: vec![3],
                franking_key_id: 1,
            });
        }
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        let report = report_of(&fx, &message, member);
        let mut repo = holding(&report);
        let listed = report.clone();
        repo.expect_list_reports().returning(move |_, _, _, _, _| {
            let items = vec![listed.clone()];
            Box::pin(async move {
                Ok(rim_core::repository::pagination::Page::new(
                    items, None, None,
                ))
            })
        });
        let svc = fx.service(repo);
        let mut page = svc
            .list_reports(rim_services::moderation::command::ListReportsQuery {
                actor: Actor::new(operator),
                server_id: fx.server_id,
                status: None,
                limit: 20,
                dir: rim_core::repository::pagination::PageDir::After,
                cursor: None,
            })
            .await
            .expect("list");
        let opened = svc
            .get_report(GetReportQuery {
                actor: Actor::new(operator),
                server_id: fx.server_id,
                report_id: report.id,
            })
            .await
            .expect("report");
        (page.items.pop().expect("one report"), opened, author)
    }

    #[tokio::test]
    async fn an_operator_reads_the_reported_unencrypted_message_of_the_direct_space() {
        let (listed, opened, author) = unfranked_report_seen_by_operator(false, true).await;
        for dto in [listed, opened] {
            assert!(!dto.message_readable, "the conversation stays unreadable");
            assert_eq!(dto.message_text.as_deref(), Some("bad words"));
            assert_eq!(dto.message_author_id, Some(author));
            assert_eq!(dto.message_author_principal_id, Some(principal_of(author)));
            assert!(dto.message_sent_at.is_some());
            assert!(dto.franking_verified_at.is_none());
        }
    }

    #[tokio::test]
    async fn an_operator_sees_nothing_of_an_unfranked_encrypted_message() {
        let (listed, opened, _) = unfranked_report_seen_by_operator(true, true).await;
        for dto in [listed, opened] {
            assert_eq!(dto.message_text, None);
            assert_eq!(dto.message_author_id, None);
            assert_eq!(dto.message_author_principal_id, None);
            assert_eq!(dto.message_sent_at, None);
        }
    }

    #[tokio::test]
    async fn a_workspace_moderator_outside_a_dm_sees_no_unfranked_message() {
        let [moderator, member, author] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member), (author, Member)]);
        let conversation = Conversation {
            conversation_type: ConversationType::Direct,
            title: None,
            is_private: true,
            ..channel(fx.server_id)
        };
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        fx.conversation_member = false;
        let report = report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));
        let get = || GetReportQuery {
            actor: Actor::new(moderator),
            server_id: fx.server_id,
            report_id: report.id,
        };
        let dto = svc.get_report(get()).await.expect("report");
        assert_eq!(dto.message_text, None);
        assert_eq!(dto.message_author_id, None);
        assert_eq!(dto.message_author_principal_id, None);

        // A moderator who reads the conversation sees the author, but no principal id.
        fx.conversation_member = true;
        let svc = fx.service(holding(&report));
        let dto = svc.get_report(get()).await.expect("report");
        assert_eq!(dto.message_text.as_deref(), Some("bad words"));
        assert_eq!(dto.message_author_id, Some(author));
        assert_eq!(dto.message_author_principal_id, None);
    }

    #[tokio::test]
    async fn an_operator_handles_no_reports_of_another_workspace() {
        let [operator, member, author] = users();
        let mut fx = Fx::new(&[(member, Member), (author, Member)]);
        fx.operators = vec![operator];
        let conversation = encrypted_dm(fx.server_id);
        let message = message_in(conversation.id, author);
        fx.conversation = Some(conversation);
        fx.message = Some(message.clone());
        let report = franked_report_of(&fx, &message, member);
        let svc = fx.service(holding(&report));
        assert!(matches!(
            svc.get_report(GetReportQuery {
                actor: Actor::new(operator),
                server_id: fx.server_id,
                report_id: report.id,
            })
            .await,
            Err(ServiceError::PermissionDenied)
        ));
    }

    // ── delete member messages ───────────────────────────────────────────

    fn counts_query(fx: &Fx, actor: UserId, target: UserId) -> MemberMessageCountsQuery {
        MemberMessageCountsQuery {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            user_id: target,
            since: None,
        }
    }

    fn delete_cmd(
        fx: &Fx,
        actor: UserId,
        target: UserId,
        include: bool,
    ) -> DeleteMemberMessagesCmd {
        DeleteMemberMessagesCmd {
            actor: Actor::new(actor),
            server_id: fx.server_id,
            user_id: target,
            since: None,
            include_unjoined_private: include,
        }
    }

    /// `n` deleted-message records with no thread.
    fn deleted_messages(n: usize) -> Vec<DeletedMessage> {
        (0..n)
            .map(|_| DeletedMessage {
                id: MessageId::new(),
                thread_id: None,
            })
            .collect()
    }

    fn private_channel(server_id: ServerId) -> Conversation {
        Conversation {
            is_private: true,
            title: Some("staff".to_string()),
            ..channel(server_id)
        }
    }

    /// The `MessagesDeleted` events the fixture recorded, as `(channel, ids, threads, private)`.
    type BulkEvent = (ConversationId, Vec<MessageId>, Vec<MessageThreadId>, bool);

    fn bulk_events(fx: &Fx) -> Vec<BulkEvent> {
        fx.events()
            .into_iter()
            .filter_map(|e| match e {
                WorkspaceEvent::MessagesDeleted {
                    conversation_id,
                    message_ids,
                    thread_ids,
                    is_private,
                    conversation_type,
                    ..
                } => {
                    assert_eq!(conversation_type, "Channel");
                    Some((conversation_id, message_ids, thread_ids, is_private))
                }
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn delete_member_messages_follows_the_kick_target_matrix() {
        let [owner, mod_a, mod_b, member, guest] = users();
        let fx = Fx::new(&[
            (owner, Owner),
            (mod_a, Moderator),
            (mod_b, Moderator),
            (member, Member),
            (guest, Guest),
        ]);
        let svc = fx.service(MockModerationRepository::new());

        let denied = [
            (mod_a, mod_b),
            (mod_a, owner),
            (mod_b, owner),
            (member, guest),
            (guest, member),
        ];
        for (actor, target) in denied {
            let err = svc
                .member_message_counts(counts_query(&fx, actor, target))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
            let err = svc
                .delete_member_messages(delete_cmd(&fx, actor, target, false))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::PermissionDenied), "{err:?}");
        }
        for actor in [owner, mod_a] {
            assert!(matches!(
                svc.member_message_counts(counts_query(&fx, actor, actor))
                    .await,
                Err(ServiceError::Validation(_))
            ));
            assert!(matches!(
                svc.delete_member_messages(delete_cmd(&fx, actor, actor, false))
                    .await,
                Err(ServiceError::Validation(_))
            ));
        }
        assert!(fx.include_flags.lock().expect("lock").is_empty());

        for (actor, target) in [(owner, mod_b), (mod_a, member), (mod_a, guest)] {
            svc.member_message_counts(counts_query(&fx, actor, target))
                .await
                .expect("allowed count");
            svc.delete_member_messages(delete_cmd(&fx, actor, target, false))
                .await
                .expect("allowed delete");
        }
    }

    #[tokio::test]
    async fn delete_member_messages_reaches_a_former_member_but_not_an_unknown_user() {
        let [owner, moderator, former, unknown] = users();
        let mut fx = Fx::new(&[(owner, Owner), (moderator, Moderator)]);
        fx.unknown_users = vec![unknown];
        let general = channel(fx.server_id);
        fx.author_channels = vec![(general, deleted_messages(2))];
        let svc = fx.service(MockModerationRepository::new());

        assert!(matches!(
            svc.member_message_counts(counts_query(&fx, moderator, unknown))
                .await,
            Err(ServiceError::NotFound)
        ));
        assert!(matches!(
            svc.delete_member_messages(delete_cmd(&fx, moderator, unknown, false))
                .await,
            Err(ServiceError::NotFound)
        ));
        assert!(fx.bulk_deletes.lock().expect("lock").is_empty());

        svc.member_message_counts(counts_query(&fx, moderator, former))
            .await
            .expect("a former member is counted");
        let outcome = svc
            .delete_member_messages(delete_cmd(&fx, moderator, former, false))
            .await
            .expect("a former member's messages are deleted");
        assert_eq!(outcome.deleted, 2);
        let rows = fx.audit.0.lock().expect("lock").clone();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].args_redacted["was_member"], false);
    }

    #[tokio::test]
    async fn delete_member_messages_refuses_the_direct_space() {
        let [owner, member] = users();
        let mut fx = Fx::new(&[(owner, Owner), (member, Member)]);
        fx.direct_space = true;
        let svc = fx.service(MockModerationRepository::new());

        assert!(matches!(
            svc.member_message_counts(counts_query(&fx, owner, member))
                .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(matches!(
            svc.delete_member_messages(delete_cmd(&fx, owner, member, true))
                .await,
            Err(ServiceError::Validation(_))
        ));
        assert!(fx.count_scopes.lock().expect("lock").is_empty());
        assert!(fx.include_flags.lock().expect("lock").is_empty());
    }

    #[tokio::test]
    async fn member_message_counts_passes_the_caller_as_viewer_and_since() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.author_counts = AuthorChannelCounts {
            readable: MessageTally {
                messages: 7,
                channels: 2,
            },
            unjoined_private: MessageTally {
                messages: 3,
                channels: 1,
            },
        };
        let svc = fx.service(MockModerationRepository::new());
        let since = Utc::now() - Duration::hours(1);

        let counts = svc
            .member_message_counts(MemberMessageCountsQuery {
                since: Some(since),
                ..counts_query(&fx, moderator, member)
            })
            .await
            .expect("counts");
        assert_eq!((counts.readable.messages, counts.readable.channels), (7, 2));
        assert_eq!(
            (
                counts.unjoined_private.messages,
                counts.unjoined_private.channels
            ),
            (3, 1)
        );
        assert_eq!(
            *fx.count_scopes.lock().expect("lock"),
            [AuthorChannelScope {
                server_id: fx.server_id,
                author: member,
                since: Some(since),
                viewer: moderator,
            }]
        );
        assert!(fx.audited().is_empty(), "a count is not audited");
    }

    #[tokio::test]
    async fn delete_member_messages_leaves_unjoined_private_channels_unless_asked() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let general = channel(fx.server_id);
        let staff = private_channel(fx.server_id);
        fx.author_channels = vec![
            (general.clone(), deleted_messages(2)),
            (staff.clone(), deleted_messages(1)),
        ];
        let svc = fx.service(MockModerationRepository::new());

        let first = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");
        assert_eq!(
            (first.deleted, first.channels, first.has_more),
            (2, 1, false)
        );
        let second = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, true))
            .await
            .expect("delete with private channels");
        assert_eq!((second.deleted, second.channels), (1, 1));

        assert_eq!(*fx.include_flags.lock().expect("lock"), [false, true]);
        let touched: Vec<ConversationId> = fx
            .bulk_deletes
            .lock()
            .expect("lock")
            .iter()
            .map(|(id, _)| *id)
            .collect();
        assert_eq!(touched, [general.id, general.id, staff.id]);
        let events = bulk_events(&fx);
        assert_eq!(events.len(), 2);
        assert_eq!((events[0].0, events[0].3), (general.id, false));
        assert_eq!((events[1].0, events[1].3), (staff.id, true));
    }

    #[tokio::test]
    async fn delete_member_messages_stops_at_the_cap_and_reports_more() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let a = channel(fx.server_id);
        let b = channel(fx.server_id);
        let c = channel(fx.server_id);
        fx.author_channels = vec![
            (a.clone(), deleted_messages(600)),
            (b.clone(), deleted_messages(600)),
            (c.clone(), deleted_messages(5)),
        ];
        fx.author_counts = AuthorChannelCounts {
            readable: MessageTally {
                messages: 205,
                channels: 2,
            },
            unjoined_private: MessageTally::default(),
        };
        let svc = fx.service(MockModerationRepository::new());

        let outcome = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");
        assert_eq!(
            (outcome.deleted, outcome.channels, outcome.has_more),
            (1000, 2, true)
        );
        assert_eq!(
            *fx.bulk_deletes.lock().expect("lock"),
            [(a.id, 1000), (b.id, 400)]
        );
        assert_eq!(
            fx.count_scopes.lock().expect("lock").len(),
            1,
            "the cap triggers one recount"
        );
        let rows = fx.audit.0.lock().expect("lock").clone();
        assert_eq!(rows[0].args_redacted["has_more"], true);

        // Below the cap there is no recount and nothing more to report.
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.author_channels = vec![(channel(fx.server_id), deleted_messages(3))];
        fx.author_counts = AuthorChannelCounts {
            readable: MessageTally {
                messages: 9,
                channels: 1,
            },
            unjoined_private: MessageTally::default(),
        };
        let svc = fx.service(MockModerationRepository::new());
        let outcome = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");
        assert_eq!((outcome.deleted, outcome.has_more), (3, false));
        assert!(fx.count_scopes.lock().expect("lock").is_empty());

        // At the cap, private channels the actor is not in count only when asked for.
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.author_channels = vec![(channel(fx.server_id), deleted_messages(1000))];
        fx.author_counts = AuthorChannelCounts {
            readable: MessageTally::default(),
            unjoined_private: MessageTally {
                messages: 4,
                channels: 1,
            },
        };
        let svc = fx.service(MockModerationRepository::new());
        let outcome = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");
        assert_eq!((outcome.deleted, outcome.has_more), (1000, false));
    }

    #[tokio::test]
    async fn delete_member_messages_emits_one_event_per_channel_with_distinct_thread_ids() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let general = channel(fx.server_id);
        let random = channel(fx.server_id);
        let [t1, t2] = [MessageThreadId::new(), MessageThreadId::new()];
        let in_general = vec![
            DeletedMessage {
                id: MessageId::new(),
                thread_id: Some(t1),
            },
            DeletedMessage {
                id: MessageId::new(),
                thread_id: None,
            },
            DeletedMessage {
                id: MessageId::new(),
                thread_id: Some(t1),
            },
            DeletedMessage {
                id: MessageId::new(),
                thread_id: Some(t2),
            },
        ];
        let in_random = deleted_messages(1);
        fx.author_channels = vec![
            (general.clone(), in_general.clone()),
            (random.clone(), in_random.clone()),
        ];
        let svc = fx.service(MockModerationRepository::new());

        svc.delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");

        let events = bulk_events(&fx);
        assert_eq!(events.len(), 2);
        let (id, ids, threads, _) = &events[0];
        assert_eq!(*id, general.id);
        assert_eq!(*ids, in_general.iter().map(|m| m.id).collect::<Vec<_>>());
        let mut expected = vec![t1, t2];
        expected.sort_by_key(|t| uuid::Uuid::from(*t));
        assert_eq!(*threads, expected);
        let (id, ids, threads, _) = &events[1];
        assert_eq!(*id, random.id);
        assert_eq!(*ids, [in_random[0].id]);
        assert!(threads.is_empty());
        assert!(
            !fx.events()
                .iter()
                .any(|e| matches!(e, WorkspaceEvent::MessageDeleted { .. })),
            "a bulk delete sends no per-message event"
        );
    }

    #[tokio::test]
    async fn delete_member_messages_forgets_unread_notifications() {
        let [moderator, member] = users();
        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let general = deleted_messages(2);
        let random = deleted_messages(1);
        fx.author_channels = vec![
            (channel(fx.server_id), general.clone()),
            (channel(fx.server_id), random.clone()),
        ];
        let svc = fx.service(MockModerationRepository::new());

        svc.delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("delete");

        let expected: Vec<MessageId> = general.iter().chain(&random).map(|m| m.id).collect();
        assert_eq!(*fx.forgotten.lock().expect("lock"), expected);
    }

    #[tokio::test]
    async fn delete_member_messages_audits_counts_without_text_and_nothing_when_none_deleted() {
        let [moderator, member] = users();
        let fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        let svc = fx.service(MockModerationRepository::new());
        let outcome = svc
            .delete_member_messages(delete_cmd(&fx, moderator, member, false))
            .await
            .expect("nothing to delete");
        assert_eq!(
            (outcome.deleted, outcome.channels, outcome.has_more),
            (0, 0, false)
        );
        assert!(fx.audited().is_empty(), "nothing deleted, nothing audited");
        assert!(bulk_events(&fx).is_empty());

        let mut fx = Fx::new(&[(moderator, Moderator), (member, Member)]);
        fx.author_channels = vec![
            (channel(fx.server_id), deleted_messages(2)),
            (private_channel(fx.server_id), deleted_messages(3)),
        ];
        let svc = fx.service(MockModerationRepository::new());
        let since = Utc::now() - Duration::days(7);
        svc.delete_member_messages(DeleteMemberMessagesCmd {
            since: Some(since),
            ..delete_cmd(&fx, moderator, member, true)
        })
        .await
        .expect("delete");

        let rows = fx.audit.0.lock().expect("lock").clone();
        assert_eq!(rows.len(), 1);
        let row = &rows[0];
        assert_eq!(row.command_id, audit_ids::DELETE_MEMBER_MESSAGES);
        assert_eq!(row.actor_user_id, uuid::Uuid::from(moderator));
        assert!(row.dangerous);
        assert_eq!(
            row.typed_verb,
            Some(rim_services::command::verb::TypedVerb::Delete)
        );
        assert_eq!(
            row.args_redacted,
            serde_json::json!({
                "user_id": member.to_string(),
                "since": since.to_rfc3339(),
                "include_unjoined_private": true,
                "deleted": 5,
                "channels": 2,
                "has_more": false,
                "was_member": true,
                "complete": true,
            })
        );
    }
}
