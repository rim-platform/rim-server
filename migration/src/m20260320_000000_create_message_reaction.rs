use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MessageReaction::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MessageReaction::Id)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(ColumnDef::new(MessageReaction::MessageId).uuid().not_null())
                    .col(ColumnDef::new(MessageReaction::UserId).uuid().not_null())
                    .col(ColumnDef::new(MessageReaction::Emoji).string().not_null())
                    .col(
                        ColumnDef::new(MessageReaction::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_reaction_message")
                            .from(MessageReaction::Table, MessageReaction::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_reaction_user")
                            .from(MessageReaction::Table, MessageReaction::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_reaction_message_id")
                    .table(MessageReaction::Table)
                    .col(MessageReaction::MessageId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("uq_message_reaction_message_user_emoji")
                    .table(MessageReaction::Table)
                    .col(MessageReaction::MessageId)
                    .col(MessageReaction::UserId)
                    .col(MessageReaction::Emoji)
                    .unique()
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("uq_message_reaction_message_user_emoji")
                    .table(MessageReaction::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_reaction_message_id")
                    .table(MessageReaction::Table)
                    .to_owned(),
            )
            .await?;
        manager
            .drop_table(Table::drop().table(MessageReaction::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum MessageReaction {
    Table,
    Id,
    MessageId,
    UserId,
    Emoji,
    CreatedAt,
}

#[derive(DeriveIden)]
pub enum Message {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}
