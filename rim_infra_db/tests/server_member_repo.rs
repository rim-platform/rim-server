#![cfg(feature = "db-tests")]

mod support;

use rim_core::host::model::NewRemoteUser;
use rim_core::host::repository::HostRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

/// Members are added, looked up and listed; the owner is the member holding the `Owner` role,
/// none before one joins.
#[tokio::test]
async fn add_get_is_member_list_members_and_owner() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let u1 = ctx.create_user("u1").await.id;
    let u2 = ctx.create_user("u2").await.id;

    assert!(!repo.is_member(server.id, u1).await.unwrap());

    let _m2 = repo
        .add_member(server.id, u2, ServerMemberRole::Member)
        .await
        .unwrap();
    assert_eq!(repo.get_owner_id(server.id).await.unwrap(), None);
    let m1 = repo
        .add_member(server.id, u1, ServerMemberRole::Owner)
        .await
        .unwrap();

    assert!(repo.is_member(server.id, u1).await.unwrap());
    assert_eq!(repo.get_owner_id(server.id).await.unwrap(), Some(u1));

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

/// A local user joins the direct space as a guest, once; a remote user, an ordinary workspace
/// and an unknown space admit nobody.
#[tokio::test]
async fn join_direct_space_admits_local_users_of_the_direct_space_only() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let host = SeaOrmHostRepository::new(ctx.db.clone());
    let direct = host.ensure_direct_space("Direct messages").await.unwrap();
    let workspace = ctx.create_server("s").await;
    let local = ctx.create_user("alice").await.id;
    let remote = host
        .create_remote_user(NewRemoteUser {
            principal_id: "rimp1_bob".to_string(),
            identity_key: "ed25519:bob".to_string(),
            username: "r_bob".to_string(),
            display_name: "Bob".to_string(),
            home_relay: Some("rims1_relay".to_string()),
        })
        .await
        .unwrap();

    assert!(repo.join_direct_space(direct, local).await.unwrap());
    assert!(repo.join_direct_space(direct, local).await.unwrap());
    let standing = repo.get_member_standing(direct, local).await.unwrap();
    assert!(standing.expect("member").role.is_guest());
    assert_eq!(repo.get_members_for_server(direct).await.unwrap().len(), 1);

    assert!(!repo.join_direct_space(direct, remote).await.unwrap());
    assert!(!repo.join_direct_space(workspace.id, local).await.unwrap());
    assert!(!repo.is_member(workspace.id, local).await.unwrap());
    assert!(
        !repo
            .join_direct_space(rim_core::server::model::ServerId::new(), local)
            .await
            .unwrap()
    );
}

/// A user shares a workspace with the holder of a principal, or with an account, when both are
/// members of one, unless that one is the workspace left out; the direct space never counts.
#[tokio::test]
async fn shares_workspace_counts_workspaces_but_the_one_left_out() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let host = SeaOrmHostRepository::new(ctx.db.clone());
    let direct = host.ensure_direct_space("Direct messages").await.unwrap();
    let everyone = ctx.create_server("everyone").await.id;
    let team = ctx.create_server("team").await.id;
    let ada = ctx.create_user("ada").await.id;
    let bob = host
        .create_remote_user(NewRemoteUser {
            principal_id: "rimp1_bob".to_string(),
            identity_key: "ed25519:bob".to_string(),
            username: "r_bob".to_string(),
            display_name: "Bob".to_string(),
            home_relay: Some("rims1_relay".to_string()),
        })
        .await
        .unwrap();
    let cal = ctx.create_user("cal").await.id;
    for user in [ada, bob, cal] {
        repo.add_member(direct, user, ServerMemberRole::Member)
            .await
            .unwrap();
        repo.add_member(everyone, user, ServerMemberRole::Member)
            .await
            .unwrap();
    }
    let shares = |except| host.shares_workspace(ada, "rimp1_bob", except);
    let shares_with_cal = |except| host.shares_workspace_with_account(ada, cal, except);

    assert!(shares(None).await.unwrap());
    assert!(!shares(Some(everyone)).await.unwrap());
    assert!(shares_with_cal(None).await.unwrap());
    assert!(!shares_with_cal(Some(everyone)).await.unwrap());
    assert!(
        !host
            .shares_workspace_with_account(ada, ctx.create_user("dan").await.id, None)
            .await
            .unwrap()
    );
    assert!(
        !host
            .shares_workspace(ada, "rimp1_carol", None)
            .await
            .unwrap()
    );

    repo.add_member(team, ada, ServerMemberRole::Member)
        .await
        .unwrap();
    repo.add_member(team, bob, ServerMemberRole::Member)
        .await
        .unwrap();
    assert!(shares(Some(everyone)).await.unwrap());
    assert!(shares(Some(team)).await.unwrap());
    assert!(!shares_with_cal(Some(everyone)).await.unwrap());
    repo.add_member(team, cal, ServerMemberRole::Member)
        .await
        .unwrap();
    assert!(shares_with_cal(Some(everyone)).await.unwrap());
}
