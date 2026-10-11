use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EscalationRun::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(EscalationRun::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(EscalationRun::IncidentId).uuid().not_null())
                    .col(ColumnDef::new(EscalationRun::PolicyId).uuid().not_null())
                    .col(
                        ColumnDef::new(EscalationRun::CurrentStepIndex)
                            .integer()
                            .not_null()
                            .default(0),
                    )
                    .col(ColumnDef::new(EscalationRun::NextFireAt).timestamp_with_time_zone())
                    .col(
                        ColumnDef::new(EscalationRun::Status)
                            .string_len(20)
                            .not_null()
                            .default("active"),
                    )
                    .col(
                        ColumnDef::new(EscalationRun::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(EscalationRun::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_run_incident")
                            .from(EscalationRun::Table, EscalationRun::IncidentId)
                            .to(Incident::Table, Incident::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_escalation_run_policy")
                            .from(EscalationRun::Table, EscalationRun::PolicyId)
                            .to(EscalationPolicy::Table, EscalationPolicy::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // Enforces "one active run per incident" — the service creates a fresh run only
        // after the previous one transitioned out of `active`.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_escalation_run_active_incident \
                 ON escalation_run (incident_id) \
                 WHERE status = 'active'",
            )
            .await?;

        // Backs the boot-recovery scan (`EscalationRunRepository::list_active`) and any future
        // "next due" poller.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_escalation_run_active_next_fire \
                 ON escalation_run (status, next_fire_at) \
                 WHERE status = 'active'",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EscalationRun::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum EscalationRun {
    Table,
    Id,
    IncidentId,
    PolicyId,
    CurrentStepIndex,
    NextFireAt,
    Status,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum Incident {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum EscalationPolicy {
    Table,
    Id,
}
