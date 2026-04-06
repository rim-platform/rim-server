use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MessageMention::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(MessageMention::MessageId).uuid().not_null())
                    .col(
                        ColumnDef::new(MessageMention::MentionedUserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessageMention::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(MessageMention::MessageId)
                            .col(MessageMention::MentionedUserId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_mention_message")
                            .from(MessageMention::Table, MessageMention::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_mention_user")
                            .from(MessageMention::Table, MessageMention::MentionedUserId)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_mention_user")
                    .table(MessageMention::Table)
                    .col(MessageMention::MentionedUserId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_mention_user")
                    .table(MessageMention::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(MessageMention::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum MessageMention {
    Table,
    MessageId,
    MentionedUserId,
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
