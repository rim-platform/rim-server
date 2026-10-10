#![cfg(feature = "db-tests")]

mod support;

use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

#[tokio::test]
async fn add_get_is_member_get_by_id_and_list_members() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u1 = ctx.create_user("u1").await.id;
    let u2 = ctx.create_user("u2").await.id;

    assert!(!repo.is_member(server.id, u1).await.unwrap());

    let m1 = repo
        .add_member(server.id, u1, ServerMemberRole::Owner)
        .await
        .unwrap();
    let _m2 = repo
        .add_member(server.id, u2, ServerMemberRole::Member)
        .await
        .unwrap();

    assert!(repo.is_member(server.id, u1).await.unwrap());

    let by_pair = repo.get_member(server.id, u1).await.unwrap().unwrap();
    assert_eq!(by_pair.id, m1.id);

    let members = repo.get_members_for_server(server.id).await.unwrap();
    assert_eq!(members.len(), 2);
}

#[tokio::test]
async fn change_member_role_and_remove_member_errors() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u = ctx.create_user("u").await.id;

    let err = repo
        .change_member_role(server.id, u, ServerMemberRole::Owner)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let err = repo.remove_member(server.id, u).await.unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::NotFound));

    let _ = repo
        .add_member(server.id, u, ServerMemberRole::Member)
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

#[tokio::test]
async fn add_member_conflicts_on_duplicate_server_membership() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user_id = ctx.create_user("dup-server-member").await.id;

    repo.add_member(server.id, user_id, ServerMemberRole::Member)
        .await
        .unwrap();

    let err = repo
        .add_member(server.id, user_id, ServerMemberRole::Member)
        .await
        .unwrap_err();
    assert!(matches!(err.kind(), RepoErrorKind::Conflict));
}
