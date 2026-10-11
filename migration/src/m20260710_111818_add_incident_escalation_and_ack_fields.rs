use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Nullable `ADD COLUMN`s, non-rewriting on PG11+. No backfill: existing incidents were
        // never escalated or acknowledged, so `NULL` is correct for them.
        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .add_column(ColumnDef::new(Incident::EscalationPolicyId).uuid())
                    .add_column(ColumnDef::new(Incident::AcknowledgedAt).timestamp_with_time_zone())
                    .add_column(ColumnDef::new(Incident::AcknowledgedBy).uuid())
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk_incident_escalation_policy")
                            .from_tbl(Incident::Table)
                            .from_col(Incident::EscalationPolicyId)
                            .to_tbl(EscalationPolicy::Table)
                            .to_col(EscalationPolicy::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .add_foreign_key(
                        TableForeignKey::new()
                            .name("fk_incident_acknowledged_by")
                            .from_tbl(Incident::Table)
                            .from_col(Incident::AcknowledgedBy)
                            .to_tbl(User::Table)
                            .to_col(User::Id),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .drop_column(Incident::EscalationPolicyId)
                    .drop_column(Incident::AcknowledgedAt)
                    .drop_column(Incident::AcknowledgedBy)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum Incident {
    Table,
    EscalationPolicyId,
    AcknowledgedAt,
    AcknowledgedBy,
}

#[derive(DeriveIden)]
pub enum EscalationPolicy {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
