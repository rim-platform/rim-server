use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MessagePin::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(MessagePin::MessageId)
                            .uuid()
                            .not_null()
                            .primary_key(),
                    )
                    .col(
                        ColumnDef::new(MessagePin::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessagePin::PinnedBy)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(MessagePin::PinnedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_pin_message")
                            .from(MessagePin::Table, MessagePin::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_pin_conversation")
                            .from(MessagePin::Table, MessagePin::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_message_pin_user")
                            .from(MessagePin::Table, MessagePin::PinnedBy)
                            .to(User::Table, User::Id),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_pin_conversation")
                    .table(MessagePin::Table)
                    .col(MessagePin::ConversationId)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_pin_conversation")
                    .table(MessagePin::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(MessagePin::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum MessagePin {
    Table,
    MessageId,
    ConversationId,
    PinnedBy,
    PinnedAt,
}

#[derive(DeriveIden)]
pub enum Message {
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
