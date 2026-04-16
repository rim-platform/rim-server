use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(WorkspaceState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(WorkspaceState::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(WorkspaceState::ServerId).uuid().not_null())
                    .col(
                        ColumnDef::new(WorkspaceState::Key)
                            .string_len(512)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceState::Value)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceState::Scope)
                            .string_len(16)
                            .not_null()
                            .default("server"),
                    )
                    .col(ColumnDef::new(WorkspaceState::UserId).uuid().null())
                    .col(ColumnDef::new(WorkspaceState::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(WorkspaceState::UpdatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(WorkspaceState::Version)
                            .big_integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(WorkspaceState::ExpiresAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(WorkspaceState::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(WorkspaceState::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_state_server_id")
                            .from(WorkspaceState::Table, WorkspaceState::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_state_user_id")
                            .from(WorkspaceState::Table, WorkspaceState::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_state_created_by")
                            .from(WorkspaceState::Table, WorkspaceState::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_workspace_state_updated_by")
                            .from(WorkspaceState::Table, WorkspaceState::UpdatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // CHECK: scope must be 'server' or 'user'
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE workspace_state ADD CONSTRAINT chk_workspace_state_scope CHECK (scope IN ('server', 'user'))",
            )
            .await?;

        // CHECK: user_id consistency with scope
        manager
            .get_connection()
            .execute_unprepared(
                "ALTER TABLE workspace_state ADD CONSTRAINT chk_workspace_state_scope_user CHECK ((scope = 'user' AND user_id IS NOT NULL) OR (scope = 'server' AND user_id IS NULL))",
            )
            .await?;

        // UNIQUE: (server_id, scope, user_id, key)
        manager
            .create_index(
                Index::create()
                    .name("uq_workspace_state_server_scope_user_key")
                    .table(WorkspaceState::Table)
                    .col(WorkspaceState::ServerId)
                    .col(WorkspaceState::Scope)
                    .col(WorkspaceState::UserId)
                    .col(WorkspaceState::Key)
                    .unique()
                    .nulls_not_distinct()
                    .to_owned(),
            )
            .await?;

        // INDEX: (server_id, key) for prefix LIKE queries
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_workspace_state_server_key ON workspace_state (server_id, key varchar_pattern_ops)",
            )
            .await?;

        // INDEX: (expires_at) partial for cleanup
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX idx_workspace_state_expires_at ON workspace_state (expires_at) WHERE expires_at IS NOT NULL",
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(WorkspaceState::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum WorkspaceState {
    Table,
    Id,
    ServerId,
    Key,
    Value,
    Scope,
    UserId,
    CreatedBy,
    UpdatedBy,
    Version,
    ExpiresAt,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
enum Server {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
