#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use rim_core::server::model::{Server, ServerId};
    use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
    use rim_core::user::model::UserId;
    use rim_services::server::command::{CreateServerCmd, GetServerQuery, ListMyServersQuery};
    use rim_services::server::service::ServerService;
    use rim_services::{Actor, ServiceError};

    use crate::server::DefaultServerService;
    use crate::test_support::server::{MockServerMemberRepo, MockServerRepo, ServerReposPorts};

    #[tokio::test]
    async fn create_server_validates_name() {
        let actor_id = UserId::new();
        let (server_repo, server_member_repo) = (MockServerRepo::new(), MockServerMemberRepo::new());
        let svc = DefaultServerService::new_for_tests(ServerReposPorts {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor_id),
                name: "   ".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn create_server_creates_owner_membership() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();
        let now = chrono::Utc::now();

        let mut server_repo = MockServerRepo::new();
        server_repo.expect_create_server().returning(move |name| {
            Ok(Server {
                id: server_id,
                name: name.to_string(),
                created_at: now,
                updated_at: now,
            })
        });

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_add_member()
            .returning(move |sid, uid, role| {
                assert_eq!(sid, server_id);
                assert_eq!(uid, actor_id);
                assert_eq!(role, ServerMemberRole::Owner);
                Ok(ServerMember {
                    id: ServerMemberId::new(),
                    server_id: sid,
                    user_id: uid,
                    member_role: role,
                    joined_at: now,
                })
            });

        let svc = DefaultServerService::new_for_tests(ServerReposPorts {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let dto = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor_id),
                name: "My server".to_string(),
            })
            .await
            .unwrap();
        assert_eq!(dto.id, server_id);
    }

    #[tokio::test]
    async fn get_server_denies_non_member() {
        let actor_id = UserId::new();
        let server_id = ServerId::new();

        let mut server_member_repo = MockServerMemberRepo::new();
        server_member_repo
            .expect_is_member()
            .returning(|_sid, _uid| Ok(false));

        let mut server_repo = MockServerRepo::new();
        server_repo.expect_get_by_id().never();

        let svc = DefaultServerService::new_for_tests(ServerReposPorts {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let err = svc
            .get_server(GetServerQuery {
                actor: Actor::new(actor_id),
                server_id,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn list_my_servers_returns_dtos() {
        let actor_id = UserId::new();
        let now = chrono::Utc::now();
        let s1 = Server {
            id: ServerId::new(),
            name: "a".to_string(),
            created_at: now,
            updated_at: now,
        };
        let s2 = Server {
            id: ServerId::new(),
            name: "b".to_string(),
            created_at: now,
            updated_at: now,
        };

        let mut server_repo = MockServerRepo::new();
        server_repo
            .expect_get_servers_for_user()
            .returning(move |_uid| Ok(vec![s1.clone(), s2.clone()]));

        let server_member_repo = MockServerMemberRepo::new();

        let svc = DefaultServerService::new_for_tests(ServerReposPorts {
            server_repo: Arc::new(server_repo),
            server_member_repo: Arc::new(server_member_repo),
        });

        let list = svc
            .list_my_servers(ListMyServersQuery {
                actor: Actor::new(actor_id),
            })
            .await
            .unwrap();
        assert_eq!(list.len(), 2);
    }
}
