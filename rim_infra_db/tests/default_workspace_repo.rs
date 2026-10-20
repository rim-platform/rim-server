#![cfg(feature = "db-tests")]

//! The default workspace record against a real database: the claim is taken once and only for
//! the recorded invite, a claimed workspace is recorded with its owner, deleting it clears the
//! record, the other local accounts leave out remote and purged ones, and a workspace is
//! renamed.

mod support;

use chrono::{Duration, Utc};
use rim_core::auth::model::NewInstanceInvite;
use rim_core::auth::repository::InstanceInviteRepository;
use rim_core::default_workspace::model::DefaultWorkspaceRecord;
use rim_core::default_workspace::repository::DefaultWorkspaceRepository;
use rim_core::server::repository::ServerRepository;
use rim_infra_db::sea_orm_impl::default_workspace::SeaOrmDefaultWorkspaceRepository;
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use sea_orm::prelude::Uuid;
use sea_orm::{ConnectionTrait, DbBackend, Statement};

async fn invite(ctx: &support::TestCtx, token: &str) -> rim_core::auth::model::InstanceInviteId {
    SeaOrmInstanceInviteRepository::new(ctx.db.clone())
        .create(NewInstanceInvite {
            created_by_user_id: None,
            token_hash: format!("hash-of-{token}"),
            expires_at: Utc::now() + Duration::days(30),
            max_uses: 1,
            email: None,
        })
        .await
        .expect("invite")
        .id
}

async fn execute(ctx: &support::TestCtx, sql: &str, id: Uuid) {
    ctx.db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            sql,
            vec![id.into()],
        ))
        .await
        .expect(sql);
}

#[tokio::test]
async fn the_claim_is_taken_once_for_the_recorded_invite() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmDefaultWorkspaceRepository::new(ctx.db.clone());
    assert_eq!(
        repo.record().await.unwrap(),
        DefaultWorkspaceRecord::default()
    );

    let first = invite(&ctx, "rim_iinv_first").await;
    let second = invite(&ctx, "rim_iinv_second").await;
    repo.set_claim_invite(first).await.unwrap();
    repo.set_claim_invite(second).await.unwrap();
    assert_eq!(repo.record().await.unwrap().claim_invite_id, Some(second));

    assert!(!repo.take_claim(first).await.unwrap(), "a replaced invite");
    assert!(repo.take_claim(second).await.unwrap());
    assert!(!repo.take_claim(second).await.unwrap(), "taken once");
    assert_eq!(repo.record().await.unwrap().claim_invite_id, None);

    let owner = ctx.create_user("claimer").await;
    let workspace = ctx.create_server("RIM").await;
    repo.record_claim(workspace.id, owner.id).await.unwrap();
    let record = repo.record().await.unwrap();
    assert_eq!(record.workspace_id, Some(workspace.id));
    assert_eq!(record.claimed_by, Some(owner.id));
    assert!(record.claimed_at.is_some());

    // A claimed workspace takes no claim, even through a newly recorded invite.
    let third = invite(&ctx, "rim_iinv_third").await;
    repo.set_claim_invite(third).await.unwrap();
    assert!(!repo.take_claim(third).await.unwrap());
}

#[tokio::test]
async fn deleting_the_claimed_workspace_clears_it_from_the_record() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmDefaultWorkspaceRepository::new(ctx.db.clone());
    let owner = ctx.create_user("claimer").await;
    let workspace = ctx.create_server("RIM").await;
    repo.record_claim(workspace.id, owner.id).await.unwrap();

    SeaOrmServerRepository::new(ctx.db.clone())
        .delete_workspaces(&[workspace.id])
        .await
        .expect("delete");
    let record = repo.record().await.unwrap();
    assert_eq!(record.workspace_id, None);
    assert_eq!(record.claimed_by, Some(owner.id));
    assert!(record.claimed_at.is_some());
}

#[tokio::test]
async fn other_local_accounts_leave_out_the_owner_remote_and_purged_accounts() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmDefaultWorkspaceRepository::new(ctx.db.clone());
    let owner = ctx.create_user("owner").await;
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let remote = ctx.create_key_only_user("r_remote", "rimp1_remote").await;
    execute(
        &ctx,
        r#"UPDATE "user" SET kind = 'remote' WHERE id = $1"#,
        remote.id.into(),
    )
    .await;
    let purged = ctx.create_user("purged").await;
    execute(
        &ctx,
        r#"UPDATE "user" SET deleted_at = now() WHERE id = $1"#,
        purged.id.into(),
    )
    .await;

    assert_eq!(
        repo.other_local_accounts(owner.id).await.unwrap(),
        vec![alice.id, bob.id]
    );
}

#[tokio::test]
async fn a_workspace_is_renamed() {
    let ctx = support::TestCtx::new().await;
    let servers = SeaOrmServerRepository::new(ctx.db.clone());
    let workspace = ctx.create_server("RIM").await;
    assert!(servers.rename(workspace.id, "RIM community").await.unwrap());
    let renamed = servers
        .get_by_id(workspace.id)
        .await
        .unwrap()
        .expect("workspace");
    assert_eq!(renamed.name, "RIM community");
    assert!(renamed.updated_at >= workspace.updated_at);
    assert!(
        !servers
            .rename(rim_core::server::model::ServerId::new(), "x")
            .await
            .unwrap()
    );
}
