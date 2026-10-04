use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ChannelLens::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ChannelLens::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(ChannelLens::ServerId).uuid().not_null())
                    .col(
                        ColumnDef::new(ChannelLens::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(ColumnDef::new(ChannelLens::CreatedBy).uuid().not_null())
                    .col(ColumnDef::new(ChannelLens::Name).string_len(100).not_null())
                    .col(
                        ColumnDef::new(ChannelLens::FilterSpec)
                            .json_binary()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ChannelLens::IsShared)
                            .boolean()
                            .not_null()
                            .default(false),
                    )
                    .col(
                        ColumnDef::new(ChannelLens::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(
                        ColumnDef::new(ChannelLens::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_channel_lens_server_id")
                            .from(ChannelLens::Table, ChannelLens::ServerId)
                            .to(Server::Table, Server::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_channel_lens_conversation_id")
                            .from(ChannelLens::Table, ChannelLens::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_channel_lens_created_by")
                            .from(ChannelLens::Table, ChannelLens::CreatedBy)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_channel_lens_conv_user")
                    .table(ChannelLens::Table)
                    .col(ChannelLens::ConversationId)
                    .col(ChannelLens::CreatedBy)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_channel_lens_conv_shared")
                    .table(ChannelLens::Table)
                    .col(ChannelLens::ConversationId)
                    .col(ChannelLens::IsShared)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_channel_lens_conv_shared")
                    .table(ChannelLens::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_channel_lens_conv_user")
                    .table(ChannelLens::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(ChannelLens::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum ChannelLens {
    Table,
    Id,
    ServerId,
    ConversationId,
    CreatedBy,
    Name,
    FilterSpec,
    IsShared,
    CreatedAt,
    UpdatedAt,
}

#[derive(DeriveIden)]
pub enum Server {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Conversation {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
