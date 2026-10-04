use crate::m20251122_154540_create_conversation::Conversation;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .add_column(uuid(Message::ConversationId).null())
                    .add_column(uuid(Message::ThreadId).null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-message-conversation-id")
                    .from(Message::Table, Message::ConversationId)
                    .to(Conversation::Table, Conversation::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-message-thread-id")
                    .from(Message::Table, Message::ThreadId)
                    .to(MessageThread::Table, MessageThread::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-message-conversation-sent-at-id")
                    .table(Message::Table)
                    .col(Message::ConversationId)
                    .col(Message::SentAt)
                    .col(Message::Id)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx-message-thread-sent-at-id")
                    .table(Message::Table)
                    .col(Message::ThreadId)
                    .col(Message::SentAt)
                    .col(Message::Id)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE message m
                SET conversation_id = mt.conversation_id,
                    thread_id = CASE
                        WHEN c.main_thread_id = m.message_thread_id THEN NULL
                        ELSE m.message_thread_id
                    END
                FROM message_thread mt
                JOIN conversation c ON c.id = mt.conversation_id
                WHERE mt.id = m.message_thread_id
                  AND m.conversation_id IS NULL
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .modify_column(uuid(Message::ConversationId).not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx-message-thread-sent-at-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx-message-conversation-sent-at-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-message-thread-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-message-conversation-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .drop_column(Message::ThreadId)
                    .drop_column(Message::ConversationId)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Message {
    Table,
    Id,
    ConversationId,
    ThreadId,
    SentAt,
}

#[derive(DeriveIden)]
enum MessageThread {
    Table,
    Id,
}
