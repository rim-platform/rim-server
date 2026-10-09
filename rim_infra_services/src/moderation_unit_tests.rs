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
    use rim_core::moderation::model::{
        MemberTimeout, MessageReport, MessageReportId, MessageReportStatus,
    };
    use rim_core::moderation::repository::MockModerationRepository;
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
        BanMemberCmd, ClearTimeoutCmd, KickMemberCmd, ReportMessageCmd, ReportOutcome,
        ResolveReportCmd, SetMemberRoleCmd, SetSlowModeCmd, TimeoutMemberCmd, UnbanMemberCmd,
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
        }
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

    struct Fx {
        server_id: ServerId,
        roles: Vec<(UserId, ServerMemberRole)>,
        audit: Arc<RecordingAudit>,
        events: Arc<RecordingBroadcaster>,
        conversation: Option<Conversation>,
        message: Option<Message>,
    }

    impl Fx {
        fn new(roles: &[(UserId, ServerMemberRole)]) -> Self {
            Self {
                server_id: ServerId::new(),
                roles: roles.to_vec(),
                audit: Arc::default(),
                events: Arc::default(),
                conversation: None,
                message: None,
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
            conversation_member_repo
                .expect_is_member()
                .returning(|_, _| Box::pin(async { Ok(true) }));

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

            DefaultModerationService::new(
                ModerationRepos {
                    server_member_repo: Arc::new(server_member_repo),
                    user_repo: Arc::new(user_repo),
                    conversation_repo: Arc::new(conversation_repo),
                    conversation_member_repo: Arc::new(conversation_member_repo),
                    message_repo: Arc::new(message_repo),
                    moderation_repo: Arc::new(moderation_repo),
                },
                Observers::new(Arc::clone(&self.events) as Arc<dyn Broadcaster>),
            )
            .with_audit_log_service(Arc::clone(&self.audit) as Arc<dyn AuditLogService>)
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
}
