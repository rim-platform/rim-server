use sea_orm_migration::prelude::*;

/// Adds `command_audit_log.parent_audit_id` (plain nullable `uuid`, no FK) plus
/// a partial index for the child→parent join. No FK: macro step (child) rows
/// are inserted before the parent macro row is written (separate inserts, not
/// one transaction), so a self-referencing FK would reject the child insert.
/// One consequence worth flagging for anyone querying this column: if a macro
/// crashes mid-execution, child rows can be left permanently referencing a
/// `parent_audit_id` whose parent row was never written — see the doc comment
/// on `AuditLogEntryDto::parent_audit_id` in `rim_services` for the full
/// rationale and how forensic queries should handle it.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .add_column(ColumnDef::new(CommandAuditLog::ParentAuditId).uuid().null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_command_audit_log_parent_audit_id \
                 ON command_audit_log (parent_audit_id) \
                 WHERE parent_audit_id IS NOT NULL",
            )
            .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_command_audit_log_parent_audit_id")
            .await?;
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .drop_column(CommandAuditLog::ParentAuditId)
                    .to_owned(),
            )
            .await?;
        Ok(())
    }
}

#[derive(DeriveIden)]
enum CommandAuditLog {
    Table,
    ParentAuditId,
}
