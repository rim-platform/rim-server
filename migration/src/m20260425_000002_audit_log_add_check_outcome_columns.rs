use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .add_column(
                        ColumnDef::new(CommandAuditLog::CapabilityCheckOutcome)
                            .string_len(32)
                            .null(),
                    )
                    .add_column(
                        ColumnDef::new(CommandAuditLog::ConfirmationOutcome)
                            .string_len(32)
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(CommandAuditLog::Table)
                    .drop_column(CommandAuditLog::CapabilityCheckOutcome)
                    .drop_column(CommandAuditLog::ConfirmationOutcome)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum CommandAuditLog {
    Table,
    CapabilityCheckOutcome,
    ConfirmationOutcome,
}
