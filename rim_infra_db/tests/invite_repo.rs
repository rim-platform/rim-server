#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};

use rim_core::auth::model::{Invite, NewInvite};
use rim_core::auth::repository::{InviteRepository, ListInvitesParams};
use rim_core::repository::pagination::PageDir;
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
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

/// An invite stores the role it grants.
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

async fn invite_expiring(
    repo: &SeaOrmInviteRepository,
    server_id: ServerId,
    creator: UserId,
    hash: &str,
    expires_at: Option<chrono::DateTime<Utc>>,
) -> Invite {
    repo.create_invite(NewInvite {
        server_id,
        created_by_user_id: creator,
        token_hash: hash.to_string(),
        expires_at,
        max_uses: 5,
        role: ServerMemberRole::Member,
    })
    .await
    .unwrap()
}

/// The consume refuses a revoked invite and an expired one; an unexpired invite and one
/// without an expiry are consumed.
#[tokio::test]
async fn try_consume_refuses_revoked_and_expired_invites() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmInviteRepository::new(ctx.db.clone());
    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;

    let revoked = invite_expiring(&repo, server.id, user.id, "revoked", None).await;
    repo.revoke(revoked.id, Utc::now()).await.unwrap();
    assert!(!repo.try_consume(revoked.id).await.unwrap());

    let expired = invite_expiring(
        &repo,
        server.id,
        user.id,
        "expired",
        Some(Utc::now() - Duration::seconds(1)),
    )
    .await;
    assert!(!repo.try_consume(expired.id).await.unwrap());

    for (hash, expires_at) in [
        ("open", None),
        ("dated", Some(Utc::now() + Duration::minutes(5))),
    ] {
        let invite = invite_expiring(&repo, server.id, user.id, hash, expires_at).await;
        assert!(repo.try_consume(invite.id).await.unwrap(), "{hash}");
    }

    for (invite, uses) in [(revoked, 0), (expired, 0)] {
        let stored = repo.get_by_id(invite.id).await.unwrap().unwrap();
        assert_eq!(stored.use_count, uses);
    }
}

/// A revocation keeps the first revocation time, and revoking an unknown invite changes
/// nothing.
#[tokio::test]
async fn revoke_keeps_the_first_revocation() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmInviteRepository::new(ctx.db.clone());
    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;
    let invite = invite_expiring(&repo, server.id, user.id, "h", None).await;

    let first = Utc::now() - Duration::minutes(1);
    repo.revoke(invite.id, first).await.unwrap();
    repo.revoke(invite.id, Utc::now()).await.unwrap();
    let stored = repo.get_by_id(invite.id).await.unwrap().unwrap();
    let revoked_at = stored.revoked_at.expect("revoked");
    assert!(
        (revoked_at - first).num_milliseconds().abs() < 1,
        "{revoked_at}"
    );

    repo.revoke(rim_core::auth::model::InviteId::new(), Utc::now())
        .await
        .unwrap();
}

/// A workspace's invites list newest first, in pages with the standard DESC cursors, and
/// leave out other workspaces' invites.
#[tokio::test]
async fn list_for_server_pages_newest_first() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmInviteRepository::new(ctx.db.clone());
    let user = ctx.create_user("u").await;
    let server = ctx.create_server("s").await;
    let other = ctx.create_server("other").await;

    let mut created = Vec::new();
    for i in 0..3 {
        created.push(invite_expiring(&repo, server.id, user.id, &format!("h{i}"), None).await);
    }
    invite_expiring(&repo, other.id, user.id, "elsewhere", None).await;
    let newest_first: Vec<_> = created.iter().rev().map(|i| i.id).collect();

    let params = |dir, cursor| ListInvitesParams {
        server_id: server.id,
        limit: 2,
        dir,
        cursor,
    };
    let first = repo
        .list_for_server(params(PageDir::After, None))
        .await
        .unwrap();
    let ids: Vec<_> = first.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, newest_first[..2]);
    assert!(first.prev.is_none());
    let next = first.next.expect("an older page");

    let second = repo
        .list_for_server(params(PageDir::After, Some(next)))
        .await
        .unwrap();
    let ids: Vec<_> = second.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, newest_first[2..]);
    assert!(second.next.is_none());
    let back = second.prev.expect("a newer page");

    let again = repo
        .list_for_server(params(PageDir::Before, Some(back)))
        .await
        .unwrap();
    let ids: Vec<_> = again.items.iter().map(|i| i.id).collect();
    assert_eq!(ids, newest_first[..2]);
}
