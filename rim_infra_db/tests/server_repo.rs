#![cfg(feature = "db-tests")]

mod support;

use rim_core::server::repository::ServerRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;

use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_get_and_find_by_name() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let repo = SeaOrmServerRepository::new(db.clone());
    assert!(repo.find_by_name("missing").await.unwrap().is_none());

    let created = repo.create_server("s_test").await.unwrap();
    let fetched = repo.get_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(fetched.id, created.id);
    assert_eq!(fetched.name, "s_test");

    let by_name = repo.find_by_name("s_test").await.unwrap().unwrap();
    assert_eq!(by_name.id, created.id);
}

#[tokio::test]
async fn get_servers_for_user_returns_joined_servers() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let server_repo = SeaOrmServerRepository::new(db.clone());
    let member_repo = SeaOrmServerMemberRepository::new(db.clone());
    let user_repo = SeaOrmUserRepository::new(db.clone());

    let user_id = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap()
        .id;
    let s1 = server_repo.create_server("s1").await.unwrap();
    let s2 = server_repo.create_server("s2").await.unwrap();

    assert_eq!(
        server_repo
            .get_servers_for_user(user_id)
            .await
            .unwrap()
            .len(),
        0
    );

    let _ = member_repo
        .add_member(s1.id, user_id, ServerMemberRole::Guest)
        .await
        .unwrap();
    let _ = member_repo
        .add_member(s2.id, user_id, ServerMemberRole::Owner)
        .await
        .unwrap();

    let mut servers = server_repo.get_servers_for_user(user_id).await.unwrap();
    servers.sort_by_key(|s| s.name.clone());
    assert_eq!(servers.len(), 2);
    assert_eq!(servers[0].id, s1.id);
    assert_eq!(servers[1].id, s2.id);
}
