#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::conversation::repository::MockConversationRepository;
    use rim_core::conversation_member::repository::MockConversationMemberRepository;
    use rim_core::server::model::{Server, ServerId};
    use rim_core::server::repository::MockServerRepository;
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::server_member::repository::MockServerMemberRepository;
    use rim_core::user::model::UserId;
    use rim_core::user::repository::MockUserRepository;
    use rim_services::server_member::command::{
        AddServerMemberCmd, GetServerMemberQuery, LeaveServerCmd, RemoveServerMemberCmd,
    };
    use rim_services::server_member::service::ServerMemberService;
    use rim_services::{Actor, ServiceError};

    use crate::ServerMemberRepos;
    use crate::server_member::DefaultServerMemberService;

    fn server(server_id: ServerId) -> Server {
        let now = chrono::Utc::now();
        Server {
            id: server_id,
            name: "s".to_string(),
            created_at: now,
            updated_at: now,
        }
    }

    #[tokio::test]
    async fn add_member_denies_adding_self() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(server(server_id))));

        let mut server_member_repo = MockServerMemberRepository::new();
        // owner check path needs get_member
        server_member_repo
            .expect_get_member()
            .returning(move |_sid, uid| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id: uid,
                    member_role: ServerMemberRole::Owner,
                    joined_at: chrono::Utc::now(),
                }))
            });

        let user_repo = MockUserRepository::new();

        let svc = DefaultServerMemberService::new(ServerMemberRepos {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            user_repo: Arc::new(user_repo),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        });

        let err = svc
            .add_member(AddServerMemberCmd {
                actor: Actor::new(actor_id),
                server_id,
                user_id: actor_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn remove_member_denies_removing_owner() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();
        let target_id = UserId::new();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(server(server_id))));

        let mut server_member_repo = MockServerMemberRepository::new();
        // owner check
        server_member_repo
            .expect_get_member()
            .returning(move |_sid, uid| {
                let role = if uid == actor_id || uid == target_id {
                    ServerMemberRole::Owner
                } else {
                    ServerMemberRole::Member
                };
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id: uid,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                }))
            });
        server_member_repo.expect_remove_member().never();

        let user_repo = MockUserRepository::new();

        let svc = DefaultServerMemberService::new(ServerMemberRepos {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            user_repo: Arc::new(user_repo),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        });

        let err = svc
            .remove_member(RemoveServerMemberCmd {
                actor: Actor::new(actor_id),
                server_id,
                user_id: target_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn leave_denies_owner_leaving() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(server(server_id))));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member()
            .returning(move |_sid, _uid| {
                Ok(Some(ServerMember {
                    id: ServerMemberId::new(),
                    server_id,
                    user_id: actor_id,
                    member_role: ServerMemberRole::Owner,
                    joined_at: chrono::Utc::now(),
                }))
            });
        server_member_repo.expect_remove_member().never();

        let user_repo = MockUserRepository::new();

        let svc = DefaultServerMemberService::new(ServerMemberRepos {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            user_repo: Arc::new(user_repo),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        });

        let err = svc
            .leave(LeaveServerCmd {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn get_member_requires_server_membership() {
        let server_id = ServerId::new();
        let actor_id = UserId::new();

        let mut server_repo = MockServerRepository::new();
        server_repo
            .expect_get_by_id()
            .returning(move |_id| Ok(Some(server(server_id))));

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));
        server_member_repo
            .expect_get_member_standing()
            .returning(|_, _| Ok(None));

        let user_repo = MockUserRepository::new();

        let svc = DefaultServerMemberService::new(ServerMemberRepos {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
            user_repo: Arc::new(user_repo),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            moderation_repo: std::sync::Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        });

        let err = svc
            .get_member(GetServerMemberQuery {
                actor: Actor::new(actor_id),
                server_id,
                user_id: UserId::new(),
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::PermissionDenied | ServiceError::NotFound),
            "got: {err:?}"
        );
    }

    /// Track 1.2: a guest's member directory holds only the people who share a conversation
    /// with them (and themselves); everyone else is hidden, including from `get_member`.
    #[tokio::test]
    async fn guest_directory_lists_only_shared_conversation_members() {
        use rim_core::conversation::model::ConversationId;
        use rim_core::server_member::model::MemberStanding;
        use rim_services::server_member::command::ListServerMembersQuery;

        let server_id = ServerId::new();
        let [guest, buddy, stranger] = [UserId::new(), UserId::new(), UserId::new()];
        let shared = ConversationId::new();
        let member = |user_id, role| ServerMember {
            id: ServerMemberId::new(),
            server_id,
            user_id,
            member_role: role,
            joined_at: chrono::Utc::now(),
        };
        let all = vec![
            member(guest, ServerMemberRole::Guest),
            member(buddy, ServerMemberRole::Member),
            member(stranger, ServerMemberRole::Member),
        ];

        let mut server_member_repo = MockServerMemberRepository::new();
        server_member_repo
            .expect_get_member_standing()
            .returning(move |_, uid| {
                Ok(Some(MemberStanding::new(if uid == guest {
                    ServerMemberRole::Guest
                } else {
                    ServerMemberRole::Member
                })))
            });
        let listed = all.clone();
        server_member_repo
            .expect_get_members_for_server()
            .returning(move |_| Ok(listed.clone()));
        let mut conversation_repo = MockConversationRepository::new();
        conversation_repo
            .expect_list_joined_conversation_ids()
            .returning(move |_, _| Box::pin(async move { Ok(vec![shared]) }));
        let mut conversation_member_repo = MockConversationMemberRepository::new();
        conversation_member_repo
            .expect_list_member_ids_for_conversations()
            .returning(move |_| {
                Box::pin(async move {
                    Ok(std::collections::HashMap::from([(
                        shared,
                        vec![guest, buddy],
                    )]))
                })
            });
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_ids().returning(|ids| {
            Ok(ids
                .iter()
                .map(|id| rim_core::user::model::User {
                    id: *id,
                    username: id.to_string(),
                    password_hash: String::new(),
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    last_login_at: None,
                    display_name: None,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    email: "x@example.com".to_string(),
                    timezone: None,
                })
                .collect())
        });

        let svc = DefaultServerMemberService::new(ServerMemberRepos {
            server_repo: Arc::new(MockServerRepository::new()),
            server_member_repo: Arc::new(server_member_repo),
            user_repo: Arc::new(user_repo),
            conversation_repo: Arc::new(conversation_repo),
            conversation_member_repo: Arc::new(conversation_member_repo),
            moderation_repo: Arc::new(
                rim_core::moderation::repository::MockModerationRepository::permissive(),
            ),
        });

        let mut ids: Vec<UserId> = svc
            .list_members(ListServerMembersQuery {
                actor: Actor::new(guest),
                server_id,
            })
            .await
            .expect("list")
            .into_iter()
            .map(|m| m.user_id)
            .collect();
        ids.sort_by_key(|id| id.to_string());
        let mut expected = vec![guest, buddy];
        expected.sort_by_key(|id| id.to_string());
        assert_eq!(ids, expected);

        let err = svc
            .get_member(GetServerMemberQuery {
                actor: Actor::new(guest),
                server_id,
                user_id: stranger,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::NotFound));
    }
}
