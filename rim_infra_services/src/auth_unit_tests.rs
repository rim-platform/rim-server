#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use chrono::Duration;
    use rim_core::auth::model::{Invite, InviteId, Session, SessionId};
    use rim_core::repository::error::{RepoError, RepoErrorKind};
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::{NewUser, User, UserId};
    use rim_services::auth::service::AuthService;
    use rim_services::{Actor, ServiceError};

    use crate::auth::DefaultAuthService;
    use crate::test_support::auth::{AuthReposPorts, MockInviteRepo, MockServerMemberRepo, MockServerRepo, MockSessionRepo, MockUserRepo};

    fn repos() -> (
        MockUserRepo,
        MockServerRepo,
        MockServerMemberRepo,
        MockSessionRepo,
        MockInviteRepo,
    ) {
        (
            MockUserRepo::new(),
            MockServerRepo::new(),
            MockServerMemberRepo::new(),
            MockSessionRepo::new(),
            MockInviteRepo::new(),
        )
    }

    #[tokio::test]
    async fn bootstrap_denies_if_secret_missing() {
        let (user_repo, server_repo, server_member_repo, session_repo, invite_repo) = repos();

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            None,
            Duration::hours(1),
        );

        let err = svc
            .bootstrap("secret", "u", "p", "s")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn bootstrap_denies_if_users_exist() {
        let (mut user_repo, server_repo, server_member_repo, session_repo, invite_repo) = repos();
        user_repo.expect_any_users_exist().returning(|| Ok(true));

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let err = svc
            .bootstrap("secret", "u", "p", "s")
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

        let (user_repo, server_repo, server_member_repo, mut session_repo, invite_repo) = repos();
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

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let err = svc
            .logout(Actor::new(actor_user_id), session_id)
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn create_invite_validates_max_uses() {
        let server_id = ServerId::new();
        let actor_user_id = UserId::new();

        let (user_repo, server_repo, server_member_repo, session_repo, invite_repo) = repos();
        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let err = svc
            .create_invite(Actor::new(actor_user_id), server_id, None, 0)
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

        let (mut user_repo, server_repo, server_member_repo, session_repo, mut invite_repo) = repos();
        user_repo.expect_find_by_username().returning(move |_u| {
            Ok(Some(User {
                id: existing_user_id,
                username: "taken".to_string(),
                password_hash: "hash".to_string(),
                created_at: now,
                updated_at: now,
                last_login_at: now,
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

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let err = svc
            .accept_invite("token", "taken", "pw")
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Conflict(m) if m.contains("username")));
    }

    #[tokio::test]
    async fn bootstrap_maps_repo_errors_to_service_error() {
        let now = chrono::Utc::now();

        let (mut user_repo, server_repo, server_member_repo, session_repo, invite_repo) = repos();
        user_repo.expect_any_users_exist().returning(|| Ok(false));
        user_repo.expect_create_user().returning(move |_new: NewUser| {
            Err(RepoError::with_message(RepoErrorKind::Conflict, "duplicate"))
        });

        let mut server_repo = server_repo;
        server_repo.expect_create_server().never();
        let mut server_member_repo = server_member_repo;
        server_member_repo.expect_add_member().never();
        let mut session_repo = session_repo;
        session_repo.expect_create_session().never();

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let err = svc
            .bootstrap("secret", "u", "p", "s")
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

        let (mut user_repo, mut server_repo, mut server_member_repo, mut session_repo, invite_repo) = repos();

        user_repo.expect_any_users_exist().returning(|| Ok(false));
        user_repo.expect_create_user().returning(move |new: NewUser| {
            assert_eq!(new.username, "u");
            Ok(User {
                id: user_id,
                username: new.username,
                password_hash: new.password,
                created_at: now,
                updated_at: now,
                last_login_at: now,
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

        let svc = DefaultAuthService::new_for_tests(
            AuthReposPorts {
                user_repo: Arc::new(user_repo),
                server_repo: Arc::new(server_repo),
                server_member_repo: Arc::new(server_member_repo),
                session_repo: Arc::new(session_repo),
                invite_repo: Arc::new(invite_repo),
            },
            Some("secret".to_string()),
            Duration::hours(1),
        );

        let res = svc.bootstrap("secret", "u", "p", "s").await.unwrap();
        assert_eq!(res.user_id, user_id);
        assert_eq!(res.server_id, server_id);
        assert_eq!(res.session.user_id, user_id);
        assert_eq!(res.session.session_id, session_id);
        assert!(res.session_token.starts_with("rim_sess_"));
        assert!(res.csrf_token.starts_with("rim_csrf_"));
    }
}
