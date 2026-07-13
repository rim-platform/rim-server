use sea_orm_migration::prelude::*;

/// Adds `incident.alert_fingerprint` and `incident.source`, plus a partial unique index
/// enforcing "at most one non-resolved incident per (server, fingerprint)" — the DB-level
/// dedup guarantee for Track B2 alert ingestion (see `docs/alerts.md`).
///
/// `source` defaults to `'manual'` so every existing row (and every human-declared incident
/// going forward) is correct with no backfill. No CHECK constraint is added on `source` here —
/// it mirrors the existing `status`/`severity` columns on this table, which are also plain
/// `varchar` validated only at the application layer.
///
/// **No FK / referential enforcement on `alert_fingerprint`** — it's a caller-supplied or
/// derived dedup key, not a reference to another row.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .add_column(
                        ColumnDef::new(Incident::AlertFingerprint)
                            .string_len(128)
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(Incident::Source)
                            .string_len(32)
                            .not_null()
                            .default("manual"),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "CREATE UNIQUE INDEX idx_incident_active_fingerprint \
                 ON incident (server_id, alert_fingerprint) \
                 WHERE alert_fingerprint IS NOT NULL AND status <> 'resolved'",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_incident_active_fingerprint")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(Incident::Table)
                    .drop_column(Incident::Source)
                    .drop_column(Incident::AlertFingerprint)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum Incident {
    Table,
    AlertFingerprint,
    Source,
}
