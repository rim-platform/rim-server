#![cfg(feature = "db-tests")]

//! Track 0.4: the `add_workspace_feature` migration's data step keeps the incident module on
//! for every workspace that existed before feature flags, while workspaces created afterwards
//! fall back to the default (off).
//!
//! Rolls the schema back to just before the migration, seeds a server, and migrates up again.
//! Destructive for the shared test database, like every db-test: run single-threaded.

mod support;

use migration::{Migrator, MigratorTrait};

use rim_core::workspace_feature::model::WorkspaceFeature;
use rim_core::workspace_feature::repository::WorkspaceFeatureRepository;
use rim_infra_db::sea_orm_impl::workspace_feature::SeaOrmWorkspaceFeatureRepository;

const MIGRATION_NAME: &str = "m20261004_220805_add_workspace_feature";

#[tokio::test]
async fn data_step_enables_incidents_for_pre_existing_servers_only() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;

    // Steps to roll back so that `add_workspace_feature` is un-applied, robust to later
    // migrations being appended after it.
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");

    Migrator::down(&conn, Some(steps))
        .await
        .expect("roll back to before add_workspace_feature");
    let pre_existing = ctx.create_server("pre-existing").await;
    Migrator::up(&conn, None)
        .await
        .expect("re-apply add_workspace_feature");

    let repo = SeaOrmWorkspaceFeatureRepository::new(ctx.db.clone());
    let rows = repo.list_for_server(pre_existing.id).await.unwrap();
    assert_eq!(rows.len(), 1, "one seeded row per pre-existing server");
    assert_eq!(rows[0].feature, WorkspaceFeature::Incidents);
    assert!(rows[0].enabled, "pre-existing workspaces keep incidents on");
    assert_eq!(rows[0].updated_by, None, "seeded rows have no author");
    assert!(
        repo.is_enabled(pre_existing.id, WorkspaceFeature::Incidents)
            .await
            .unwrap()
    );

    let created_after = ctx.create_server("created-after").await;
    assert!(
        repo.list_for_server(created_after.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        !repo
            .is_enabled(created_after.id, WorkspaceFeature::Incidents)
            .await
            .unwrap(),
        "new workspaces get the default (off)"
    );
}
