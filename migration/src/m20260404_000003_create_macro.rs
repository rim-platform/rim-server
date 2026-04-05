use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(Macro::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(Macro::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(Macro::ServerId).uuid().not_null())
                    .col(ColumnDef::new(Macro::UserId).uuid().not_null())
                    .col(
                        ColumnDef::new(Macro::Name)
                            .string_len(128)
                            .not_null(),
                    )
                    .col(ColumnDef::new(Macro::Description).text().null())
                    .col(
                        ColumnDef::new(Macro::Steps)
                            .json_binary()
                            .not_null()
                            .default("[]"),
                    )
                    .col(
                        ColumnDef::new(Macro::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(Macro::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_macro_server_id")
                            .from(Macro::Table, Macro::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_macro_user_id")
                            .from(Macro::Table, Macro::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_macro_server_user_name")
                    .table(Macro::Table)
                    .col(Macro::ServerId)
                    .col(Macro::UserId)
                    .col(Macro::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_macro_server_id")
                    .table(Macro::Table)
                    .col(Macro::ServerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_macro_user_id")
                    .table(Macro::Table)
                    .col(Macro::UserId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_macro_user_id")
                    .table(Macro::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_macro_server_id")
                    .table(Macro::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("uq_macro_server_user_name")
                    .table(Macro::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(Macro::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum Macro {
    Table,
    Id,
    ServerId,
    UserId,
    Name,
    Description,
    Steps,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum Server {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
