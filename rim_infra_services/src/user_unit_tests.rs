#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::user::model::{UpdateProfilePatch, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
    use rim_services::user::service::UserService;
    use rim_services::{Actor, ServiceError};

    use crate::UserRepos;
    use crate::user::DefaultUserService;

    fn user(id: UserId) -> User {
        let now = chrono::Utc::now();
        User {
            id,
            username: "u".to_string(),
            password_hash: "h".to_string(),
            created_at: now,
            updated_at: now,
            last_login_at: Some(now),
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: "u@local.invalid".to_string(),
            timezone: None,
            principal_id: None,
        }
    }

    #[tokio::test]
    async fn get_me_returns_not_found_when_missing() {
        let actor_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_id().returning(|_id| Ok(None));

        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let err = svc
            .get_me(GetMeQuery {
                actor: Actor::new(actor_id),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }

    #[tokio::test]
    async fn get_user_returns_user() {
        let id = UserId::new();
        let u = user(id);
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(u.clone())));

        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let dto = svc
            .get_user(GetUserQuery {
                actor: Actor::new(UserId::new()),
                user_id: id,
            })
            .await
            .unwrap();
        assert_eq!(dto.id, id);
    }

    #[tokio::test]
    async fn update_profile_sets_fields() {
        let id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_update_profile()
            .returning(move |uid, patch: UpdateProfilePatch| {
                assert_eq!(uid, id);
                let mut u = user(id);
                if let Some(v) = patch.display_name {
                    u.display_name = v;
                }
                if let Some(v) = patch.status_text {
                    u.status_text = v;
                }
                if let Some(v) = patch.bio {
                    u.bio = v;
                }
                Ok(u)
            });

        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let dto = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(id),
                display_name: Some("Alice".to_string()),
                status_text: Some("busy".to_string()),
                avatar_url: None,
                bio: Some("hello".to_string()),
            })
            .await
            .unwrap();
        assert_eq!(dto.display_name, Some("Alice".to_string()));
    }

    #[tokio::test]
    async fn update_profile_rejects_long_display_name() {
        let user_repo = MockUserRepository::new();
        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let err = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(UserId::new()),
                display_name: Some("x".repeat(65)),
                status_text: None,
                avatar_url: None,
                bio: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn update_profile_rejects_external_avatar_url() {
        let user_repo = MockUserRepository::new();
        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let err = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(UserId::new()),
                display_name: None,
                status_text: None,
                avatar_url: Some("https://evil.com/avatar.png".to_string()),
                bio: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn update_profile_strips_control_chars() {
        let id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_update_profile()
            .returning(move |_uid, patch: UpdateProfilePatch| {
                // Control chars should be stripped
                let mut u = user(id);
                if let Some(v) = patch.display_name {
                    u.display_name = v;
                }
                Ok(u)
            });

        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let dto = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(id),
                display_name: Some("Al\x00ice\x07".to_string()),
                status_text: None,
                avatar_url: None,
                bio: None,
            })
            .await
            .unwrap();
        assert_eq!(dto.display_name, Some("Alice".to_string()));
    }
}
