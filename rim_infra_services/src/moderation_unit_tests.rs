#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use async_trait::async_trait;
    use chrono::{Duration, Utc};

    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::message::model::{Message, MessageId};
    use rim_core::message::repository::MockMessageRepository;
    use rim_core::message_attachment::model::{MessageAttachment, MessageAttachmentId};
    use rim_core::message_attachment::repository::MockMessageAttachmentRepository;
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
        AuthorizeReportAttachmentQuery, BanMemberCmd, ClearTimeoutCmd, GetReportQuery,
        KickMemberCmd, ReportMessageCmd, ReportOutcome, ResolveReportCmd, SetMemberRoleCmd,
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
            password_hash: Some(String::new()),
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
            user_repo
                .expect_get_by_id()
                .returning(|id| Ok(Some(user(id))));
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
            conversation_repo.expect_get_by_ids().returning(move |_| {
                let c = conversation.clone();
                Box::pin(async move { Ok(c.into_iter().collect()) })
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

            let roles = self.roles.clone();
            server_member_repo
                .expect_get_member_standing()
                .returning(move |_, uid| {
                    Ok(roles.iter().find(|(u, _)| *u == uid).map(|(_, role)| {
                        rim_core::server_member::model::MemberStanding::new(*role)
                    }))
                });

            let mut notification_repo = MockNotificationRepository::new();
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
        }
    }

    fn report_of(fx: &Fx, message: &Message, reporter: UserId) -> MessageReport {
        MessageReport {
            id: MessageReportId::new(),
            server_id: fx.server_id,
            conversation_id: message.conversation_id,
            message_id: message.id,
            reporter_user_id: reporter,
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
}
