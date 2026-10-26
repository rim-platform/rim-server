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
    use rim_core::conversation::model::{Conversation, ConversationId, PostingPolicy};
    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::{User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::auth::command::{
        CreateInviteCmd, ListInvitesQuery, LogoutCmd, LogoutOtherSessionsCmd,
        PreviewInviteTokenQuery, RevokeInviteCmd,
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
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
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
            dm_policy_repo: Arc::new(
                rim_core::dm_policy::repository::MockDmPolicyRepository::new(),
            ),
            default_workspace_repo: Arc::new(
                rim_core::default_workspace::repository::MockDefaultWorkspaceRepository::new(),
            ),
        }
    }

    /// Construct a default user for test helpers.
    fn make_user(id: UserId, username: &str) -> User {
        let now = Utc::now();
        User {
            id,
            username: username.to_string(),
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
            profile_locked_until: None,
            handle: rim_core::user::model::UserHandle::Local,
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
            kept: true,
            secret_hash: None,
            keep_key_sealed: None,
            reload_key_sealed: None,
            ws_closed_at: None,
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
        repo.expect_create_conversation_with_members().returning(
            |conv, _member_ids, _pending_ids| {
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
                        posting_policy: PostingPolicy::Everyone,
                        posting_allows_threads: true,
                        encrypted_at: None,
                    })
                })
            },
        );
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
                kept: true,
                secret_hash: None,
                keep_key_sealed: None,
                reload_key_sealed: None,
                ws_closed_at: None,
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
    async fn invite_emails_follow_the_invites_per_user_per_day_cap_when_invites_are_gone() {
        let (server_id, owner) = (ServerId::new(), UserId::new());
        // The day's invites count 0, as when their workspace was deleted.
        let svc = invite_service(
            server_id,
            owner,
            Some(0),
            rim_services::limits::InstanceLimits {
                invites_per_user_per_day: Some(2),
                ..Default::default()
            },
        );
        let emailed = |email: Option<&str>| CreateInviteCmd {
            actor: Actor::new(owner),
            server_id,
            expires_at: None,
            max_uses: 1,
            recipient_email: email.map(str::to_string),
            role: ServerMemberRole::Member,
        };
        for _ in 0..2 {
            svc.create_invite(emailed(Some("invitee@example.com")))
                .await
                .expect("below the cap");
        }
        let err = svc
            .create_invite(emailed(Some("invitee@example.com")))
            .await
            .expect_err("at the cap");
        assert!(
            matches!(&err, ServiceError::Validation(m)
                if m == "this instance allows 2 invite emails per user per day"),
            "{err:?}"
        );
        svc.create_invite(emailed(None))
            .await
            .expect("an invite without an email is not counted as one");
    }

    #[tokio::test]
    async fn invite_emails_are_not_counted_without_a_cap() {
        let (server_id, owner) = (ServerId::new(), UserId::new());
        let svc = invite_service(server_id, owner, None, Default::default());
        for _ in 0..3 {
            svc.create_invite(CreateInviteCmd {
                actor: Actor::new(owner),
                server_id,
                expires_at: None,
                max_uses: 1,
                recipient_email: Some("invitee@example.com".to_string()),
                role: ServerMemberRole::Member,
            })
            .await
            .expect("no cap");
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
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
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
        let svc = DefaultAuthService::new(
            make_auth_repos_with_registration(ur, sr, smr, sess_r, ir, cr, bt_repo),
            None,
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
        .with_preview_rate_limiter(preview_rl);

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

    /// Creates an invite for an owner of a workspace with the given expiry.
    async fn invite_expiring(
        expires_at: Option<chrono::DateTime<Utc>>,
    ) -> Result<Option<chrono::DateTime<Utc>>, ServiceError> {
        let (server_id, owner) = (ServerId::new(), UserId::new());
        let svc = invite_service(server_id, owner, None, Default::default());
        svc.create_invite(CreateInviteCmd {
            actor: Actor::new(owner),
            server_id,
            expires_at,
            max_uses: 1,
            recipient_email: None,
            role: ServerMemberRole::Member,
        })
        .await
        .map(|created| created.invite.expires_at)
    }

    /// An invite created without an expiry expires seven days later; a given expiry is kept.
    #[tokio::test]
    async fn create_invite_expires_in_a_week_by_default() {
        let expires_at = invite_expiring(None)
            .await
            .expect("created")
            .expect("an expiry");
        let week = Utc::now() + Duration::days(7);
        assert!((week - expires_at).num_seconds().abs() < 60, "{expires_at}");

        let tomorrow = Utc::now() + Duration::days(1);
        assert_eq!(
            invite_expiring(Some(tomorrow)).await.expect("created"),
            Some(tomorrow)
        );
    }

    /// An expiry in the past or more than thirty days out is `Validation`, and nothing is
    /// stored.
    #[tokio::test]
    async fn create_invite_refuses_past_and_far_expiries() {
        for (expires_at, why) in [
            (Utc::now() - Duration::minutes(1), "future"),
            (Utc::now() + Duration::days(31), "30 days"),
        ] {
            match invite_expiring(Some(expires_at)).await {
                Err(ServiceError::Validation(message)) => {
                    assert!(message.contains(why), "{message}")
                }
                other => panic!("expected a validation error, got {other:?}"),
            }
        }
    }

    fn stored_invite(server_id: ServerId, creator: UserId) -> Invite {
        Invite {
            id: InviteId::new(),
            server_id,
            created_by_user_id: creator,
            token_hash: "hash".to_string(),
            created_at: Utc::now(),
            expires_at: Some(Utc::now() + Duration::days(7)),
            max_uses: 3,
            use_count: 1,
            revoked_at: None,
            role: ServerMemberRole::Member,
        }
    }

    /// An auth service over `invite` (`None`: no invite stored), where the actor holds `role`
    /// in the invite's workspace (`None`: not a member). The store expects `revokes`
    /// revocations, each setting `revoked_at` if unset.
    fn revoke_service(
        invite: Option<Invite>,
        role: Option<ServerMemberRole>,
        revokes: usize,
    ) -> DefaultAuthService {
        let (user_repo, server_repo, mut server_member_repo, session_repo, mut invite_repo, conv) =
            repos();
        let stored = Arc::new(std::sync::Mutex::new(invite));
        let read = Arc::clone(&stored);
        invite_repo
            .expect_get_by_id()
            .returning(move |id| Ok(read.lock().unwrap().clone().filter(|i| i.id == id)));
        let written = Arc::clone(&stored);
        invite_repo
            .expect_revoke()
            .times(revokes)
            .returning(move |_, at| {
                if let Some(invite) = written.lock().unwrap().as_mut() {
                    invite.revoked_at.get_or_insert(at);
                }
                Ok(())
            });
        server_member_repo
            .expect_is_member()
            .returning(move |_, _| Ok(role.is_some()));
        server_member_repo
            .expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(role.map(|member_role| ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                }))
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
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
    }

    async fn revoke(
        svc: &DefaultAuthService,
        invite_id: InviteId,
    ) -> Result<rim_services::auth::dto::InviteDto, ServiceError> {
        svc.revoke_invite(RevokeInviteCmd {
            actor: Actor::new(UserId::new()),
            invite_id,
        })
        .await
    }

    /// The owner revokes an invite and gets it back revoked; revoking it again changes nothing
    /// and answers the same invite.
    #[tokio::test]
    async fn the_owner_revokes_an_invite_once() {
        let invite = stored_invite(ServerId::new(), UserId::new());
        let svc = revoke_service(Some(invite.clone()), Some(ServerMemberRole::Owner), 1);

        let revoked = revoke(&svc, invite.id).await.expect("revoked");
        assert_eq!(revoked.id, invite.id);
        let revoked_at = revoked.revoked_at.expect("revoked_at set");
        assert_eq!(
            revoked.state_at(Utc::now()),
            rim_services::auth::dto::InviteState::Revoked
        );

        let again = revoke(&svc, invite.id).await.expect("idempotent");
        assert_eq!(again.revoked_at, Some(revoked_at));
    }

    /// A moderator, a member or a guest of the invite's workspace is `PermissionDenied`; an
    /// outsider and an unknown invite are `NotFound`. None of them revokes anything.
    #[tokio::test]
    async fn only_the_owner_revokes_and_others_see_no_invite() {
        let invite = stored_invite(ServerId::new(), UserId::new());
        for role in [
            ServerMemberRole::Moderator,
            ServerMemberRole::Member,
            ServerMemberRole::Guest,
        ] {
            let svc = revoke_service(Some(invite.clone()), Some(role), 0);
            assert!(
                matches!(
                    revoke(&svc, invite.id).await,
                    Err(ServiceError::PermissionDenied)
                ),
                "{role:?}"
            );
        }

        let outsider = revoke_service(Some(invite.clone()), None, 0);
        assert!(matches!(
            revoke(&outsider, invite.id).await,
            Err(ServiceError::NotFound)
        ));
        let unknown = revoke_service(None, Some(ServerMemberRole::Owner), 0);
        assert!(matches!(
            revoke(&unknown, InviteId::new()).await,
            Err(ServiceError::NotFound)
        ));
    }

    /// An auth service whose actor holds `role` in every workspace and whose store lists
    /// `stored` for `lists` calls.
    fn list_service(
        role: Option<ServerMemberRole>,
        stored: Vec<Invite>,
        lists: usize,
    ) -> DefaultAuthService {
        let (user_repo, server_repo, mut server_member_repo, session_repo, mut invite_repo, conv) =
            repos();
        server_member_repo
            .expect_get_member()
            .returning(move |server_id, user_id| {
                Ok(role.map(|member_role| ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id,
                    member_role,
                    joined_at: Utc::now(),
                }))
            });
        invite_repo
            .expect_list_for_server()
            .times(lists)
            .returning(move |params| {
                assert_eq!(params.limit, 10);
                assert_eq!(params.dir, rim_core::repository::pagination::PageDir::After);
                Ok(rim_core::repository::pagination::Page::new(
                    stored.clone(),
                    None,
                    None,
                ))
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
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
    }

    fn list_query(server_id: ServerId) -> ListInvitesQuery {
        ListInvitesQuery {
            actor: Actor::new(UserId::new()),
            server_id,
            dir: rim_core::repository::pagination::PageDir::After,
            cursor: None,
            limit: 10,
        }
    }

    /// The owner lists the workspace's invites; anyone else is `PermissionDenied` and lists
    /// nothing.
    #[tokio::test]
    async fn only_the_owner_lists_invites() {
        let server_id = ServerId::new();
        let invite = stored_invite(server_id, UserId::new());
        let owner = list_service(Some(ServerMemberRole::Owner), vec![invite.clone()], 1);
        let page = owner
            .list_invites(list_query(server_id))
            .await
            .expect("listed");
        assert_eq!(page.items.len(), 1);
        assert_eq!(page.items[0].id, invite.id);
        assert_eq!(page.items[0].use_count, 1);
        assert_eq!(page.items[0].created_at, invite.created_at);

        for role in [
            Some(ServerMemberRole::Moderator),
            Some(ServerMemberRole::Member),
            Some(ServerMemberRole::Guest),
            None,
        ] {
            let svc = list_service(role, vec![invite.clone()], 0);
            assert!(
                matches!(
                    svc.list_invites(list_query(server_id)).await,
                    Err(ServiceError::PermissionDenied)
                ),
                "{role:?}"
            );
        }
    }
}
