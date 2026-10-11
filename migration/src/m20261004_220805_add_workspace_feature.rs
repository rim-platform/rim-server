use crate::m20251012_135522_create_user::User;
use crate::m20251029_222439_create_server::Server;
use sea_orm_migration::{prelude::*, schema::*};

/// Creates `workspace_feature` — per-workspace on/off flags for optional modules. A missing row
/// means the feature's
/// default (`WorkspaceFeature::default_enabled` in `rim_core`), which for `incidents` is off.
///
/// **Data step:** every server that exists when this migration runs gets an explicit
/// `('incidents', true)` row, so workspaces created before feature flags existed keep their
/// incident/on-call module. Only servers created afterwards fall back to the default.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspaceFeature::Table)
                    .if_not_exists()
                    .col(uuid(WorkspaceFeature::ServerId).not_null())
                    .col(text(WorkspaceFeature::Feature).not_null())
                    .col(boolean(WorkspaceFeature::Enabled).not_null())
                    .col(uuid_null(WorkspaceFeature::UpdatedBy))
                    .col(
                        timestamp_with_time_zone(WorkspaceFeature::UpdatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .name("pk-workspace-feature")
                            .col(WorkspaceFeature::ServerId)
                            .col(WorkspaceFeature::Feature),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-workspace-feature-server")
                            .from(WorkspaceFeature::Table, WorkspaceFeature::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-workspace-feature-updated-by")
                            .from(WorkspaceFeature::Table, WorkspaceFeature::UpdatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::SetNull)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "INSERT INTO workspace_feature (server_id, feature, enabled, updated_by, updated_at) \
                 SELECT id, 'incidents', TRUE, NULL, now() FROM server \
                 ON CONFLICT (server_id, feature) DO NOTHING",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WorkspaceFeature::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum WorkspaceFeature {
    Table,
    ServerId,
    Feature,
    Enabled,
    UpdatedBy,
    UpdatedAt,
}
