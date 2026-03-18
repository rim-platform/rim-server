#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::user::model::{User, UserId};
    use rim_services::user::command::{GetMeQuery, GetUserQuery, UpdateMyProfileCmd};
    use rim_services::user::service::UserService;
    use rim_services::{Actor, ServiceError};

    use crate::test_support::user::{MockUserRepo, UserReposPorts};
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
        }
    }

    #[tokio::test]
    async fn get_me_returns_not_found_when_missing() {
        let actor_id = UserId::new();
        let mut user_repo = MockUserRepo::new();
        user_repo.expect_get_by_id().returning(|_id| Ok(None));

        let svc = DefaultUserService::new_for_tests(UserReposPorts {
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
        let mut user_repo = MockUserRepo::new();
        user_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(u.clone())));

        let svc = DefaultUserService::new_for_tests(UserReposPorts {
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
    async fn update_profile_is_validation_error_in_v0() {
        let user_repo = MockUserRepo::new();
        let svc = DefaultUserService::new_for_tests(UserReposPorts {
            user_repo: Arc::new(user_repo),
        });

        let err = svc
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(UserId::new()),
                display_name: None,
                status_text: None,
                avatar_url: None,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }
}
