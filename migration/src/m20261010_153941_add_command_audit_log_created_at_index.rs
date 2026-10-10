use sea_orm_migration::prelude::*;

/// Indexes the command audit log by `(created_at, id)`, the order the retention purge reads
/// and deletes old rows in, so each batch is an index range instead of a table scan.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "CREATE INDEX IF NOT EXISTS idx_command_audit_log_created_at_id \
     ON command_audit_log (created_at, id)";

const DOWN_SQL: &str = "DROP INDEX IF EXISTS idx_command_audit_log_created_at_id";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
