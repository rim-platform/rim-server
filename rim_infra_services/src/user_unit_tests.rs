#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::user::model::{UpdateProfilePatch, User, UserId};
    use rim_core::user::repository::MockUserRepository;
    use rim_services::storage::service::MockStorageService;
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
    async fn successful_profile_changes_notify_workspaces_and_the_users_direct_space() {
        use rim_core::server::model::{Server, ServerId};
        use rim_core::server::repository::MockServerRepository;
        use rim_services::events::{Broadcaster, Observers, WorkspaceEvent};
        #[derive(Default)]
        struct Seen(std::sync::Mutex<Vec<WorkspaceEvent>>);
        impl Broadcaster for Seen {
            fn broadcast(&self, event: WorkspaceEvent) {
                self.0.lock().unwrap().push(event);
            }
        }
        let id = UserId::new();
        let workspace = ServerId::new();
        let direct = ServerId::new();
        let mut users = MockUserRepository::new();
        users
            .expect_update_profile()
            .returning(move |_, _| Ok(user(id)));
        let mut servers = MockServerRepository::new();
        servers.expect_get_servers_for_user().returning(move |_| {
            Ok(vec![Server {
                id: workspace,
                name: "Workspace".to_string(),
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }])
        });
        let seen = Arc::new(Seen::default());
        let service = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(users),
        })
        .with_profile_events(
            Arc::new(servers),
            Observers::new(seen.clone()),
            Some(direct),
        );
        service
            .update_my_profile(UpdateMyProfileCmd {
                actor: Actor::new(id),
                display_name: None,
                avatar_url: None,
                status_text: Some("ready".to_string()),
                bio: None,
            })
            .await
            .unwrap();
        let events = seen.0.lock().unwrap();
        assert_eq!(events.len(), 2);
        for workspace_id in [workspace, direct] {
            assert!(events.iter().any(|event| matches!(event, WorkspaceEvent::MemberProfileUpdated { workspace_id: sid, user_id } if *sid == workspace_id && *user_id == id)));
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

    fn set_avatar(id: UserId, avatar_url: &str) -> UpdateMyProfileCmd {
        UpdateMyProfileCmd {
            actor: Actor::new(id),
            display_name: None,
            status_text: None,
            avatar_url: Some(avatar_url.to_string()),
            bio: None,
        }
    }

    /// A user service over one account `id` that saves the avatar URL it is given, deleting
    /// files through `storage`.
    fn avatar_svc(id: UserId, storage: MockStorageService) -> DefaultUserService {
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_update_profile()
            .returning(move |_, patch: UpdateProfilePatch| {
                Ok(User {
                    avatar_url: patch.avatar_url.flatten(),
                    ..user(id)
                })
            });
        DefaultUserService::new(UserRepos {
            user_repo: Arc::new(user_repo),
        })
        .with_storage(Arc::new(storage))
    }

    /// Only the caller's own avatar URL, or an empty one, is accepted: an external URL, another
    /// user's avatar, any other stored file and an unknown extension are refused before the
    /// profile is saved or a file deleted.
    #[tokio::test]
    async fn update_profile_accepts_only_the_callers_own_avatar_url() {
        let id = UserId::new();
        let other = UserId::new();
        let svc = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(MockUserRepository::new()),
        })
        .with_storage(Arc::new(MockStorageService::new()));

        for url in [
            "https://evil.com/avatar.png".to_string(),
            format!("/api/files/avatars/{other}/avatar.png"),
            format!("/api/files/avatars/{id}/avatar.svg"),
            format!("/api/files/avatars/{id}/../{other}/avatar.png"),
            format!("/api/files/uploads/{id}/a.png"),
            "/api/files/".to_string(),
        ] {
            let err = svc
                .update_my_profile(set_avatar(id, &url))
                .await
                .unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(msg) if msg.contains("avatar_url")),
                "{url}: {err:?}"
            );
        }
    }

    /// Setting the avatar to one of the caller's files keeps that file and deletes the caller's
    /// other avatar files.
    #[tokio::test]
    async fn setting_the_avatar_deletes_the_callers_other_avatar_files() {
        let id = UserId::new();
        let key = format!("avatars/{id}/avatar.webp");
        let kept = key.clone();
        let mut storage = MockStorageService::new();
        storage
            .expect_delete_avatar_files()
            .withf(move |user_id, keep| *user_id == id && keep.as_deref() == Some(kept.as_str()))
            .times(1)
            .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
        let svc = avatar_svc(id, storage);

        let url = format!(" /api/files/{key} ");
        let dto = svc.update_my_profile(set_avatar(id, &url)).await.unwrap();
        assert_eq!(dto.avatar_url, Some(format!("/api/files/{key}")));
    }

    /// Clearing the avatar deletes every avatar file of the caller; a storage failure is logged
    /// and the cleared profile still returned.
    #[tokio::test]
    async fn clearing_the_avatar_deletes_all_the_callers_avatar_files() {
        let id = UserId::new();
        let mut storage = MockStorageService::new();
        storage
            .expect_delete_avatar_files()
            .withf(move |user_id, keep| *user_id == id && keep.is_none())
            .times(2)
            .returning(|_, _| Box::pin(async { Err(ServiceError::Unavailable) }));
        let svc = avatar_svc(id, storage);

        for url in ["", "  "] {
            let dto = svc.update_my_profile(set_avatar(id, url)).await.unwrap();
            assert_eq!(dto.avatar_url, None, "{url:?}");
        }
    }

    /// A profile change without an avatar URL deletes no file.
    #[tokio::test]
    async fn a_profile_change_without_an_avatar_url_deletes_no_file() {
        let id = UserId::new();
        let mut storage = MockStorageService::new();
        storage.expect_delete_avatar_files().never();
        let svc = avatar_svc(id, storage);

        svc.update_my_profile(UpdateMyProfileCmd {
            actor: Actor::new(id),
            display_name: None,
            status_text: Some("away".to_string()),
            avatar_url: None,
            bio: None,
        })
        .await
        .unwrap();
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

    /// An instance operator is never on probation, however new the account.
    #[tokio::test]
    async fn an_instance_operator_has_no_probation_end() {
        let id = UserId::new();
        let created = chrono::Utc::now();
        let service = DefaultUserService::new(UserRepos {
            user_repo: Arc::new(crate::probation_fixtures::users_and_operators(
                vec![crate::probation_fixtures::account(id, created, None)],
                vec![id],
            )),
        })
        .with_limits(rim_services::limits::InstanceLimits {
            probation_hours: Some(24),
            ..Default::default()
        })
        .with_clock(crate::clock::Clock::fixed(created));
        let me = GetMeQuery {
            actor: Actor::new(id),
        };
        assert_eq!(service.probation_ends_at(me).await.unwrap(), None);
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
        for name in [
            "Rim",
            "rim.official",
            "ｒｉｍ team",
            "R\u{200B}im",
            "R\u{200D}im",
            "R\u{200C}im Team",
            "\u{2066}RIM\u{2069} Support",
            "R\u{456}m Support",
        ] {
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

    /// A display name is stored without control and format characters, with its whitespace
    /// collapsed; one with nothing visible left clears it.
    #[tokio::test]
    async fn update_profile_cleans_the_display_name() {
        let id = UserId::new();
        let svc = reserving_svc(id, "carol");
        for (typed, stored) in [
            ("\u{202E}nimda", Some("nimda")),
            (
                "Ada\u{AD} \n  Love\u{2060}lace\u{FEFF}",
                Some("Ada Lovelace"),
            ),
            ("\u{200B} \u{2060}", None),
            (
                "Ada \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}",
                Some("Ada \u{1F468}\u{200D}\u{1F469}\u{200D}\u{1F467}"),
            ),
            (
                "\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}",
                Some("\u{645}\u{6CC}\u{200C}\u{62E}\u{648}\u{627}\u{647}\u{645}"),
            ),
        ] {
            let dto = svc.update_my_profile(rename(id, typed)).await.unwrap();
            assert_eq!(dto.display_name.as_deref(), stored, "{typed:?}");
        }
        let padded = format!("{}\u{200B}", "x".repeat(64));
        assert!(svc.update_my_profile(rename(id, &padded)).await.is_ok());
    }
}
