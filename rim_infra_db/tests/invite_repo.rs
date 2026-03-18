#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::InviteRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

#[tokio::test]
async fn create_get_by_id_and_token_hash_and_revoke() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let server_repo = SeaOrmServerRepository::new(db.clone());
    let member_repo = SeaOrmServerMemberRepository::new(db.clone());
    let repo = SeaOrmInviteRepository::new(db.clone());

    let user = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();
    let server = server_repo.create_server("s").await.unwrap();
    let _ = member_repo
        .add_member(server.id, user.id, ServerMemberRole::Owner)
        .await
        .unwrap();

    let inv = repo
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: user.id,
            token_hash: "ith".to_string(),
            expires_at: None,
            max_uses: 2,
        })
        .await
        .unwrap();

    assert!(repo.get_by_id(inv.id).await.unwrap().is_some());
    let by_hash = repo.get_by_token_hash("ith").await.unwrap().unwrap();
    assert_eq!(by_hash.id, inv.id);

    repo.revoke_invite(inv.id).await.unwrap();
    let after = repo.get_by_id(inv.id).await.unwrap().unwrap();
    assert!(after.revoked_at.is_some());
}

#[tokio::test]
async fn try_consume_respects_max_uses() {
    let Some(conn) = support::db_conn_or_skip().await else {
        return;
    };
    support::reset_db(&conn).await;
    let db: DbHandle = conn.into();

    let user_repo = SeaOrmUserRepository::new(db.clone());
    let server_repo = SeaOrmServerRepository::new(db.clone());
    let repo = SeaOrmInviteRepository::new(db.clone());

    let user = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "pw".to_string(),
        })
        .await
        .unwrap();
    let server = server_repo.create_server("s").await.unwrap();

    let inv = repo
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: user.id,
            token_hash: "ith".to_string(),
            expires_at: None,
            max_uses: 1,
        })
        .await
        .unwrap();

    assert_eq!(repo.try_consume(inv.id).await.unwrap(), true);
    assert_eq!(repo.try_consume(inv.id).await.unwrap(), false);
}
