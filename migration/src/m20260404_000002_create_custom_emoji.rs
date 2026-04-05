use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(CustomEmoji::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(CustomEmoji::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(CustomEmoji::ServerId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CustomEmoji::Name)
                            .string_len(64)
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CustomEmoji::ImageUrl)
                            .text()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CustomEmoji::UploadedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(CustomEmoji::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_custom_emoji_server")
                            .from(CustomEmoji::Table, CustomEmoji::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_custom_emoji_user")
                            .from(CustomEmoji::Table, CustomEmoji::UploadedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_custom_emoji_server")
                    .table(CustomEmoji::Table)
                    .col(CustomEmoji::ServerId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_custom_emoji_server_name")
                    .table(CustomEmoji::Table)
                    .col(CustomEmoji::ServerId)
                    .col(CustomEmoji::Name)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_custom_emoji_server_name")
                    .table(CustomEmoji::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_custom_emoji_server")
                    .table(CustomEmoji::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(CustomEmoji::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum CustomEmoji {
    Table,
    Id,
    ServerId,
    Name,
    ImageUrl,
    UploadedBy,
    CreatedAt,
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
