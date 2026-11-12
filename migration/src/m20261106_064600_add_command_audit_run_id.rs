use sea_orm_migration::prelude::*;

/// Adds `command_audit_log.run_id`, the identifier a client gives one stretch of an agent's
/// calls, and a partial index for reading one actor's run. Building the index blocks writes to
/// `command_audit_log` while it runs.
#[derive(DeriveMigrationName)]
pub struct Migration;

const INDEX_UP_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_command_audit_log_actor_run_id \
     ON command_audit_log (actor_user_id, run_id) \
     WHERE run_id IS NOT NULL";

const INDEX_DOWN_SQL: &str = "DROP INDEX IF EXISTS idx_command_audit_log_actor_run_id";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .add_column(ColumnDef::new(CommandAuditLog::RunId).text().null())
                    .to_owned(),
            )
            .await?;
        manager
            .get_connection()
            .execute_unprepared(INDEX_UP_SQL)
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(INDEX_DOWN_SQL)
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .drop_column(CommandAuditLog::RunId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum CommandAuditLog {
    Table,
    RunId,
}
