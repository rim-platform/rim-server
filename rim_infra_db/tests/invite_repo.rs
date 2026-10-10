#![cfg(feature = "db-tests")]

mod support;

use rim_core::auth::model::NewInvite;
use rim_core::auth::repository::InviteRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_infra_db::sea_orm_impl::invite::SeaOrmInviteRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;

#[tokio::test]
async fn create_get_by_id_and_token_hash_and_revoke() {
    let ctx = support::TestCtx::new().await;

    let member_repo = SeaOrmServerMemberRepository::new(ctx.db.clone());
    let repo = SeaOrmInviteRepository::new(ctx.db.clone());

    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;
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
            role: ServerMemberRole::Member,
        })
        .await
        .unwrap();

    assert!(repo.get_by_id(inv.id).await.unwrap().is_some());
    let by_hash = repo.get_by_token_hash("ith").await.unwrap().unwrap();
    assert_eq!(by_hash.id, inv.id);
}

#[tokio::test]
async fn try_consume_respects_max_uses() {
    let ctx = support::TestCtx::new().await;

    let repo = SeaOrmInviteRepository::new(ctx.db.clone());

    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;

    let inv = repo
        .create_invite(NewInvite {
            server_id: server.id,
            created_by_user_id: user.id,
            token_hash: "ith".to_string(),
            expires_at: None,
            max_uses: 1,
            role: ServerMemberRole::Member,
        })
        .await
        .unwrap();

    assert!(repo.try_consume(inv.id).await.unwrap());
    assert!(!repo.try_consume(inv.id).await.unwrap());
}

/// Track 1.2: an invite stores the role it grants.
#[tokio::test]
async fn invite_role_round_trips() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmInviteRepository::new(ctx.db.clone());
    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;

    for (hash, role) in [
        ("member-link", ServerMemberRole::Member),
        ("guest-link", ServerMemberRole::Guest),
    ] {
        let inv = repo
            .create_invite(NewInvite {
                server_id: server.id,
                created_by_user_id: user.id,
                token_hash: hash.to_string(),
                expires_at: None,
                max_uses: 1,
                role,
            })
            .await
            .unwrap();
        assert_eq!(inv.role, role);
        assert_eq!(
            repo.get_by_token_hash(hash).await.unwrap().unwrap().role,
            role
        );
    }
}
