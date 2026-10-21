#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::user::model::{UpdateProfilePatch, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::user::command::{GetMeQuery, UpdateMyProfileCmd};
    use rim_services::user::service::UserService;
    use rim_services::{Actor, ServiceError};

    use crate::UserRepos;
    use crate::user::DefaultUserService;

    fn user(id: UserId) -> User {
        let now = chrono::Utc::now();
        User {
            id,
            username: "u".to_string(),
            password_hash: Some("h".to_string()),
            created_at: now,
            updated_at: now,
            last_login_at: Some(now),
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: Some("u@local.invalid".to_string()),
            timezone: None,
            principal_id: None,
            deletion_scheduled_for: None,
            handle: rim_core::user::model::UserHandle::Local,
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
    async fn only_me_shows_a_pending_deletion() {
        let id = UserId::new();
        let at = chrono::Utc::now();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_id().returning(move |id| {
            Ok(Some(User {
                deletion_scheduled_for: Some(at),
                ..user(id)
            }))
        });
        user_repo.expect_get_by_ids().returning(move |ids| {
            Ok(ids
                .iter()
                .map(|id| User {
                    deletion_scheduled_for: Some(at),
                    ..user(*id)
                })
                .collect())
        });
        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        });

        let me = svc
            .get_me(GetMeQuery {
                actor: Actor::new(id),
            })
            .await
            .unwrap();
        assert_eq!(me.deletion_scheduled_for, Some(at));
        let others = svc.get_users_by_ids(&[id]).await.unwrap();
        assert_eq!(others[0].deletion_scheduled_for, None);
    }

    #[tokio::test]
    async fn handles_are_built_for_local_and_remote_users() {
        use rim_core::user::model::UserHandle;

        let local = UserId::new();
        let remote = UserId::new();
        let unknown = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_ids().returning(move |_| {
            Ok(vec![
                User {
                    username: "Kuba".to_string(),
                    ..user(local)
                },
                User {
                    username: "r_txDLbnipi5i3kCJtJwBOUixzv_MY8O".to_string(),
                    handle: UserHandle::Home {
                        username: "kuba".to_string(),
                        domain: "rim-platform.org".to_string(),
                    },
                    ..user(remote)
                },
                User {
                    handle: UserHandle::Unknown,
                    ..user(unknown)
                },
            ])
        });
        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        })
        .with_handle_domain(Some("chat.example".to_string()));
        let handles: Vec<Option<String>> = svc
            .get_users_by_ids(&[local, remote, unknown])
            .await
            .unwrap()
            .into_iter()
            .map(|u| u.handle)
            .collect();
        assert_eq!(
            handles,
            vec![
                Some("@kuba:chat.example".to_string()),
                Some("@kuba:rim-platform.org".to_string()),
                None,
            ]
        );
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

    #[tokio::test]
    async fn probation_ends_at_is_set_while_the_account_is_in_probation() {
        let id = UserId::new();
        let created = chrono::Utc::now();
        let service = |hours: Option<u32>, now| {
            DefaultUserService::new(UserRepos {
                user_repo: Arc::new(crate::probation_fixtures::users(vec![
                    crate::probation_fixtures::account(id, created, None),
                ])),
            })
            .with_limits(rim_services::limits::InstanceLimits {
                probation_hours: hours,
                ..Default::default()
            })
            .with_clock(crate::clock::Clock::fixed(now))
        };
        let me = || GetMeQuery {
            actor: Actor::new(id),
        };
        for (hours, now, expected) in [
            (
                Some(24),
                created + chrono::Duration::hours(1),
                Some(created + chrono::Duration::hours(24)),
            ),
            (Some(24), created + chrono::Duration::hours(24), None),
            (None, created, None),
        ] {
            assert_eq!(
                service(hours, now).probation_ends_at(me()).await.unwrap(),
                expected,
                "{hours:?}"
            );
        }
    }
    /// A user service reserving `rim` over one account named `username`.
    fn reserving_svc(id: UserId, username: &'static str) -> DefaultUserService {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_id().returning(move |_| {
            Ok(Some(User {
                username: username.to_string(),
                ..user(id)
            }))
        });
        user_repo
            .expect_update_profile()
            .returning(move |_, patch: UpdateProfilePatch| {
                Ok(User {
                    display_name: patch.display_name.flatten(),
                    ..user(id)
                })
            });
        DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        })
        .with_reserved_names(crate::reserved_names::ReservedNames::parse("rim").unwrap())
    }

    fn rename(id: UserId, display_name: &str) -> UpdateMyProfileCmd {
        UpdateMyProfileCmd {
            actor: Actor::new(id),
            display_name: Some(display_name.to_string()),
            avatar_url: None,
            status_text: None,
            bio: None,
        }
    }

    #[tokio::test]
    async fn update_profile_refuses_a_reserved_display_name_but_its_holders_keep_theirs() {
        let id = UserId::new();
        let svc = reserving_svc(id, "carol");
        for name in ["Rim", "rim.official", "ｒｉｍ team"] {
            let err = svc.update_my_profile(rename(id, name)).await.unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == crate::reserved_names::RESERVED_NAME_REFUSAL),
                "{name}: {err:?}"
            );
        }
        let dto = svc
            .update_my_profile(rename(id, "Rimmer"))
            .await
            .expect("not reserved");
        assert_eq!(dto.display_name.as_deref(), Some("Rimmer"));

        // An account whose own username is reserved belongs to the maintainers.
        let svc = reserving_svc(id, "rim-ops");
        let dto = svc
            .update_my_profile(rename(id, "RIM Team"))
            .await
            .expect("a maintainer's name");
        assert_eq!(dto.display_name.as_deref(), Some("RIM Team"));
    }
}
