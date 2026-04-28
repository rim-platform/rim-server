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
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::{NewUser, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::auth::command::{
        AcceptInviteCmd, BootstrapCmd, CreateInstanceInviteCmd, CreateInviteCmd, LogoutCmd,
        PreviewInviteTokenQuery, SignupOpenCmd, SignupWithBootstrapCmd,
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
        repo.expect_create_conversation_with_members().returning(
            |conv_type, server_id, title, _member_ids| {
                Box::pin(async move {
                    let now = chrono::Utc::now();
                    Ok(Conversation {
                        id: ConversationId::new(),
                        conversation_type: conv_type,
                        title,
                        topic: None,
                        created_at: now,
                        last_activity_at: now,
                        server_id,
                    })
                })
            },
        );
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
        );

        let err = svc
            .create_invite(CreateInviteCmd {
                actor: Actor::new(actor_user_id),
                server_id,
                expires_at: None,
                max_uses: 0,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
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
                    display_name: None,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
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
        bootstrap_repo.expect_mark_used().returning(|_, _| Ok(()));

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));
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
            })
            .await;
        assert!(res.is_ok(), "first use should succeed");

        // Second use: exhausted → InvalidToken.
        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "carol".to_string(),
                password: "s3cur3p4ss!".to_string(),
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
            })
            .await
            .unwrap();
        }

        let err = svc
            .signup_with_instance_invite(SignupWithInstanceInviteCmd {
                instance_invite_token: "raw_token".to_string(),
                username: "user4".to_string(),
                password: "s3cur3p4ss!".to_string(),
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
            })
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::Validation(ref m) if m.contains("max_uses")),
            "got: {err:?}"
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
        bootstrap_repo.expect_mark_used().returning(|_, _| Ok(()));

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
                password: "short".to_string(), // < 8 chars
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
        // try_consume / mark_used must NOT be called (read-only).
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
    /// `previewInviteToken` for a valid bootstrap token returns `INSTANCE` type.
    /// (FE collapses bootstrap+instance into the same branch.)
    #[tokio::test]
    async fn preview_invite_token_bootstrap_returns_instance_type() {
        let token_id = BootstrapTokenId::new();
        let bt = make_bootstrap_token(token_id);
        let expires_at = bt.expires_at;

        let mut bootstrap_repo = MockBootstrapTokenRepository::new();
        bootstrap_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(bt.clone())));
        // mark_used must NOT be called.
        bootstrap_repo.expect_mark_used().never();

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
        assert_eq!(res.token_type, InviteTokenType::Instance);
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
}
