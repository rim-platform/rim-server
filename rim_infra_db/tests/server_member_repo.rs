#![cfg(feature = "db-tests")]

mod support;

use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_core::server::repository::ServerRepository;

#[tokio::test]
async fn add_get_is_member_get_by_id_and_list_members() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let repo = SeaOrmServerMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u1 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u1".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;
    let u2 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u2".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    assert_eq!(repo.is_member(server.id, u1).await.unwrap(), false);

    let m1 = repo
        .add_member(server.id, u1, ServerMemberRole::Owner)
        .await
        .unwrap();
    let _m2 = repo
        .add_member(server.id, u2, ServerMemberRole::Guest)
        .await
        .unwrap();

    assert_eq!(repo.is_member(server.id, u1).await.unwrap(), true);

    let by_pair = repo.get_member(server.id, u1).await.unwrap().unwrap();
    assert_eq!(by_pair.id, m1.id);

    let by_id = repo.get_by_id(m1.id).await.unwrap().unwrap();
    assert_eq!(by_id.user_id, u1);

    let members = repo.get_members_for_server(server.id).await.unwrap();
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn change_member_role_and_remove_member_errors() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;

    let server_repo = SeaOrmServerRepository::new(conn.clone());
    let repo = SeaOrmServerMemberRepository::new(conn.clone());
    let user_repo = SeaOrmUserRepository::new(conn.clone());

    let server = server_repo.create_server("s").await.unwrap();
    let u = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;

    let err = repo
        .change_member_role(server.id, u, ServerMemberRole::Owner)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let err = repo.remove_member(server.id, u).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let _ = repo
        .add_member(server.id, u, ServerMemberRole::Guest)
        .await
        .unwrap();

    repo.change_member_role(server.id, u, ServerMemberRole::Owner)
        .await
        .unwrap();

    let updated = repo.get_member(server.id, u).await.unwrap().unwrap();
    assert_eq!(updated.member_role, ServerMemberRole::Owner);

    repo.remove_member(server.id, u).await.unwrap();
    assert!(repo.get_member(server.id, u).await.unwrap().is_none());
}
