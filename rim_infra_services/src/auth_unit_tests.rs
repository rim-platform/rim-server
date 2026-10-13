#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Duration, Utc};
    use rim_core::auth::model::{
        InstanceInvite, InstanceInviteId, Invite, InviteId, Session, SessionId,
    };
    use rim_core::auth::repository::{
        MockInstanceInviteRepository, MockInviteRepository, MockSessionRepository,
    };
    use rim_core::conversation::model::{Conversation, ConversationId};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::{NewUser, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::auth::command::{
        AcceptInviteCmd, CreateInviteCmd, LoginCmd, LogoutCmd, LogoutOtherSessionsCmd,
        PreviewInviteTokenQuery, SignupOpenCmd, SignupWithInstanceInviteCmd,
    };
    use rim_services::auth::service::AuthService;
    use rim_services::session_revocation::{SessionRevocations, WatchedSession};
    use rim_services::{Actor, ServiceError};

    use rim_services::events::{NoopBroadcaster, Observers};

    use crate::AuthRepos;
    use crate::auth::DefaultAuthService;
    use crate::registration_rate_limiter::RegistrationRateLimiter;
    use crate::session_revocation::InProcessSessionRevocations;

    fn noop_observers() -> Observers {
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
    }

    /// Build a complete set of mock repositories; the instance invite repository is passed
    /// separately so callers can configure expectations on it.
    fn repos() -> (
        MockUserRepository,
        MockServerRepository,
        MockServerMemberRepository,
        MockSessionRepository,
        MockInviteRepository,
        MockConversationRepository,
    ) {
        (
            MockUserRepository::new(),
            MockServerRepository::new(),
            MockServerMemberRepository::new(),
            MockSessionRepository::new(),
            MockInviteRepository::new(),
            mock_conversation_repo(),
        )
    }

    /// Build `AuthRepos` from the six standard repos plus an empty instance invite repo.
    fn make_auth_repos(
        user_repo: MockUserRepository,
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        session_repo: MockSessionRepository,
        invite_repo: MockInviteRepository,
        conversation_repo: MockConversationRepository,
    ) -> AuthRepos {
        AuthRepos {
            user_repo: Arc::new(user_repo),
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            session_repo: Arc::new(session_repo),
            invite_repo: Arc::new(invite_repo),
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        }
    }

    /// Build `AuthRepos` with an explicit instance invite repo.
    #[allow(clippy::too_many_arguments)]
    fn make_auth_repos_with_registration(
        user_repo: MockUserRepository,
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        session_repo: MockSessionRepository,
        invite_repo: MockInviteRepository,
        conversation_repo: MockConversationRepository,
        instance_invite_repo: MockInstanceInviteRepository,
    ) -> AuthRepos {
        AuthRepos {
            user_repo: Arc::new(user_repo),
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            session_repo: Arc::new(session_repo),
            invite_repo: Arc::new(invite_repo),
            instance_invite_repo: Arc::new(instance_invite_repo),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        }
    }

    /// Construct a default user for test helpers.
    fn make_user(id: UserId, username: &str) -> User {
        let now = Utc::now();
        User {
            id,
            username: username.to_string(),
            password_hash: Some("hash".to_string()),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: Some(format!("{}@local.invalid", username.to_lowercase())),
            timezone: None,
            principal_id: None,
            deletion_scheduled_for: None,
        }
    }

    /// Construct a default session for test helpers.
    fn make_session(id: SessionId, user_id: UserId) -> Session {
        let now = Utc::now();
        Session {
            id,
            user_id,
            token_hash: "hash".to_string(),
            created_at: now,
            expires_at: now + Duration::hours(1),
            last_seen_at: None,
            revoked_at: None,
            user_agent: None,
            ip: None,
            device_key_id: None,
        }
    }

    /// Construct a valid instance invite in the future.
    fn make_instance_invite(id: InstanceInviteId, max_uses: i32, use_count: i32) -> InstanceInvite {
        let now = Utc::now();
        InstanceInvite {
            id,
            created_by_user_id: Some(UserId::new()),
            token_hash: "invite_hash".to_string(),
            created_at: now,
            expires_at: now + Duration::days(7),
            max_uses,
            use_count,
            revoked_at: None,
            email: None,
        }
    }

    fn mock_conversation_repo() -> MockConversationRepository {
        let mut repo = MockConversationRepository::new();
        // New workspace members auto-join public channels; none exist in these fixtures.
        repo.expect_list_public_channel_ids()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        repo.expect_create_conversation_with_members()
            .returning(|conv, _member_ids| {
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

    #[tokio::test]
    async fn logout_denies_if_session_belongs_to_other_user() {
        let actor_user_id = UserId::new();
        let other_user_id = UserId::new();
        let session_id = SessionId::new();
        let now = chrono::Utc::now();

        let (
            user_repo,
            server_repo,
            server_member_repo,
            mut session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();
        session_repo.expect_get_by_id().returning(move |id| {
            assert_eq!(id, session_id);
            Ok(Some(Session {
                id: session_id,
                user_id: other_user_id,
                token_hash: "h".to_string(),
                created_at: now,
                expires_at: now,
                last_seen_at: None,
                revoked_at: None,
                user_agent: None,
                ip: None,
                device_key_id: None,
            }))
        });
        session_repo.expect_revoke_session().never();

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .logout(LogoutCmd {
                actor: Actor::new(actor_user_id),
                session_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    /// A service over `session_repo`, with every other repository left without expectations.
    fn svc_with_sessions(session_repo: MockSessionRepository) -> DefaultAuthService {
        let (user_repo, server_repo, server_member_repo, _, invite_repo, conversation_repo) =
            repos();
        DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
    }

    #[tokio::test]
    async fn logout_other_sessions_keeps_the_caller_and_returns_the_count() {
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_get_by_id()
            .withf(move |id| *id == session_id)
            .returning(move |id| Ok(Some(make_session(id, user_id))));
        session_repo
            .expect_revoke_all_for_user_except()
            .withf(move |user, keep| *user == user_id && *keep == session_id)
            .times(1)
            .returning(|_, _| Ok(2));
        session_repo.expect_revoke_all_for_user().never();
        let registry = Arc::new(InProcessSessionRevocations::default());
        let mut caller = registry.watch(watched(session_id, user_id));
        let mut other = registry.watch(watched(SessionId::new(), user_id));

        let ended = svc_with_sessions(session_repo)
            .with_session_revocations(registry)
            .logout_other_sessions(LogoutOtherSessionsCmd {
                actor: Actor::new(user_id),
                session_id,
            })
            .await
            .unwrap();
        assert_eq!(ended, 2);
        assert!(other.has_ended(), "the other session's connection closes");
        assert!(!caller.has_ended(), "the caller's connection stays");
    }

    /// The live connection of a session for `user_id`.
    fn watched(session_id: SessionId, user_id: UserId) -> WatchedSession {
        WatchedSession {
            session_id,
            user_id,
            device_key_id: None,
        }
    }

    #[tokio::test]
    async fn logout_ends_the_live_connections_of_the_calling_session_only() {
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_get_by_id()
            .returning(move |id| Ok(Some(make_session(id, user_id))));
        session_repo
            .expect_revoke_session()
            .withf(move |id| *id == session_id)
            .times(1)
            .returning(|_| Ok(()));
        let registry = Arc::new(InProcessSessionRevocations::default());
        let mut caller = registry.watch(watched(session_id, user_id));
        let mut other = registry.watch(watched(SessionId::new(), user_id));

        svc_with_sessions(session_repo)
            .with_session_revocations(registry)
            .logout(LogoutCmd {
                actor: Actor::new(user_id),
                session_id,
            })
            .await
            .unwrap();
        assert!(caller.has_ended());
        assert!(!other.has_ended());
    }

    #[tokio::test]
    async fn logout_other_sessions_denies_a_session_of_another_user() {
        let session_id = SessionId::new();
        let owner_id = UserId::new();

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_get_by_id()
            .returning(move |id| Ok(Some(make_session(id, owner_id))));
        session_repo.expect_revoke_all_for_user_except().never();

        let err = svc_with_sessions(session_repo)
            .logout_other_sessions(LogoutOtherSessionsCmd {
                actor: Actor::new(UserId::new()),
                session_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn logout_other_sessions_without_the_calling_session_is_not_found() {
        let mut session_repo = MockSessionRepository::new();
        session_repo.expect_get_by_id().returning(|_| Ok(None));
        session_repo.expect_revoke_all_for_user_except().never();

        let err = svc_with_sessions(session_repo)
            .logout_other_sessions(LogoutOtherSessionsCmd {
                actor: Actor::new(UserId::new()),
                session_id: SessionId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn logout_other_sessions_passes_a_repository_error_through() {
        let user_id = UserId::new();
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_get_by_id()
            .returning(move |id| Ok(Some(make_session(id, user_id))));
        session_repo
            .expect_revoke_all_for_user_except()
            .returning(|_, _| Err(RepoError::new(RepoErrorKind::Unavailable)));

        let err = svc_with_sessions(session_repo)
            .logout_other_sessions(LogoutOtherSessionsCmd {
                actor: Actor::new(user_id),
                session_id: SessionId::new(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unavailable));
    }

    #[tokio::test]
    async fn create_invite_validates_max_uses() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (
            user_repo,
            server_repo,
            server_member_repo,
            session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();
        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 0,
                recipient_email: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// An invite grants `Member` or `Guest`, never a moderating role.
    #[tokio::test]
    async fn create_invite_rejects_roles_above_member() {
        for role in [
            rim_core::server_member::model::ServerMemberRole::Owner,
            rim_core::server_member::model::ServerMemberRole::Moderator,
        ] {
            let (
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ) = repos();
            let svc = DefaultAuthService::new(
                make_auth_repos(
                    user_repo,
                    server_repo,
                    server_member_repo,
                    session_repo,
                    invite_repo,
                    conversation_repo,
                ),
                None,
                Duration::hours(1),
                noop_observers(),
                Arc::new(crate::email::LogEmailSender),
                None,
            );
            let err = svc
                .create_invite(CreateInviteCmd {
                    actor: Actor::new(UserId::new()),
                    server_id: ServerId::new(),
                    expires_at: None,
                    max_uses: 1,
                    recipient_email: None,
                    role,
                })
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Validation(_)), "{role:?}");
        }
    }

    fn make_owner_member(server_id: ServerId, user_id: UserId) -> ServerMember {
        ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: ServerMemberRole::Owner,
            joined_at: Utc::now(),
        }
    }

    #[tokio::test]
    async fn create_invite_with_recipient_email_sends_email() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (
            user_repo,
            server_repo,
            mut server_member_repo,
            session_repo,
            mut invite_repo,
            conversation_repo,
        ) = repos();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_user_id))));
        invite_repo.expect_create_invite().returning(move |new| {
            Ok(Invite {
                id: InviteId::new(),
                server_id: new.server_id,
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .withf(|msg| msg.to == "invitee@example.com" && msg.body_text.contains("/join/"))
            .times(1)
            .returning(|_| Ok(()));

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 1,
                recipient_email: Some("invitee@example.com".to_string()),
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
            .await
            .unwrap();
        assert!(!result.invite_token.is_empty());
    }

    #[tokio::test]
    async fn create_invite_without_recipient_email_does_not_send() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (
            user_repo,
            server_repo,
            mut server_member_repo,
            session_repo,
            mut invite_repo,
            conversation_repo,
        ) = repos();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_user_id))));
        invite_repo.expect_create_invite().returning(move |new| {
            Ok(Invite {
                id: InviteId::new(),
                server_id: new.server_id,
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender.expect_send().never();

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 1,
                recipient_email: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
            .await
            .unwrap();
        assert!(!result.invite_token.is_empty());
    }

    /// An auth service where `owner` owns `server_id` and has created `today` invites in the
    /// last day; `None` expects the day's invites never to be counted.
    fn invite_service(
        server_id: ServerId,
        owner: UserId,
        today: Option<u64>,
        limits: rim_services::limits::InstanceLimits,
    ) -> DefaultAuthService {
        let (user_repo, server_repo, mut server_member_repo, session_repo, mut invite_repo, conv) =
            repos();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, owner))));
        match today {
            Some(today) => {
                invite_repo
                    .expect_count_created_since()
                    .withf(move |user_id, since| {
                        *user_id == owner
                            && (Utc::now() - Duration::days(1) - *since)
                                .num_seconds()
                                .abs()
                                < 60
                    })
                    .returning(move |_, _| Ok(today));
            }
            None => {
                invite_repo.expect_count_created_since().never();
            }
        }
        invite_repo.expect_create_invite().returning(|new| {
            Ok(Invite {
                id: InviteId::new(),
                server_id: new.server_id,
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
                role: new.role,
            })
        });
        DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conv,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
        .with_limits(limits)
    }

    async fn invite_with_uses(
        svc: &DefaultAuthService,
        server_id: ServerId,
        owner: UserId,
        max_uses: i32,
    ) -> Result<(), ServiceError> {
        svc.create_invite(CreateInviteCmd {
            actor: Actor::new(owner),
            server_id,
            expires_at: None,
            max_uses,
            recipient_email: None,
            role: ServerMemberRole::Member,
        })
        .await
        .map(|_| ())
    }

    #[tokio::test]
    async fn create_invite_follows_the_invites_per_user_per_day_cap() {
        for (cap, today, allowed) in [
            (Some(5), Some(4), true),
            (Some(5), Some(5), false),
            (None, None, true),
        ] {
            let (server_id, owner) = (ServerId::new(), UserId::new());
            let svc = invite_service(
                server_id,
                owner,
                today,
                rim_services::limits::InstanceLimits {
                    invites_per_user_per_day: cap,
                    ..Default::default()
                },
            );
            match invite_with_uses(&svc, server_id, owner, 1).await {
                Ok(()) => assert!(allowed, "{cap:?} {today:?}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {today:?}");
                    assert_eq!(message, "this instance allows 5 invites per user per day");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn create_invite_follows_the_invite_max_uses_cap() {
        for (cap, max_uses, allowed) in [
            (Some(5), 5, true),
            (Some(5), 6, false),
            (None, 10_000, true),
        ] {
            let (server_id, owner) = (ServerId::new(), UserId::new());
            let svc = invite_service(
                server_id,
                owner,
                None,
                rim_services::limits::InstanceLimits {
                    invite_max_uses: cap,
                    ..Default::default()
                },
            );
            match invite_with_uses(&svc, server_id, owner, max_uses).await {
                Ok(()) => assert!(allowed, "{cap:?} {max_uses}"),
                Err(ServiceError::Validation(message)) => {
                    assert!(!allowed, "{cap:?} {max_uses}");
                    assert_eq!(message, "this instance allows 5 uses per invite");
                }
                Err(other) => panic!("unexpected {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn create_invite_rejects_malformed_recipient_email() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (
            user_repo,
            server_repo,
            server_member_repo,
            session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 1,
                recipient_email: Some("not-an-email".to_string()),
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    /// Email delivery is best-effort: an `EmailSender` failure must not fail the mutation, and
    /// the invite (with its usable token) must still be created and returned.
    #[tokio::test]
    async fn create_invite_send_failure_is_non_fatal() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (
            user_repo,
            server_repo,
            mut server_member_repo,
            session_repo,
            mut invite_repo,
            conversation_repo,
        ) = repos();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_user_id))));
        invite_repo.expect_create_invite().returning(move |new| {
            Ok(Invite {
                id: InviteId::new(),
                server_id: new.server_id,
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .returning(|_| Err(anyhow::anyhow!("smtp down")));

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 1,
                recipient_email: Some("invitee@example.com".to_string()),
                role: rim_core::server_member::model::ServerMemberRole::Member,
            })
            .await
            .unwrap();
        assert!(
            !result.invite_token.is_empty(),
            "invite token must still be returned even when the email send fails"
        );
    }

    #[tokio::test]
    async fn accept_invite_returns_conflict_if_username_taken() {
        let server_id = ServerId::new();
        let existing_user_id = UserId::new();
        let invite_id = InviteId::new();
        let now = chrono::Utc::now();

        let (
            mut user_repo,
            server_repo,
            server_member_repo,
            session_repo,
            mut invite_repo,
            conversation_repo,
        ) = repos();
        user_repo.expect_find_by_username().returning(move |_u| {
            Ok(Some(User {
                id: existing_user_id,
                username: "taken".to_string(),
                password_hash: Some("hash".to_string()),
                created_at: now,
                updated_at: now,
                last_login_at: Some(now),
                display_name: None,
                avatar_url: None,
                status_text: None,
                bio: None,
                email: Some("taken@local.invalid".to_string()),
                timezone: None,
                principal_id: None,
                deletion_scheduled_for: None,
            }))
        });
        user_repo.expect_create_user().never();

        invite_repo.expect_get_by_token_hash().returning(move |_h| {
            Ok(Some(Invite {
                id: invite_id,
                server_id,
                created_by_user_id: UserId::new(),
                token_hash: "th".to_string(),
                created_at: now,
                expires_at: None,
                max_uses: 1,
                use_count: 0,
                revoked_at: None,
                role: rim_core::server_member::model::ServerMemberRole::Member,
            }))
        });
        invite_repo.expect_try_consume().never();

        let svc = DefaultAuthService::new(
            make_auth_repos(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .accept_invite(AcceptInviteCmd {
                invite_token: "token".to_string(),
                username: "taken".to_string(),
                password: "password123".to_string(),
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(m) if m.contains("username")));
    }

    // ── Registration unit tests ──────────────────────────────────────────────

    /// Helper: build a service with only the repos configured for a given test.
    #[allow(clippy::too_many_arguments)]
    fn svc_with_registration(
        user_repo: MockUserRepository,
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        session_repo: MockSessionRepository,
        invite_repo: MockInviteRepository,
        conversation_repo: MockConversationRepository,
        instance_invite_repo: MockInstanceInviteRepository,
    ) -> DefaultAuthService {
        DefaultAuthService::new(
            make_auth_repos_with_registration(
                user_repo,
                server_repo,
                server_member_repo,
                session_repo,
                invite_repo,
                conversation_repo,
                instance_invite_repo,
            ),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
    }

    /// A signup through an instance invite creates a user with no server_member row.
    #[tokio::test]
    async fn instance_invite_creates_user_with_no_workspace() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let chain = (uuid::Uuid::from(bt.id), bt.created_by_user_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        signup_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        // The account records the invite it used and that invite's creator.
        user_repo
            .expect_record_invite_chain()
            .withf(move |id, invite, by| *id == user_id && (*invite, *by) == chain)
            .times(1)
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, mut smr, _, ir, cr) = repos();
        // server_member::add_member must NEVER be called for an instance invite signup.
        smr.expect_add_member().never();

        let svc =
            svc_with_registration(user_repo, sr, smr, session_repo, ir, cr, signup_invite_repo);

        let res = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
        assert!(
            res.server_id.is_none(),
            "should have no server_id for an instance invite signup"
        );
    }

    /// `max_uses=1` instance invite: first use succeeds, second returns `InvalidToken`.
    #[tokio::test]
    async fn instance_invite_single_use_enforced() {
        let invite_id = InstanceInviteId::new();
        let invite = make_instance_invite(invite_id, 1, 0);

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        // First call: consumed (try_consume returns true).
        // Second call: exhausted (try_consume returns false).
        let mut call_count = 0u32;
        instance_invite_repo
            .expect_try_consume()
            .times(2)
            .returning(move |_| {
                call_count += 1;
                Ok(call_count == 1)
            });

        // Mock user creation (first call only).
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            session_repo,
            ir,
            cr,
            instance_invite_repo,
        );

        // First use: allowed.
        let res = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "bob".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "bob@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await;
        assert!(res.is_ok(), "first use should succeed");

        // Second use: exhausted → InvalidToken.
        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "carol".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "carol@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    /// Multi-use invite: three successes, fourth fails with `InvalidToken`.
    #[tokio::test]
    async fn instance_invite_multi_use_allowed_until_exhausted() {
        let invite_id = InstanceInviteId::new();
        let invite = make_instance_invite(invite_id, 3, 0);

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));

        let mut call_count = 0u32;
        instance_invite_repo
            .expect_try_consume()
            .times(4)
            .returning(move |_| {
                call_count += 1;
                Ok(call_count <= 3)
            });

        let user_id = UserId::new();
        let session_id = SessionId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            session_repo,
            ir,
            cr,
            instance_invite_repo,
        );

        for _ in 0..3 {
            svc.signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "user".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "user@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap();
        }

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "user4".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "user4@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidToken),
            "4th call should be limited, got: {err:?}"
        );
    }

    /// Expired instance invite returns `ExpiredToken`.
    #[tokio::test]
    async fn instance_invite_expired_rejected() {
        let invite_id = InstanceInviteId::new();
        let mut invite = make_instance_invite(invite_id, 1, 0);
        invite.expires_at = Utc::now() - Duration::seconds(1);

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, instance_invite_repo);

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::ExpiredToken), "got: {err:?}");
    }

    /// Revoked instance invite returns `InvalidToken`.
    #[tokio::test]
    async fn instance_invite_revoked_rejected() {
        let invite_id = InstanceInviteId::new();
        let mut invite = make_instance_invite(invite_id, 1, 0);
        invite.revoked_at = Some(Utc::now());

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, instance_invite_repo);

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    /// `signup_open` with no tokens and valid creds succeeds in open mode.
    #[tokio::test]
    async fn signup_open_accepts_no_token() {
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        // No invite, no chain.
        user_repo.expect_record_invite_chain().never();
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            session_repo,
            ir,
            cr,
            MockInstanceInviteRepository::new(),
        );

        let res = svc
            .signup_open(SignupOpenCmd {
                username: "carol".to_string(),
                password: "s3cur3p4ss!".to_string(),
                remote_ip: "1.2.3.4".to_string(),
                email: "carol@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
        assert!(res.server_id.is_none());
    }

    const TERMS: &str = "https://rim.example.org/terms";

    /// A user store for one signup that creates `user_id` and expects the acceptance of
    /// [`TERMS`] recorded on it once.
    fn user_repo_recording_terms(user_id: UserId) -> MockUserRepository {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));
        let before = Utc::now();
        user_repo
            .expect_record_terms_acceptance()
            .withf(move |id, url, at| *id == user_id && url == TERMS && *at >= before)
            .times(1)
            .returning(|_, _, _| Ok(()));
        user_repo
    }

    fn session_repo_creating() -> MockSessionRepository {
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(|new| Ok(make_session(SessionId::new(), new.user_id)));
        session_repo
    }

    /// The open and instance-invite signups record the terms they accepted on the new account.
    #[tokio::test]
    async fn password_signups_record_the_accepted_terms() {
        let user_id = UserId::new();
        let (_, sr, smr, _, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo_recording_terms(user_id),
            sr,
            smr,
            session_repo_creating(),
            ir,
            cr,
            MockInstanceInviteRepository::new(),
        );
        svc.signup_open(SignupOpenCmd {
            username: "carol".to_string(),
            password: "s3cur3p4ss!".to_string(),
            remote_ip: "1.2.3.4".to_string(),
            email: "carol@example.com".to_string(),
            display_name: None,
            terms_accepted_url: Some(TERMS.to_string()),
        })
        .await
        .expect("open signup");

        let user_id = UserId::new();
        let invite = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        instance_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));
        let (_, sr, smr, _, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo_recording_terms(user_id),
            sr,
            smr,
            session_repo_creating(),
            ir,
            cr,
            instance_invite_repo,
        );
        svc.signup_with_instance_invite(SignupWithInstanceInviteCmd {
            instance_invite_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
            terms_accepted_url: Some(TERMS.to_string()),
        })
        .await
        .expect("instance invite signup");
    }

    /// Open-mode signup rate-limited after threshold.
    ///
    /// The signup rate limiter is checked inside `signup_open`.
    /// This test verifies the limiter mechanics directly using `RegistrationRateLimiter`.
    #[tokio::test]
    async fn signup_open_rate_limited_after_threshold() {
        // Use a tight limiter: 2 req / 60 sec.
        let signup_rl = RegistrationRateLimiter::new(60, 2);
        let ip = "10.0.0.1";
        let key = "signup";
        let now = Utc::now();
        signup_rl.check_and_record(ip, key, now);
        signup_rl.check_and_record(ip, key, now);
        let decision = signup_rl.check_and_record(ip, key, now);
        assert!(
            matches!(
                decision,
                rim_services::command::rate_limit::RateLimitDecision::Limited { .. }
            ),
            "3rd call over limit=2 should be limited"
        );
    }

    /// signup with username already taken returns `Conflict`.
    #[tokio::test]
    async fn signup_username_already_taken_returns_conflict() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let existing_id = UserId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        signup_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(make_user(existing_id, "alice"))));

        let (_, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(user_repo, sr, smr, sess_r, ir, cr, signup_invite_repo);

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Conflict(_)), "got: {err:?}");
    }

    /// signup with password shorter than 8 chars returns `Validation`.
    #[tokio::test]
    async fn signup_password_too_short_returns_validation() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, signup_invite_repo);

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "short".to_string(), // < 8 chars — validation fires before email check
                email: "alice@example.com".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("password")),
            "got: {err:?}"
        );
    }

    /// Every password signup refuses a reserved username or display name before it touches
    /// a repository.
    #[tokio::test]
    async fn password_signups_refuse_reserved_names() {
        use crate::reserved_names::{RESERVED_NAME_REFUSAL, ReservedNames};

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            MockInstanceInviteRepository::new(),
        )
        .with_reserved_names(ReservedNames::parse("rim,admin").unwrap());
        let open = |username: &str, display_name: Option<&str>| SignupOpenCmd {
            username: username.to_string(),
            password: "s3cur3p4ss!".to_string(),
            remote_ip: "1.2.3.4".to_string(),
            email: "carol@example.com".to_string(),
            display_name: display_name.map(str::to_string),
            terms_accepted_url: None,
        };
        let refusals = [
            svc.signup_open(open("rim-team", None)).await,
            svc.signup_open(open("carol", Some("ＡＤＭＩＮ"))).await,
        ];
        let invited = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "carol".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "carol@example.com".to_string(),
                display_name: Some("Rim Support".to_string()),
                terms_accepted_url: None,
            })
            .await;
        let accepted = svc
            .accept_invite(AcceptInviteCmd {
                invite_token: "token".to_string(),
                username: "admin_2".to_string(),
                password: "password123".to_string(),
                terms_accepted_url: None,
            })
            .await
            .map(|_| ());
        for res in refusals
            .into_iter()
            .map(|r| r.map(|_| ()))
            .chain([invited.map(|_| ()), accepted])
        {
            assert!(
                matches!(&res, Err(ServiceError::Validation(m)) if m == RESERVED_NAME_REFUSAL),
                "{res:?}"
            );
        }
    }

    /// `previewInviteToken` for a valid workspace invite returns `WORKSPACE` type
    /// with server name and inviter display name, and no owner name for a workspace without an
    /// owner; underlying row is NOT mutated.
    #[tokio::test]
    async fn preview_invite_token_workspace_returns_workspace_payload() {
        let server_id = ServerId::new();
        let inviter_id = UserId::new();
        let invite_id = InviteId::new();
        let now = Utc::now();

        let inv = Invite {
            id: invite_id,
            server_id,
            created_by_user_id: inviter_id,
            token_hash: "wh".to_string(),
            created_at: now,
            expires_at: Some(now + Duration::days(7)),
            max_uses: 5,
            use_count: 0,
            revoked_at: None,
            role: rim_core::server_member::model::ServerMemberRole::Member,
        };

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(|_| Ok(None)); // not an instance invite

        let mut invite_repo = MockInviteRepository::new();
        invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(inv.clone())));
        // try_consume / try_mark_used must NOT be called (read-only).
        invite_repo.expect_try_consume().never();

        let mut server_repo = MockServerRepository::new();
        server_repo.expect_get_by_id().returning(move |_| {
            Ok(Some(Server {
                id: server_id,
                name: "Acme".to_string(),
                created_at: now,
                updated_at: now,
            }))
        });

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_id().returning(move |_| {
            let mut u = make_user(inviter_id, "dave");
            u.display_name = Some("Dave Smith".to_string());
            Ok(Some(u))
        });

        let (_, _, mut smr, sess_r, _, cr) = repos();
        smr.expect_get_owner_id().returning(|_| Ok(None));
        let svc = svc_with_registration(
            user_repo,
            server_repo,
            smr,
            sess_r,
            invite_repo,
            cr,
            instance_invite_repo,
        );

        let res = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_ws_token".to_string(),
                remote_ip: "1.2.3.4".to_string(),
            })
            .await
            .unwrap();

        use rim_services::auth::dto::InviteTokenType;
        assert_eq!(res.token_type, InviteTokenType::Workspace);
        assert!(res.workspace.is_some());
        let ws = res.workspace.unwrap();
        assert_eq!(ws.server_name, "Acme");
        assert_eq!(ws.invited_by_display_name, "Dave Smith");
        assert_eq!(ws.owner_display_name, None);
    }

    /// `previewInviteToken` for a valid instance invite returns `INSTANCE` type
    /// with `expiresAt` set and `workspace` null; row not mutated.
    #[tokio::test]
    async fn preview_invite_token_instance_returns_minimal_payload() {
        let invite_id = InstanceInviteId::new();
        let invite = make_instance_invite(invite_id, 5, 0);

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(|_| Ok(None));

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        let expires_at = invite.expires_at;
        instance_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(invite.clone())));
        // try_consume must NOT be called.
        instance_invite_repo.expect_try_consume().never();

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, instance_invite_repo);

        let res = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_ii_token".to_string(),
                remote_ip: "1.2.3.4".to_string(),
            })
            .await
            .unwrap();

        use rim_services::auth::dto::InviteTokenType;
        assert_eq!(res.token_type, InviteTokenType::Instance);
        assert_eq!(res.expires_at, Some(expires_at));
        assert!(res.workspace.is_none());
    }

    /// `previewInviteToken` on an expired instance invite returns `ExpiredToken`.
    #[tokio::test]
    async fn preview_invite_token_expired_returns_expired_token() {
        let mut bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        bt.expires_at = Utc::now() - Duration::seconds(1);

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, signup_invite_repo);

        let err = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_invite_token".to_string(),
                remote_ip: "1.2.3.4".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::ExpiredToken), "got: {err:?}");
    }

    /// `previewInviteToken` on a missing / consumed / revoked token all collapse to
    /// `InvalidToken` (three sub-cases).
    #[tokio::test]
    async fn preview_invite_token_invalid_returns_invalid_token() {
        // Sub-case A: missing (no row in any table).
        {
            let mut ii_repo = MockInstanceInviteRepository::new();
            ii_repo.expect_get_by_token_hash().returning(|_| Ok(None));
            let mut ir = MockInviteRepository::new();
            ir.expect_get_by_token_hash().returning(|_| Ok(None));

            let (ur, sr, smr, sess_r, _, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, ii_repo);

            let err = svc
                .preview_invite_token(PreviewInviteTokenQuery {
                    token: "nonexistent".to_string(),
                    remote_ip: "1.2.3.4".to_string(),
                })
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::InvalidToken),
                "missing: got {err:?}"
            );
        }

        // Sub-case B: used-up instance invite.
        {
            let invite = make_instance_invite(InstanceInviteId::new(), 1, 1);

            let mut ii_repo = MockInstanceInviteRepository::new();
            ii_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(invite.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, ii_repo);

            let err = svc
                .preview_invite_token(PreviewInviteTokenQuery {
                    token: "consumed".to_string(),
                    remote_ip: "1.2.3.4".to_string(),
                })
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::InvalidToken),
                "consumed: got {err:?}"
            );
        }

        // Sub-case C: revoked instance invite.
        {
            let invite_id = InstanceInviteId::new();
            let mut invite = make_instance_invite(invite_id, 1, 0);
            invite.revoked_at = Some(Utc::now());

            let mut ii_repo = MockInstanceInviteRepository::new();
            ii_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(invite.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, ii_repo);

            let err = svc
                .preview_invite_token(PreviewInviteTokenQuery {
                    token: "revoked".to_string(),
                    remote_ip: "1.2.3.4".to_string(),
                })
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::InvalidToken),
                "revoked: got {err:?}"
            );
        }
    }

    /// `previewInviteToken` rate-limited after 30 calls from the same IP within
    /// a 5-minute window; exactly 30 calls succeed.
    #[tokio::test]
    async fn preview_invite_token_rate_limited_after_threshold() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);

        let mut bt_repo = MockInstanceInviteRepository::new();
        bt_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let preview_rl = RegistrationRateLimiter::preview_default(); // 30/5min

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = DefaultAuthService::with_rate_limiters(
            make_auth_repos_with_registration(ur, sr, smr, sess_r, ir, cr, bt_repo),
            None,
            Duration::hours(1),
            noop_observers(),
            preview_rl,
            RegistrationRateLimiter::signup_default(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let ip = "5.6.7.8";
        for i in 1..=30 {
            let res = svc
                .preview_invite_token(PreviewInviteTokenQuery {
                    token: "raw_invite_token".to_string(),
                    remote_ip: ip.to_string(),
                })
                .await;
            assert!(res.is_ok(), "call {i} should succeed, got: {res:?}");
        }

        let err = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_invite_token".to_string(),
                remote_ip: ip.to_string(),
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::RateLimited { .. }),
            "31st call should be rate-limited, got: {err:?}"
        );
    }

    // ── Email and display name at signup ─────────────────────────────────────

    /// Happy path: valid email + displayName are persisted correctly.
    ///
    /// Input `"  Foo@Bar.COM "` must be stored as `"foo@bar.com"` (trimmed + lowercased).
    /// `display_name` must be stored as the trimmed input value.
    #[tokio::test]
    async fn signup_with_email_happy_path() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        signup_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                // Verify normalisation happened before the repo is called.
                assert_eq!(
                    new.email.as_deref(),
                    Some("foo@bar.com"),
                    "email must be lowercased and trimmed"
                );
                assert_eq!(
                    new.display_name,
                    Some("Alice Smith".to_string()),
                    "display_name must match trimmed input"
                );
                Ok(User {
                    id: user_id,
                    username: new.username.clone(),
                    password_hash: new.password.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login_at: None,
                    display_name: new.display_name.clone(),
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    email: new.email.clone(),
                    timezone: None,
                    principal_id: None,
                    deletion_scheduled_for: None,
                })
            });

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc =
            svc_with_registration(user_repo, sr, smr, session_repo, ir, cr, signup_invite_repo);

        let res = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "  Foo@Bar.COM ".to_string(), // whitespace + mixed case
                display_name: Some("Alice Smith".to_string()),
                terms_accepted_url: None,
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
    }

    /// Duplicate email (case-insensitive) returns `EmailTaken`.
    ///
    /// Pre-seeds a user with `email = "x@y.z"`; signup with `"X@Y.Z"` must
    /// return `ServiceError::EmailTaken`, verifying case-insensitive uniqueness.
    #[tokio::test]
    async fn signup_duplicate_email_returns_email_taken() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let existing_id = UserId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        // try_mark_used must NOT be called when signup fails.
        signup_invite_repo.expect_try_consume().never();

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        // find_by_email returns Some for the duplicate.
        user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(make_user(existing_id, "other_user"))));
        user_repo.expect_create_user().never();

        let (_, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(user_repo, sr, smr, sess_r, ir, cr, signup_invite_repo);

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "bob".to_string(),
                password: "s3cur3p4ss!".to_string(),
                // Mixed-case version of the pre-seeded email; normalises to "x@y.zz".
                email: "X@Y.ZZ".to_string(),
                display_name: None,
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::EmailTaken), "got: {err:?}");
    }

    /// Malformed email input returns `Validation`.
    ///
    /// Parametrized over:
    /// "plain", "@host.com", "a@", "a@b", "a b@c.com", empty string, 255-char string.
    #[tokio::test]
    async fn signup_malformed_email_returns_validation() {
        let bad_emails = [
            "plain",
            "@host.com",
            "a@",
            "a@b",
            "a b@c.com",
            "",
            &"a".repeat(255), // exceeds 254 char limit (no @ sign either)
        ];

        for bad_email in &bad_emails {
            // Each iteration builds a fresh service — we only need the validation to fire,
            // which happens before any repo calls.
            let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);

            let mut signup_invite_repo = MockInstanceInviteRepository::new();
            signup_invite_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(bt.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, signup_invite_repo);

            let err = svc
                .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                    instance_invite_token: "raw_token".to_string(),
                    username: "alice".to_string(),
                    password: "s3cur3p4ss!".to_string(),
                    email: bad_email.to_string(),
                    display_name: None,
                    terms_accepted_url: None,
                })
                .await
                .unwrap_err();

            assert!(
                matches!(err, ServiceError::Validation(_)),
                "email {bad_email:?} should produce Validation, got: {err:?}"
            );
        }
    }

    /// `display_name: None` falls back to `username` on the persisted user.
    #[tokio::test]
    async fn signup_displayname_omitted_falls_back_to_username() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        signup_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                // When display_name was None, the service falls back to username.
                assert_eq!(
                    new.display_name,
                    Some("alice".to_string()),
                    "should fall back to username when display_name is None"
                );
                Ok(User {
                    id: user_id,
                    username: new.username.clone(),
                    password_hash: new.password.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login_at: None,
                    display_name: new.display_name.clone(),
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    email: new.email.clone(),
                    timezone: None,
                    principal_id: None,
                    deletion_scheduled_for: None,
                })
            });

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc =
            svc_with_registration(user_repo, sr, smr, session_repo, ir, cr, signup_invite_repo);

        svc.signup_with_instance_invite(SignupWithInstanceInviteCmd {
            instance_invite_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
            terms_accepted_url: None,
        })
        .await
        .unwrap();
    }

    /// `display_name: Some("   ")` (blank after trim) falls back to `username`.
    ///
    /// Must NOT return a `Validation` error — blank means "use my username."
    #[tokio::test]
    async fn signup_displayname_blank_falls_back_to_username() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        signup_invite_repo
            .expect_try_consume()
            .returning(|_| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_record_invite_chain()
            .returning(|_, _, _| Ok(()));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                // Blank display_name falls back to username silently.
                assert_eq!(
                    new.display_name,
                    Some("alice".to_string()),
                    "blank display_name should fall back to username"
                );
                Ok(User {
                    id: user_id,
                    username: new.username.clone(),
                    password_hash: new.password.clone(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                    last_login_at: None,
                    display_name: new.display_name.clone(),
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    email: new.email.clone(),
                    timezone: None,
                    principal_id: None,
                    deletion_scheduled_for: None,
                })
            });

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc =
            svc_with_registration(user_repo, sr, smr, session_repo, ir, cr, signup_invite_repo);

        svc.signup_with_instance_invite(SignupWithInstanceInviteCmd {
            instance_invite_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: Some("   ".to_string()), // blank after trim → falls back to username
            terms_accepted_url: None,
        })
        .await
        .unwrap();
    }

    /// `display_name` of 81 chars (post-trim) returns `Validation`.
    #[tokio::test]
    async fn signup_displayname_too_long_returns_validation() {
        let bt = make_instance_invite(InstanceInviteId::new(), 1, 0);

        let mut signup_invite_repo = MockInstanceInviteRepository::new();
        signup_invite_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, signup_invite_repo);

        let long_name = "x".repeat(81); // 81 chars — one over the 80-char limit

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: Some(long_name),
                terms_accepted_url: None,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("display name")),
            "got: {err:?}"
        );
    }

    // ── DefaultAuthService::validate_email ────────────────────────────────────

    #[test]
    fn validate_email_accepts_a_well_formed_address() {
        let normalised = DefaultAuthService::validate_email("alice@example.com").unwrap();
        assert_eq!(normalised, "alice@example.com");
    }

    #[test]
    fn validate_email_trims_and_lowercases() {
        let normalised = DefaultAuthService::validate_email("  Foo@Bar.COM  ").unwrap();
        assert_eq!(normalised, "foo@bar.com");
    }

    #[test]
    fn validate_email_rejects_missing_at_sign() {
        let err = DefaultAuthService::validate_email("alice.example.com").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_email_rejects_missing_domain() {
        let err = DefaultAuthService::validate_email("alice@").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_email_rejects_domain_without_dot() {
        let err = DefaultAuthService::validate_email("alice@localhost").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_email_rejects_internal_whitespace() {
        let err = DefaultAuthService::validate_email("alice bob@example.com").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_email_rejects_empty_string() {
        let err = DefaultAuthService::validate_email("   ").unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[test]
    fn validate_email_rejects_address_over_254_chars() {
        // 255 chars total (250-char local part + "@a.co", which is 5 chars), all otherwise
        // well-formed.
        let local = "a".repeat(250);
        let email = format!("{local}@a.co");
        assert_eq!(email.len(), 255);
        let err = DefaultAuthService::validate_email(&email).unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    // ─── login rate limiting ────────────────────────────────────────────────

    /// Build a service for login tests with the given user/session repos and login limiters
    /// (per IP, per (IP, username), per username).
    fn login_svc(
        user_repo: MockUserRepository,
        session_repo: MockSessionRepository,
        ip_limiter: RegistrationRateLimiter,
        ip_account_limiter: RegistrationRateLimiter,
        account_limiter: RegistrationRateLimiter,
    ) -> DefaultAuthService {
        let (_, sr, smr, _, ir, cr) = repos();
        DefaultAuthService::new(
            make_auth_repos(user_repo, sr, smr, session_repo, ir, cr),
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
        .with_login_rate_limiters(ip_limiter, ip_account_limiter, account_limiter)
    }

    fn login_cmd(username: &str, password: &str, remote_ip: &str) -> LoginCmd {
        LoginCmd {
            username: username.to_string(),
            password: password.to_string(),
            remote_ip: remote_ip.to_string(),
        }
    }

    #[tokio::test]
    async fn login_succeeds_and_successes_do_not_count_against_account_limit() {
        let user_id = UserId::new();
        let mut user = make_user(user_id, "alice");
        user.password_hash = Some(rim_auth::password::hash_password("correct-horse").unwrap());

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(2)
            .returning(move |_| Ok(Some(user.clone())));
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .times(2)
            .returning(|new| Ok(make_session(SessionId::new(), new.user_id)));

        // Failure limits of 1 would block the second call if successes were recorded.
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 10),
            RegistrationRateLimiter::new(900, 1),
            RegistrationRateLimiter::new(3600, 1),
        );

        for _ in 0..2 {
            let res = svc
                .login(login_cmd("alice", "correct-horse", "1.1.1.1"))
                .await
                .unwrap();
            assert_eq!(res.user_id, user_id);
        }
    }

    #[tokio::test]
    async fn login_unknown_user_is_unauthenticated() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo.expect_find_by_username().returning(|_| Ok(None));

        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::login_ip_default(),
            RegistrationRateLimiter::login_ip_account_default(),
            RegistrationRateLimiter::login_account_default(),
        );

        let err = svc
            .login(login_cmd("ghost", "whatever", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn login_to_an_account_whose_email_is_blocked_is_forbidden() {
        let user_id = UserId::new();
        let mut user = make_user(user_id, "banned");
        user.password_hash = Some(rim_auth::password::hash_password("correct-horse").unwrap());
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(user.clone())));
        user_repo
            .expect_is_email_blocked()
            .withf(|email| email == "banned@local.invalid")
            .returning(|_| Ok(true));
        let mut session_repo = MockSessionRepository::new();
        session_repo.expect_create_session().never();
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 10),
            RegistrationRateLimiter::new(900, 10),
            RegistrationRateLimiter::new(3600, 10),
        );
        let err = svc
            .login(login_cmd("banned", "correct-horse", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
    }

    #[tokio::test]
    async fn login_to_a_key_only_account_is_refused_like_a_wrong_password() {
        let user_id = UserId::new();
        let user = User {
            password_hash: None,
            email: None,
            principal_id: Some("rimp1_keyonly".to_string()),
            ..make_user(user_id, "keyonly")
        };
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(1)
            .returning(move |_| Ok(Some(user.clone())));
        let mut session_repo = MockSessionRepository::new();
        session_repo.expect_create_session().never();

        // A per-(IP, username) failure limit of 1: the refusal is counted as a failed login,
        // exactly like a wrong password, so the next attempt is throttled before any lookup.
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 10),
            RegistrationRateLimiter::new(900, 1),
            RegistrationRateLimiter::new(3600, 10),
        );
        let err = svc
            .login(login_cmd("keyonly", "any-password", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated), "{err:?}");
        let err = svc
            .login(login_cmd("keyonly", "any-password", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    }

    #[tokio::test]
    async fn login_is_rate_limited_per_ip_before_any_lookup() {
        let mut user_repo = MockUserRepository::new();
        // Only allowed attempts may reach the repository (and Argon2): two from the
        // throttled IP plus one from the unaffected IP.
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(3)
            .returning(|_| Ok(None));

        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 2),
            RegistrationRateLimiter::new(900, 100),
            RegistrationRateLimiter::new(3600, 100),
        );

        for i in 0..2 {
            let err = svc
                .login(login_cmd(&format!("user{i}"), "pw", "9.9.9.9"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
        let err = svc
            .login(login_cmd("user3", "pw", "9.9.9.9"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        // A different source IP is unaffected.
        let err = svc
            .login(login_cmd("user4", "pw", "8.8.8.8"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn login_account_limit_spans_source_ips_and_ignores_username_case() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(2)
            .returning(|_| Ok(None));

        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 100),
            RegistrationRateLimiter::new(3600, 2),
        );

        // Two failures from two different IPs exhaust the account budget...
        for (name, ip) in [("victim", "1.1.1.1"), ("Victim", "2.2.2.2")] {
            let err = svc.login(login_cmd(name, "guess", ip)).await.unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
        // ...so a third IP is throttled without touching the repository.
        let err = svc
            .login(login_cmd("VICTIM", "guess", "3.3.3.3"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn login_repo_error_does_not_count_against_account_limit() {
        let mut user_repo = MockUserRepository::new();
        let mut seq = mockall::Sequence::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Err(RepoError::with_message(RepoErrorKind::Internal, "db down")));
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Ok(None));

        // Limits of 1: if the infrastructure failure consumed a slot, the second attempt
        // would be rate limited instead of reaching the repository.
        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 1),
            RegistrationRateLimiter::new(3600, 1),
        );

        let err = svc
            .login(login_cmd("alice", "pw", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(!matches!(
            err,
            ServiceError::Unauthenticated | ServiceError::RateLimited { .. }
        ));
        let err = svc
            .login(login_cmd("alice", "pw", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn login_pair_limit_locks_out_the_guesser_not_the_owner() {
        let user_id = UserId::new();
        let mut user = make_user(user_id, "victim");
        user.password_hash = Some(rim_auth::password::hash_password("correct-horse").unwrap());

        let mut user_repo = MockUserRepository::new();
        // Two guesses from the attacker, one attempt on another account from the same
        // address, then the owner's login; the throttled third guess never reaches the
        // repository.
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(4)
            .returning(move |name| Ok((name == "victim").then(|| user.clone())));
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .times(1)
            .returning(|new| Ok(make_session(SessionId::new(), new.user_id)));

        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 2),
            RegistrationRateLimiter::new(3600, 100),
        );

        for _ in 0..2 {
            let err = svc
                .login(login_cmd("victim", "guess", "6.6.6.6"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
        let err = svc
            .login(login_cmd("Victim", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        // The limit is per account too: the same address can still try another username.
        let err = svc
            .login(login_cmd("bob", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));

        // The owner, from their own address, still signs in.
        let res = svc
            .login(login_cmd("victim", "correct-horse", "1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(res.user_id, user_id);
    }

    #[tokio::test]
    async fn login_account_backstop_rejection_does_not_consume_pair_budget() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(1)
            .returning(|_| Ok(None));

        // Clones share their buckets, so the second service below sees this pair state.
        let ip_account = RegistrationRateLimiter::new(900, 2);
        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 100),
            ip_account.clone(),
            RegistrationRateLimiter::new(3600, 1),
        );

        // One failure from elsewhere exhausts the per-username backstop...
        let err = svc
            .login(login_cmd("alice", "pw", "2.2.2.2"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
        // ...so this IP is throttled by it, without charging its own pair budget.
        let err = svc
            .login(login_cmd("alice", "pw", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        // Once the backstop clears (a fresh one here), the pair still has both slots.
        let svc = login_svc(
            {
                let mut repo = MockUserRepository::new();
                repo.expect_is_email_blocked().returning(|_| Ok(false));
                repo.expect_find_by_username().returning(|_| Ok(None));
                repo
            },
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 100),
            ip_account,
            RegistrationRateLimiter::new(3600, 100),
        );
        for _ in 0..2 {
            let err = svc
                .login(login_cmd("alice", "pw", "1.1.1.1"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
    }

    #[tokio::test]
    async fn login_pair_limited_attempt_does_not_charge_account_backstop() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .times(2)
            .returning(|_| Ok(None));

        let svc = login_svc(
            user_repo,
            MockSessionRepository::new(),
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 1),
            RegistrationRateLimiter::new(3600, 2),
        );

        // One failure exhausts this address's pair budget...
        let err = svc
            .login(login_cmd("alice", "pw", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
        // ...and further throttled attempts from it don't spend the account's backstop...
        for _ in 0..3 {
            let err = svc
                .login(login_cmd("alice", "pw", "6.6.6.6"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::RateLimited { .. }));
        }
        // ...so another address still gets the backstop's second slot.
        let err = svc
            .login(login_cmd("alice", "pw", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    /// Mocks for login tests against one real account `victim` / `correct-horse`; any other
    /// username is unknown.
    fn victim_login_mocks(user_id: UserId) -> (MockUserRepository, MockSessionRepository) {
        let mut user = make_user(user_id, "victim");
        user.password_hash = Some(rim_auth::password::hash_password("correct-horse").unwrap());
        let mut user_repo = MockUserRepository::new();
        // Exact match, like the real repository: usernames are case-sensitive.
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .returning(move |name| Ok((name == "victim").then(|| user.clone())));
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(|new| Ok(make_session(SessionId::new(), new.user_id)));
        (user_repo, session_repo)
    }

    #[tokio::test]
    async fn login_known_source_skips_tripped_account_backstop() {
        let user_id = UserId::new();
        let (user_repo, session_repo) = victim_login_mocks(user_id);
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 100),
            RegistrationRateLimiter::new(3600, 2),
        );

        // The owner signs in from home, making it a known source.
        svc.login(login_cmd("victim", "correct-horse", "1.1.1.1"))
            .await
            .unwrap();

        // A distributed guesser trips the per-username backstop...
        for ip in ["6.6.6.1", "6.6.6.2"] {
            let err = svc
                .login(login_cmd("victim", "guess", ip))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
        let err = svc
            .login(login_cmd("victim", "guess", "6.6.6.3"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));
        // ...which also holds back the owner on a new address...
        let err = svc
            .login(login_cmd("victim", "correct-horse", "7.7.7.7"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        // ...but not from home.
        let res = svc
            .login(login_cmd("victim", "correct-horse", "1.1.1.1"))
            .await
            .unwrap();
        assert_eq!(res.user_id, user_id);
    }

    #[tokio::test]
    async fn login_known_source_still_has_pair_limit_and_spares_backstop() {
        let user_id = UserId::new();
        let (user_repo, session_repo) = victim_login_mocks(user_id);
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 2),
            RegistrationRateLimiter::new(3600, 1),
        );

        svc.login(login_cmd("victim", "correct-horse", "1.1.1.1"))
            .await
            .unwrap();

        // Failures from a known source are still held to the strict pair limit...
        for _ in 0..2 {
            let err = svc
                .login(login_cmd("victim", "typo", "1.1.1.1"))
                .await
                .unwrap_err();
            assert!(matches!(err, ServiceError::Unauthenticated));
        }
        let err = svc
            .login(login_cmd("victim", "typo", "1.1.1.1"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));

        // ...but didn't spend the backstop, so another address still gets its one slot.
        let err = svc
            .login(login_cmd("victim", "guess", "2.2.2.2"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
    }

    #[tokio::test]
    async fn login_failure_does_not_make_a_source_known() {
        let (user_repo, session_repo) = victim_login_mocks(UserId::new());
        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 100),
            RegistrationRateLimiter::new(3600, 1),
        );

        // A failed attempt from 6.6.6.6 spends the single backstop slot...
        let err = svc
            .login(login_cmd("victim", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
        // ...and doesn't earn that address the bypass.
        let err = svc
            .login(login_cmd("victim", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::RateLimited { .. }));
    }

    #[tokio::test]
    async fn login_to_a_case_variant_account_does_not_exempt_the_other() {
        let mut victim = make_user(UserId::new(), "alice");
        victim.password_hash = Some(rim_auth::password::hash_password("victim-pass").unwrap());
        let mut attacker = make_user(UserId::new(), "Alice");
        attacker.password_hash = Some(rim_auth::password::hash_password("attacker-pass").unwrap());

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_is_email_blocked().returning(|_| Ok(false));
        user_repo
            .expect_find_by_username()
            .returning(move |name| match name {
                "alice" => Ok(Some(victim.clone())),
                "Alice" => Ok(Some(attacker.clone())),
                _ => Ok(None),
            });
        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(|new| Ok(make_session(SessionId::new(), new.user_id)));

        let svc = login_svc(
            user_repo,
            session_repo,
            RegistrationRateLimiter::new(300, 100),
            RegistrationRateLimiter::new(900, 100),
            RegistrationRateLimiter::new(3600, 1),
        );

        // The attacker signs in to their own "Alice" account from 6.6.6.6...
        svc.login(login_cmd("Alice", "attacker-pass", "6.6.6.6"))
            .await
            .unwrap();
        // ...which must not exempt 6.6.6.6 from alice's backstop.
        let err = svc
            .login(login_cmd("alice", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Unauthenticated));
        let err = svc
            .login(login_cmd("alice", "guess", "6.6.6.6"))
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::RateLimited { .. }),
            "6.6.6.6 must still be held to alice's backstop"
        );
    }
}
