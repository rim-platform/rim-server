use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CommandAuditLog::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CommandAuditLog::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CommandAuditLog::ActorUserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(CommandAuditLog::ServerId).uuid().not_null())
                    .col(ColumnDef::new(CommandAuditLog::CommandId).text().not_null())
                    .col(
                        ColumnDef::new(CommandAuditLog::Initiator)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommandAuditLog::ArgsRedacted)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommandAuditLog::Sensitivity)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommandAuditLog::Dangerous)
                            .boolean()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CommandAuditLog::ConfirmationKind)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(ColumnDef::new(CommandAuditLog::TypedVerb).text().null())
                    .col(
                        ColumnDef::new(CommandAuditLog::Outcome)
                            .string_len(32)
                            .not_null(),
                    )
                    .col(ColumnDef::new(CommandAuditLog::ErrorKind).text().null())
                    .col(
                        ColumnDef::new(CommandAuditLog::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // INDEX: (server_id, created_at DESC) — primary forensic query: all
        // audit events for a workspace ordered by time.
        manager
            .create_index(
                Index::create()
                    .name("idx_command_audit_log_server_id_created_at")
                    .table(CommandAuditLog::Table)
                    .col(CommandAuditLog::ServerId)
                    .col(CommandAuditLog::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // INDEX: (actor_user_id, created_at DESC) — query by actor.
        manager
            .create_index(
                Index::create()
                    .name("idx_command_audit_log_actor_user_id_created_at")
                    .table(CommandAuditLog::Table)
                    .col(CommandAuditLog::ActorUserId)
                    .col(CommandAuditLog::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // INDEX: (command_id, created_at DESC) — query by command type.
        manager
            .create_index(
                Index::create()
                    .name("idx_command_audit_log_command_id_created_at")
                    .table(CommandAuditLog::Table)
                    .col(CommandAuditLog::CommandId)
                    .col(CommandAuditLog::CreatedAt)
                    .to_owned(),
            )
            .await?;

        // CHECK: outcome must be one of the known values.
        let stmts = [
            "ALTER TABLE command_audit_log ADD CONSTRAINT command_audit_log_outcome_check \
             CHECK (outcome IN ('success', 'error', 'confirmation_required', 'permission_denied'))",
        ];
        for sql in stmts {
            manager.get_connection().execute_unprepared(sql).await?;
        }

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(CommandAuditLog::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum CommandAuditLog {
    Table,
    Id,
    ActorUserId,
    ServerId,
    CommandId,
    Initiator,
    ArgsRedacted,
    Sensitivity,
    Dangerous,
    ConfirmationKind,
    TypedVerb,
    Outcome,
    ErrorKind,
    CreatedAt,
}
