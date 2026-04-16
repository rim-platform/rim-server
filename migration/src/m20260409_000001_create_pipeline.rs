use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Pipeline::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Pipeline::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Pipeline::ServerId).uuid().not_null())
                    .col(ColumnDef::new(Pipeline::Name).string_len(256).not_null())
                    .col(ColumnDef::new(Pipeline::Description).text().null())
                    .col(
                        ColumnDef::new(Pipeline::TriggerData)
                            .json_binary()
                            .not_null(),
                    )
                    .col(ColumnDef::new(Pipeline::FilterData).json_binary().null())
                    .col(
                        ColumnDef::new(Pipeline::ActionsData)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(Pipeline::Enabled)
                            .boolean()
                            .not_null()
                            .default(true),
                    )
                    .col(ColumnDef::new(Pipeline::CooldownSecs).integer().null())
                    .col(ColumnDef::new(Pipeline::CreatedBy).uuid().not_null())
                    .col(
                        ColumnDef::new(Pipeline::SchemaVersion)
                            .integer()
                            .not_null()
                            .default(1),
                    )
                    .col(
                        ColumnDef::new(Pipeline::LastFiredAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .col(
                        ColumnDef::new(Pipeline::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Pipeline::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_pipeline_server_id")
                            .from(Pipeline::Table, Pipeline::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_pipeline_created_by")
                            .from(Pipeline::Table, Pipeline::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        // INDEX: (server_id, enabled)
        manager
            .create_index(
                Index::create()
                    .name("idx_pipeline_server_id_enabled")
                    .table(Pipeline::Table)
                    .col(Pipeline::ServerId)
                    .col(Pipeline::Enabled)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(Pipeline::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Pipeline {
    Table,
    Id,
    ServerId,
    Name,
    Description,
    TriggerData,
    FilterData,
    ActionsData,
    Enabled,
    CooldownSecs,
    CreatedBy,
    SchemaVersion,
    LastFiredAt,
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
