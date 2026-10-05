#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::{Duration, Utc};
    use rim_core::auth::model::{
        BootstrapToken, BootstrapTokenId, InstanceInvite, InstanceInviteId, Invite, InviteId,
        Session, SessionId,
    };
    use rim_core::auth::repository::{
        MockBootstrapTokenRepository, MockInstanceInviteRepository, MockInviteRepository,
        MockSessionRepository,
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
        AcceptInviteCmd, BootstrapCmd, CreateInstanceInviteCmd, CreateInviteCmd, LoginCmd,
        LogoutCmd, PreviewInviteTokenQuery, SignupOpenCmd, SignupWithBootstrapCmd,
        SignupWithInstanceInviteCmd,
    };
    use rim_services::auth::service::AuthService;
    use rim_services::{Actor, ServiceError};

    use rim_services::events::{NoopBroadcaster, Observers};

    use crate::AuthRepos;
    use crate::auth::DefaultAuthService;
    use crate::registration_rate_limiter::RegistrationRateLimiter;

    fn noop_observers() -> Observers {
        Observers::new(Arc::new(NoopBroadcaster) as Arc<dyn rim_services::events::Broadcaster>)
    }

    /// Build a complete set of mock repositories with the new registration repos included.
    ///
    /// The `bootstrap_token_repo` and `instance_invite_repo` are returned as separate values
    /// so callers can configure expectations on them.
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

    /// Build `AuthRepos` from the six standard repos plus empty bootstrap/instance repos.
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
            bootstrap_token_repo: Arc::new(MockBootstrapTokenRepository::new()),
            instance_invite_repo: Arc::new(MockInstanceInviteRepository::new()),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        }
    }

    /// Build `AuthRepos` with explicit bootstrap token and instance invite repos.
    #[allow(clippy::too_many_arguments)]
    fn make_auth_repos_with_registration(
        user_repo: MockUserRepository,
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        session_repo: MockSessionRepository,
        invite_repo: MockInviteRepository,
        conversation_repo: MockConversationRepository,
        bootstrap_token_repo: MockBootstrapTokenRepository,
        instance_invite_repo: MockInstanceInviteRepository,
    ) -> AuthRepos {
        AuthRepos {
            user_repo: Arc::new(user_repo),
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            session_repo: Arc::new(session_repo),
            invite_repo: Arc::new(invite_repo),
            bootstrap_token_repo: Arc::new(bootstrap_token_repo),
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
            password_hash: "hash".to_string(),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: format!("{}@local.invalid", username.to_lowercase()),
            timezone: None,
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
        }
    }

    /// Construct a valid bootstrap token in the future.
    fn make_bootstrap_token(id: BootstrapTokenId) -> BootstrapToken {
        let now = Utc::now();
        BootstrapToken {
            id,
            email: "test@example.com".to_string(),
            token_hash: "hash123".to_string(),
            created_at: now,
            expires_at: now + Duration::minutes(15),
            used_at: None,
            consumed_by_user_id: None,
            revoked_at: None,
        }
    }

    /// Construct a valid instance invite in the future.
    fn make_instance_invite(id: InstanceInviteId, max_uses: i32, use_count: i32) -> InstanceInvite {
        let now = Utc::now();
        InstanceInvite {
            id,
            created_by_user_id: UserId::new(),
            token_hash: "invite_hash".to_string(),
            created_at: now,
            expires_at: now + Duration::days(7),
            max_uses,
            use_count,
            revoked_at: None,
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
                    })
                })
            });
        repo
    }

    #[tokio::test]
    async fn bootstrap_denies_if_secret_missing() {
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
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .bootstrap(BootstrapCmd {
                secret: "secret".to_string(),
                username: "u".to_string(),
                password: "p".to_string(),
                server_name: "s".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn bootstrap_denies_if_users_exist() {
        let (
            mut user_repo,
            server_repo,
            server_member_repo,
            session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();
        user_repo.expect_any_users_exist().returning(|| Ok(true));

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
            Some("secret".to_string()),
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .bootstrap(BootstrapCmd {
                secret: "secret".to_string(),
                username: "u".to_string(),
                password: "p".to_string(),
                server_name: "s".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
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
            Some("secret".to_string()),
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
            Some("secret".to_string()),
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

    /// Track 1.2: an invite grants `Member` or `Guest`, never a moderating role.
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
                Some("secret".to_string()),
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
            Some("secret".to_string()),
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
            Some("secret".to_string()),
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
            Some("secret".to_string()),
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
            Some("secret".to_string()),
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
                password_hash: "hash".to_string(),
                created_at: now,
                updated_at: now,
                last_login_at: Some(now),
                display_name: None,
                avatar_url: None,
                status_text: None,
                bio: None,
                email: "taken@local.invalid".to_string(),
                timezone: None,
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
            Some("secret".to_string()),
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
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(m) if m.contains("username")));
    }

    #[tokio::test]
    async fn bootstrap_maps_repo_errors_to_service_error() {
        let now = chrono::Utc::now();

        let (
            mut user_repo,
            mut server_repo,
            mut server_member_repo,
            mut session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();
        user_repo.expect_any_users_exist().returning(|| Ok(false));
        user_repo
            .expect_create_user()
            .returning(move |_new: NewUser| {
                Err(RepoError::with_message(
                    RepoErrorKind::Conflict,
                    "duplicate",
                ))
            });

        server_repo.expect_create_server().never();
        server_member_repo.expect_add_member().never();
        session_repo.expect_create_session().never();

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
            Some("secret".to_string()),
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let err = svc
            .bootstrap(BootstrapCmd {
                secret: "secret".to_string(),
                username: "testuser".to_string(),
                password: "password123".to_string(),
                server_name: "s".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(m) if m == "duplicate"));

        let _ = now;
    }

    #[tokio::test]
    async fn bootstrap_happy_path_calls_repos() {
        let now = chrono::Utc::now();
        let server_id = ServerId::new();
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let (
            mut user_repo,
            mut server_repo,
            mut server_member_repo,
            mut session_repo,
            invite_repo,
            conversation_repo,
        ) = repos();

        user_repo.expect_any_users_exist().returning(|| Ok(false));
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                assert_eq!(new.username, "testuser");
                Ok(User {
                    id: user_id,
                    username: new.username,
                    password_hash: new.password,
                    created_at: now,
                    updated_at: now,
                    last_login_at: Some(now),
                    display_name: new.display_name,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    email: new.email,
                    timezone: None,
                })
            });

        server_repo.expect_create_server().returning(move |name| {
            assert_eq!(name, "s");
            Ok(Server {
                id: server_id,
                name: name.to_string(),
                created_at: now,
                updated_at: now,
            })
        });

        server_member_repo
            .expect_add_member()
            .returning(move |sid, uid, role| {
                assert_eq!(sid, server_id);
                assert_eq!(uid, user_id);
                assert_eq!(role, ServerMemberRole::Owner);
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: role,
                    joined_at: now,
                })
            });

        session_repo.expect_create_session().returning(move |new| {
            assert_eq!(new.user_id, user_id);
            Ok(Session {
                id: session_id,
                user_id: new.user_id,
                token_hash: new.token_hash,
                created_at: now,
                expires_at: new.expires_at,
                last_seen_at: None,
                revoked_at: None,
                user_agent: new.user_agent,
                ip: new.ip,
            })
        });

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
            Some("secret".to_string()),
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        );

        let res = svc
            .bootstrap(BootstrapCmd {
                secret: "secret".to_string(),
                username: "testuser".to_string(),
                password: "password123".to_string(),
                server_name: "s".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(res.user_id, user_id);
        assert_eq!(res.server_id, server_id);
        assert_eq!(res.session.user_id, user_id);
        assert_eq!(res.session.session_id, session_id);
        assert!(res.session_token.starts_with("rim_sess_"));
        assert!(res.csrf_token.starts_with("rim_csrf_"));
    }

    // ── §8.1 Registration unit tests ─────────────────────────────────────────

    /// Helper: build a service with only the repos configured for a given test.
    #[allow(clippy::too_many_arguments)]
    fn svc_with_registration(
        user_repo: MockUserRepository,
        server_repo: MockServerRepository,
        server_member_repo: MockServerMemberRepository,
        session_repo: MockSessionRepository,
        invite_repo: MockInviteRepository,
        conversation_repo: MockConversationRepository,
        bootstrap_repo: MockBootstrapTokenRepository,
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
                bootstrap_repo,
                instance_invite_repo,
            ),
            None,
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(crate::email::LogEmailSender),
            None,
        )
    }

    // ── Test 1 ──────────────────────────────────────────────────────────────
    /// A consumed bootstrap token (`used_at IS NOT NULL`) is rejected with `InvalidToken`.
    #[tokio::test]
    async fn bootstrap_token_single_use_enforced() {
        let token_id = BootstrapTokenId::new();
        let mut bt = make_bootstrap_token(token_id);
        bt.used_at = Some(Utc::now()); // mark as already consumed

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    // ── Test 2 ──────────────────────────────────────────────────────────────
    /// A bootstrap token with `expires_at < now()` is rejected with `ExpiredToken`.
    #[tokio::test]
    async fn bootstrap_token_expired_rejected() {
        let token_id = BootstrapTokenId::new();
        let mut bt = make_bootstrap_token(token_id);
        bt.expires_at = Utc::now() - Duration::seconds(1); // already expired

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::ExpiredToken), "got: {err:?}");
    }

    // ── Test 3 ──────────────────────────────────────────────────────────────
    /// A revoked bootstrap token (`revoked_at IS NOT NULL`) returns `InvalidToken`,
    /// NOT `ExpiredToken` — revocation must be indistinguishable from "not found".
    #[tokio::test]
    async fn bootstrap_token_revoked_rejected() {
        let token_id = BootstrapTokenId::new();
        let mut bt = make_bootstrap_token(token_id);
        bt.revoked_at = Some(Utc::now());

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        // Must be InvalidToken, NOT ExpiredToken — spec §4.5 indistinguishability rule.
        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    // ── Test 4 ──────────────────────────────────────────────────────────────
    /// A consumed bootstrap token collapses to `InvalidToken` (same code as missing).
    /// This is test 3 from a different angle but spec explicitly calls it out.
    #[tokio::test]
    async fn bootstrap_token_consumed_rejected_indistinguishable_from_invalid() {
        let token_id = BootstrapTokenId::new();
        let mut bt = make_bootstrap_token(token_id);
        bt.used_at = Some(Utc::now());
        bt.consumed_by_user_id = Some(UserId::new());

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    // ── Test 5 ──────────────────────────────────────────────────────────────
    /// Successful bootstrap token consumption creates a user with no server_member row.
    #[tokio::test]
    async fn bootstrap_token_creates_user_with_no_workspace() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(|_, _| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, mut smr, _, ir, cr) = repos();
        // server_member::add_member must NEVER be called for bootstrap signup.
        smr.expect_add_member().never();

        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            session_repo,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let res = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
        assert!(
            res.server_id.is_none(),
            "should have no server_id for bootstrap signup"
        );
    }

    // ── Test 6 ──────────────────────────────────────────────────────────────
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
            MockBootstrapTokenRepository::new(),
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
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    // ── Test 7 ──────────────────────────────────────────────────────────────
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
            MockBootstrapTokenRepository::new(),
            instance_invite_repo,
        );

        for _ in 0..3 {
            svc.signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "user".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "user@example.com".to_string(),
                display_name: None,
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
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::InvalidToken),
            "4th call should be limited, got: {err:?}"
        );
    }

    // ── Test 8 ──────────────────────────────────────────────────────────────
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
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            MockBootstrapTokenRepository::new(),
            instance_invite_repo,
        );

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::ExpiredToken), "got: {err:?}");
    }

    // ── Test 9 ──────────────────────────────────────────────────────────────
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
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            MockBootstrapTokenRepository::new(),
            instance_invite_repo,
        );

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::InvalidToken), "got: {err:?}");
    }

    // ── Test 10 ─────────────────────────────────────────────────────────────
    /// `createInstanceInvite` with `max_uses > 50` returns `Validation`.
    #[tokio::test]
    async fn instance_invite_max_uses_cap_enforced() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id: actor_id,
                    member_role: ServerMemberRole::Owner,
                    joined_at: Utc::now(),
                }))
            });

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_| {
                Ok(vec![Server {
                    id: server_id,
                    name: "ws".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let (ur, _, _, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            server_repo,
            server_member_repo,
            sess_r,
            ir,
            cr,
            MockBootstrapTokenRepository::new(),
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor_id),
                expires_at: Some(Utc::now() + Duration::days(7)),
                max_uses: Some(51),
                recipient_email: None,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("max_uses")),
            "got: {err:?}"
        );
    }

    #[tokio::test]
    async fn create_instance_invite_with_recipient_email_sends_email() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_id))));

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_| {
                Ok(vec![Server {
                    id: server_id,
                    name: "ws".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo.expect_create().returning(|new| {
            Ok(InstanceInvite {
                id: InstanceInviteId::new(),
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .withf(|msg| msg.to == "invitee@example.com" && msg.body_text.contains("/join/"))
            .times(1)
            .returning(|_| Ok(()));

        let (ur, _, _, sess_r, ir, cr) = repos();
        let svc = DefaultAuthService::new(
            make_auth_repos_with_registration(
                ur,
                server_repo,
                server_member_repo,
                sess_r,
                ir,
                cr,
                MockBootstrapTokenRepository::new(),
                instance_invite_repo,
            ),
            None,
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor_id),
                expires_at: Some(Utc::now() + Duration::days(7)),
                max_uses: Some(1),
                recipient_email: Some("invitee@example.com".to_string()),
            })
            .await
            .unwrap();
        assert!(!result.invite_token.is_empty());
    }

    #[tokio::test]
    async fn create_instance_invite_without_recipient_email_does_not_send() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_id))));

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_| {
                Ok(vec![Server {
                    id: server_id,
                    name: "ws".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo.expect_create().returning(|new| {
            Ok(InstanceInvite {
                id: InstanceInviteId::new(),
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender.expect_send().never();

        let (ur, _, _, sess_r, ir, cr) = repos();
        let svc = DefaultAuthService::new(
            make_auth_repos_with_registration(
                ur,
                server_repo,
                server_member_repo,
                sess_r,
                ir,
                cr,
                MockBootstrapTokenRepository::new(),
                instance_invite_repo,
            ),
            None,
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor_id),
                expires_at: Some(Utc::now() + Duration::days(7)),
                max_uses: Some(1),
                recipient_email: None,
            })
            .await
            .unwrap();
        assert!(!result.invite_token.is_empty());
    }

    /// Email delivery is best-effort: an `EmailSender` failure must not fail the mutation, and
    /// the instance invite (with its usable token) must still be created and returned.
    #[tokio::test]
    async fn create_instance_invite_send_failure_is_non_fatal() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| Ok(Some(make_owner_member(server_id, actor_id))));

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_| {
                Ok(vec![Server {
                    id: server_id,
                    name: "ws".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo.expect_create().returning(|new| {
            Ok(InstanceInvite {
                id: InstanceInviteId::new(),
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .returning(|_| Err(anyhow::anyhow!("smtp down")));

        let (ur, _, _, sess_r, ir, cr) = repos();
        let svc = DefaultAuthService::new(
            make_auth_repos_with_registration(
                ur,
                server_repo,
                server_member_repo,
                sess_r,
                ir,
                cr,
                MockBootstrapTokenRepository::new(),
                instance_invite_repo,
            ),
            None,
            None,
            Duration::hours(1),
            noop_observers(),
            Arc::new(email_sender),
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor_id),
                expires_at: Some(Utc::now() + Duration::days(7)),
                max_uses: Some(1),
                recipient_email: Some("invitee@example.com".to_string()),
            })
            .await
            .unwrap();
        assert!(
            !result.invite_token.is_empty(),
            "invite token must still be returned even when the email send fails"
        );
    }

    // ── Test 11 ─────────────────────────────────────────────────────────────
    /// `createInstanceInvite` with no `expires_at` defaults to `now() + 7 days`.
    #[tokio::test]
    async fn instance_invite_default_ttl_seven_days() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let invite_id = InstanceInviteId::new();

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_, _| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id: actor_id,
                    member_role: ServerMemberRole::Owner,
                    joined_at: Utc::now(),
                }))
            });

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_| {
                Ok(vec![Server {
                    id: server_id,
                    name: "ws".to_string(),
                    created_at: Utc::now(),
                    updated_at: Utc::now(),
                }])
            });

        let mut instance_invite_repo = MockInstanceInviteRepository::new();
        instance_invite_repo
            .expect_create()
            .returning(move |new| Ok(make_instance_invite(invite_id, new.max_uses, 0)));

        let (ur, _, _, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            server_repo,
            server_member_repo,
            sess_r,
            ir,
            cr,
            MockBootstrapTokenRepository::new(),
            instance_invite_repo,
        );

        let before = Utc::now();
        let res = svc
            .create_instance_invite(CreateInstanceInviteCmd {
                actor: Actor::new(actor_id),
                expires_at: None, // should default to 7 days
                max_uses: Some(1),
                recipient_email: None,
            })
            .await
            .unwrap();

        let expected_min = before + Duration::days(6) + Duration::hours(23);
        let expected_max = Utc::now() + Duration::days(7) + Duration::seconds(5);
        assert!(
            res.invite.expires_at >= expected_min && res.invite.expires_at <= expected_max,
            "default expires_at should be ~7 days, got {:?}",
            res.invite.expires_at
        );
    }

    // ── Test 12 ─────────────────────────────────────────────────────────────
    /// `signup_open` with no tokens and valid creds succeeds in open mode.
    #[tokio::test]
    async fn signup_open_accepts_no_token() {
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
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
            MockBootstrapTokenRepository::new(),
            MockInstanceInviteRepository::new(),
        );

        let res = svc
            .signup_open(SignupOpenCmd {
                username: "carol".to_string(),
                password: "s3cur3p4ss!".to_string(),
                remote_ip: "1.2.3.4".to_string(),
                email: "carol@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
        assert!(res.server_id.is_none());
    }

    // ── Test 13 ─────────────────────────────────────────────────────────────
    /// Open-mode signup rate-limited after threshold.
    ///
    /// The signup rate limiter is checked inside `signup_open` (spec §4.4).
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

    // ── Test 14 ─────────────────────────────────────────────────────────────
    /// signup with username already taken returns `Conflict`.
    #[tokio::test]
    async fn signup_username_already_taken_returns_conflict() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let existing_id = UserId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(|_, _| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(make_user(existing_id, "alice"))));

        let (_, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Conflict(_)), "got: {err:?}");
    }

    // ── Test 15 ─────────────────────────────────────────────────────────────
    /// signup with password shorter than 8 chars returns `Validation`.
    #[tokio::test]
    async fn signup_password_too_short_returns_validation() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "short".to_string(), // < 8 chars — validation fires before email check
                email: "alice@example.com".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("password")),
            "got: {err:?}"
        );
    }

    // ── Test 16 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` for a valid workspace invite returns `WORKSPACE` type
    /// with server name and inviter display name; underlying row is NOT mutated.
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

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(|_| Ok(None)); // not a bootstrap token

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

        let (_, _, smr, sess_r, _, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            server_repo,
            smr,
            sess_r,
            invite_repo,
            cr,
            bootstrap_repo,
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
    }

    // ── Test 17 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` for a valid instance invite returns `INSTANCE` type
    /// with `expiresAt` set and `workspace` null; row not mutated.
    #[tokio::test]
    async fn preview_invite_token_instance_returns_minimal_payload() {
        let invite_id = InstanceInviteId::new();
        let invite = make_instance_invite(invite_id, 5, 0);

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
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
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            instance_invite_repo,
        );

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

    // ── Test 18 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` for a valid bootstrap token returns `Bootstrap` type
    /// so the frontend can route to the `bootstrapToken` input field on
    /// `signupWithInvite`. `expiresAt` is set; `workspace` is null.
    #[tokio::test]
    async fn preview_invite_token_bootstrap_returns_bootstrap_type() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let expires_at = bt.expires_at;

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        // try_mark_used must NOT be called (read-only).
        bootstrap_repo.expect_try_mark_used().never();

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let res = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_boot_token".to_string(),
                remote_ip: "1.2.3.4".to_string(),
            })
            .await
            .unwrap();

        use rim_services::auth::dto::InviteTokenType;
        assert_eq!(res.token_type, InviteTokenType::Bootstrap);
        assert_eq!(res.expires_at, Some(expires_at));
        assert!(res.workspace.is_none());
    }

    // ── Test 19 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` on an expired bootstrap token returns `ExpiredToken`.
    #[tokio::test]
    async fn preview_invite_token_expired_returns_expired_token() {
        let token_id = BootstrapTokenId::new();
        let mut bt = make_bootstrap_token(token_id);
        bt.expires_at = Utc::now() - Duration::seconds(1);

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_boot_token".to_string(),
                remote_ip: "1.2.3.4".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::ExpiredToken), "got: {err:?}");
    }

    // ── Test 20 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` on a missing / consumed / revoked token all collapse to
    /// `InvalidToken` (three sub-cases).
    #[tokio::test]
    async fn preview_invite_token_invalid_returns_invalid_token() {
        // Sub-case A: missing (no row in any table).
        {
            let mut bt_repo = MockBootstrapTokenRepository::new();
            bt_repo.expect_get_by_token_hash().returning(|_| Ok(None));
            let mut ii_repo = MockInstanceInviteRepository::new();
            ii_repo.expect_get_by_token_hash().returning(|_| Ok(None));
            let mut ir = MockInviteRepository::new();
            ir.expect_get_by_token_hash().returning(|_| Ok(None));

            let (ur, sr, smr, sess_r, _, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, bt_repo, ii_repo);

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

        // Sub-case B: consumed bootstrap token.
        {
            let token_id = BootstrapTokenId::new();
            let mut bt = make_bootstrap_token(token_id);
            bt.used_at = Some(Utc::now());

            let mut bt_repo = MockBootstrapTokenRepository::new();
            bt_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(bt.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(
                ur,
                sr,
                smr,
                sess_r,
                ir,
                cr,
                bt_repo,
                MockInstanceInviteRepository::new(),
            );

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

            let mut bt_repo = MockBootstrapTokenRepository::new();
            bt_repo.expect_get_by_token_hash().returning(|_| Ok(None));
            let mut ii_repo = MockInstanceInviteRepository::new();
            ii_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(invite.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(ur, sr, smr, sess_r, ir, cr, bt_repo, ii_repo);

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

    // ── Test 21 ─────────────────────────────────────────────────────────────
    /// `previewInviteToken` rate-limited after 30 calls from the same IP within
    /// a 5-minute window; exactly 30 calls succeed.
    #[tokio::test]
    async fn preview_invite_token_rate_limited_after_threshold() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);

        let mut bt_repo = MockBootstrapTokenRepository::new();
        bt_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let preview_rl = RegistrationRateLimiter::preview_default(); // 30/5min

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = DefaultAuthService::with_rate_limiters(
            make_auth_repos_with_registration(
                ur,
                sr,
                smr,
                sess_r,
                ir,
                cr,
                bt_repo,
                MockInstanceInviteRepository::new(),
            ),
            None,
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
                    token: "raw_boot_token".to_string(),
                    remote_ip: ip.to_string(),
                })
                .await;
            assert!(res.is_ok(), "call {i} should succeed, got: {res:?}");
        }

        let err = svc
            .preview_invite_token(PreviewInviteTokenQuery {
                token: "raw_boot_token".to_string(),
                remote_ip: ip.to_string(),
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::RateLimited { .. }),
            "31st call should be rate-limited, got: {err:?}"
        );
    }

    // ── §13.6 Amendment tests ────────────────────────────────────────────────

    // ── Test 33 ─────────────────────────────────────────────────────────────
    /// Happy path: valid email + displayName are persisted correctly.
    ///
    /// Input `"  Foo@Bar.COM "` must be stored as `"foo@bar.com"` (trimmed + lowercased).
    /// `display_name` must be stored as the trimmed input value.
    #[tokio::test]
    async fn signup_with_email_happy_path() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(|_, _| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new: NewUser| {
                // Verify normalisation happened before the repo is called.
                assert_eq!(
                    new.email, "foo@bar.com",
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
                })
            });

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
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let res = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "  Foo@Bar.COM ".to_string(), // whitespace + mixed case
                display_name: Some("Alice Smith".to_string()),
            })
            .await
            .unwrap();

        assert_eq!(res.user_id, user_id);
    }

    // ── Test 34 ─────────────────────────────────────────────────────────────
    /// Duplicate email (case-insensitive) returns `EmailTaken`.
    ///
    /// Pre-seeds a user with `email = "x@y.z"`; signup with `"X@Y.Z"` must
    /// return `ServiceError::EmailTaken`, verifying case-insensitive uniqueness.
    #[tokio::test]
    async fn signup_duplicate_email_returns_email_taken() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let existing_id = UserId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        // try_mark_used must NOT be called when signup fails.
        bootstrap_repo.expect_try_mark_used().never();

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        // find_by_email returns Some for the duplicate.
        user_repo
            .expect_find_by_email()
            .returning(move |_| Ok(Some(make_user(existing_id, "other_user"))));
        user_repo.expect_create_user().never();

        let (_, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            user_repo,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "bob".to_string(),
                password: "s3cur3p4ss!".to_string(),
                // Mixed-case version of the pre-seeded email; normalises to "x@y.zz".
                email: "X@Y.ZZ".to_string(),
                display_name: None,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::EmailTaken), "got: {err:?}");
    }

    // ── Test 35 ─────────────────────────────────────────────────────────────
    /// Malformed email input returns `Validation`.
    ///
    /// Parametrized over the cases from spec §13.3:
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
            let token_id = BootstrapTokenId::new();
            let bt = make_bootstrap_token(token_id);

            let mut bootstrap_repo = MockBootstrapTokenRepository::new();
            bootstrap_repo
                .expect_get_by_token_hash()
                .returning(move |_| Ok(Some(bt.clone())));

            let (ur, sr, smr, sess_r, ir, cr) = repos();
            let svc = svc_with_registration(
                ur,
                sr,
                smr,
                sess_r,
                ir,
                cr,
                bootstrap_repo,
                MockInstanceInviteRepository::new(),
            );

            let err = svc
                .signup_with_bootstrap(SignupWithBootstrapCmd {
                    bootstrap_token: "raw_token".to_string(),
                    username: "alice".to_string(),
                    password: "s3cur3p4ss!".to_string(),
                    email: bad_email.to_string(),
                    display_name: None,
                })
                .await
                .unwrap_err();

            assert!(
                matches!(err, ServiceError::Validation(_)),
                "email {bad_email:?} should produce Validation, got: {err:?}"
            );
        }
    }

    // ── Test 36 ─────────────────────────────────────────────────────────────
    /// `display_name: None` falls back to `username` on the persisted user.
    #[tokio::test]
    async fn signup_displayname_omitted_falls_back_to_username() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(|_, _| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
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
                })
            });

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
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        svc.signup_with_bootstrap(SignupWithBootstrapCmd {
            bootstrap_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();
    }

    // ── Test 37 ─────────────────────────────────────────────────────────────
    /// `display_name: Some("   ")` (blank after trim) falls back to `username`.
    ///
    /// Must NOT return a `Validation` error — blank means "use my username."
    #[tokio::test]
    async fn signup_displayname_blank_falls_back_to_username() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(|_, _| Ok(true));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
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
                })
            });

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
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        svc.signup_with_bootstrap(SignupWithBootstrapCmd {
            bootstrap_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: Some("   ".to_string()), // blank after trim → falls back to username
        })
        .await
        .unwrap();
    }

    // ── Test 38 ─────────────────────────────────────────────────────────────
    /// `display_name` of 81 chars (post-trim) returns `Validation`.
    #[tokio::test]
    async fn signup_displayname_too_long_returns_validation() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));

        let (ur, sr, smr, sess_r, ir, cr) = repos();
        let svc = svc_with_registration(
            ur,
            sr,
            smr,
            sess_r,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        );

        let long_name = "x".repeat(81); // 81 chars — one over the 80-char limit

        let err = svc
            .signup_with_bootstrap(SignupWithBootstrapCmd {
                bootstrap_token: "raw_token".to_string(),
                username: "alice".to_string(),
                password: "s3cur3p4ss!".to_string(),
                email: "alice@example.com".to_string(),
                display_name: Some(long_name),
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("display name")),
            "got: {err:?}"
        );
    }

    // ── C1 hardening ────────────────────────────────────────────────────────
    /// Simulate the double-spend race at the mock layer.
    ///
    /// `try_mark_used` returns `true` on the first call (token claimed) and
    /// `false` on all subsequent calls (already consumed). Two concurrent
    /// `signup_with_bootstrap` calls with the same raw token: exactly one must
    /// succeed and the other must fail with `InvalidToken`.
    ///
    /// Note: this is a mock-layer test because the race itself happens inside
    /// the DB (via the atomic UPDATE WHERE); the mock simulates the DB
    /// serialisation outcome. The real concurrent-DB test requires the `db-tests`
    /// feature and a live Postgres.
    #[tokio::test]
    async fn bootstrap_token_double_spend_rejected_by_atomic_mark_used() {
        use std::sync::atomic::{AtomicUsize, Ordering};

        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let user_id = UserId::new();
        let session_id = SessionId::new();

        // Counter shared across calls: first call gets true, subsequent get false.
        let call_count = Arc::new(AtomicUsize::new(0));
        let call_count_clone = Arc::clone(&call_count);

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        let bt_for_hash = bt.clone();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt_for_hash.clone())));
        bootstrap_repo
            .expect_try_mark_used()
            .returning(move |_, _| {
                // First invocation wins the race; subsequent ones lose.
                let prev = call_count_clone.fetch_add(1, Ordering::SeqCst);
                Ok(prev == 0)
            });

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
        user_repo.expect_find_by_email().returning(|_| Ok(None));
        user_repo
            .expect_create_user()
            .returning(move |new| Ok(make_user(user_id, &new.username)));

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_create_session()
            .returning(move |new| Ok(make_session(session_id, new.user_id)));

        let (_, sr, smr, _, ir, cr) = repos();
        let svc = Arc::new(svc_with_registration(
            user_repo,
            sr,
            smr,
            session_repo,
            ir,
            cr,
            bootstrap_repo,
            MockInstanceInviteRepository::new(),
        ));

        let cmd = || SignupWithBootstrapCmd {
            bootstrap_token: "raw_token".to_string(),
            username: "alice".to_string(),
            password: "s3cur3p4ss!".to_string(),
            email: "alice@example.com".to_string(),
            display_name: None,
        };

        let svc1 = Arc::clone(&svc);
        let svc2 = Arc::clone(&svc);
        let (r1, r2) = tokio::join!(
            svc1.signup_with_bootstrap(cmd()),
            svc2.signup_with_bootstrap(cmd()),
        );

        // Exactly one must succeed, the other must return InvalidToken.
        let successes = [r1.is_ok(), r2.is_ok()].iter().filter(|&&ok| ok).count();
        let invalid_token_failures = [&r1, &r2]
            .iter()
            .filter(|r| matches!(r, Err(ServiceError::InvalidToken)))
            .count();

        assert_eq!(
            successes, 1,
            "exactly one signup must succeed; got r1={r1:?} r2={r2:?}"
        );
        assert_eq!(
            invalid_token_failures, 1,
            "exactly one signup must fail with InvalidToken; got r1={r1:?} r2={r2:?}"
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

    // ─── login rate limiting (S-H3) ─────────────────────────────────────────

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
        user.password_hash = rim_auth::password::hash_password("correct-horse").unwrap();

        let mut user_repo = MockUserRepository::new();
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
    async fn login_is_rate_limited_per_ip_before_any_lookup() {
        let mut user_repo = MockUserRepository::new();
        // Only allowed attempts may reach the repository (and Argon2): two from the
        // throttled IP plus one from the unaffected IP.
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
        user_repo
            .expect_find_by_username()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Err(RepoError::with_message(RepoErrorKind::Internal, "db down")));
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
        user.password_hash = rim_auth::password::hash_password("correct-horse").unwrap();

        let mut user_repo = MockUserRepository::new();
        // Two guesses from the attacker, one attempt on another account from the same
        // address, then the owner's login; the throttled third guess never reaches the
        // repository.
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
        user.password_hash = rim_auth::password::hash_password("correct-horse").unwrap();
        let mut user_repo = MockUserRepository::new();
        // Exact match, like the real repository: usernames are case-sensitive.
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
        victim.password_hash = rim_auth::password::hash_password("victim-pass").unwrap();
        let mut attacker = make_user(UserId::new(), "Alice");
        attacker.password_hash = rim_auth::password::hash_password("attacker-pass").unwrap();

        let mut user_repo = MockUserRepository::new();
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
