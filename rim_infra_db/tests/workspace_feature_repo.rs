#![cfg(feature = "db-tests")]

mod support;

use rim_core::workspace_feature::model::WorkspaceFeature;
use rim_core::workspace_feature::repository::WorkspaceFeatureRepository;
use rim_infra_db::sea_orm_impl::workspace_feature::SeaOrmWorkspaceFeatureRepository;
use sea_orm::ConnectionTrait;

#[tokio::test]
async fn is_enabled_falls_back_to_default_without_a_row() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceFeatureRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;

    assert!(repo.list_for_server(server.id).await.unwrap().is_empty());
    assert_eq!(
        repo.is_enabled(server.id, WorkspaceFeature::Incidents)
            .await
            .unwrap(),
        WorkspaceFeature::Incidents.default_enabled()
    );
}

#[tokio::test]
async fn set_upserts_and_list_returns_the_latest_value() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceFeatureRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let other_server = ctx.create_server("other").await;
    let owner = ctx.create_user("owner").await;

    let on = repo
        .set(server.id, WorkspaceFeature::Incidents, true, Some(owner.id))
        .await
        .unwrap();
    assert!(on.enabled);
    assert_eq!(on.updated_by, Some(owner.id));
    assert!(
        repo.is_enabled(server.id, WorkspaceFeature::Incidents)
            .await
            .unwrap()
    );

    // Second write for the same (server, feature) updates the row in place.
    let off = repo
        .set(server.id, WorkspaceFeature::Incidents, false, None)
        .await
        .unwrap();
    assert!(!off.enabled);
    assert_eq!(off.updated_by, None);
    assert!(off.updated_at >= on.updated_at);

    let listed = repo.list_for_server(server.id).await.unwrap();
    assert_eq!(listed.len(), 1, "upsert must not duplicate the row");
    assert_eq!(listed[0].feature, WorkspaceFeature::Incidents);
    assert!(!listed[0].enabled);

    // Scoped per server.
    assert!(
        repo.list_for_server(other_server.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn list_skips_rows_with_unknown_feature_names() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceFeatureRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;

    repo.set(server.id, WorkspaceFeature::Incidents, true, None)
        .await
        .unwrap();
    let server_uuid: uuid::Uuid = server.id.into();
    ctx.db
        .execute_unprepared(&format!(
            "INSERT INTO workspace_feature (server_id, feature, enabled, updated_at) \
             VALUES ('{server_uuid}', 'from_the_future', TRUE, now())"
        ))
        .await
        .unwrap();

    let listed = repo.list_for_server(server.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].feature, WorkspaceFeature::Incidents);
}

#[tokio::test]
async fn deleting_the_updating_user_nulls_updated_by() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmWorkspaceFeatureRepository::new(ctx.db.clone());
    let server = ctx.create_server("s").await;
    let owner = ctx.create_user("owner").await;

    repo.set(server.id, WorkspaceFeature::Incidents, true, Some(owner.id))
        .await
        .unwrap();
    let owner_uuid: uuid::Uuid = owner.id.into();
    ctx.db
        .execute_unprepared(&format!("DELETE FROM \"user\" WHERE id = '{owner_uuid}'"))
        .await
        .unwrap();

    let listed = repo.list_for_server(server.id).await.unwrap();
    assert_eq!(listed[0].updated_by, None);
    assert!(listed[0].enabled, "the flag survives its author");
}
