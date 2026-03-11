use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Keybind::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Keybind::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Keybind::Scope).string().not_null())
                    .col(ColumnDef::new(Keybind::SubjectId).uuid().not_null())
                    .col(ColumnDef::new(Keybind::Keystroke).string().not_null())
                    .col(ColumnDef::new(Keybind::CommandId).string().not_null())
                    .col(ColumnDef::new(Keybind::Context).string().not_null())
                    .col(ColumnDef::new(Keybind::PresetArgs).json_binary().null())
                    .col(
                        ColumnDef::new(Keybind::IsEnabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(
                        ColumnDef::new(Keybind::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Keybind::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_keybind_scope_subject")
                    .table(Keybind::Table)
                    .col(Keybind::Scope)
                    .col(Keybind::SubjectId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_keybind_scope_subject_context_keystroke")
                    .table(Keybind::Table)
                    .col(Keybind::Scope)
                    .col(Keybind::SubjectId)
                    .col(Keybind::Context)
                    .col(Keybind::Keystroke)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("uq_keybind_scope_subject_context_keystroke")
                    .table(Keybind::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_keybind_scope_subject")
                    .table(Keybind::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Keybind::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Keybind {
    Table,
    Id,
    Scope,
    SubjectId,
    Keystroke,
    CommandId,
    Context,
    PresetArgs,
    IsEnabled,
    CreatedAt,
    UpdatedAt,
}
