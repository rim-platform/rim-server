use crate::m20251122_154540_create_conversation::Conversation;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-message-message_thread-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .drop_column(Message::MessageThreadId)
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::MainThreadId)
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(uuid(Conversation::MainThreadId).null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                WITH new_threads AS (
                  INSERT INTO message_thread (id, conversation_id, created_at, last_message_at)
                  SELECT gen_random_uuid(), c.id, NOW(), NOW()
                  FROM conversation c
                  WHERE c.main_thread_id IS NULL
                  RETURNING id, conversation_id
                )
                UPDATE conversation c
                SET main_thread_id = nt.id
                FROM new_threads nt
                WHERE c.id = nt.conversation_id;

                UPDATE conversation c
                SET main_thread_id = mt.id
                FROM message_thread mt
                WHERE c.main_thread_id IS NULL
                  AND mt.conversation_id = c.id
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .modify_column(uuid(Conversation::MainThreadId).not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .add_column(uuid(Message::MessageThreadId).null())
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE message m
                SET message_thread_id = COALESCE(m.thread_id, c.main_thread_id)
                FROM conversation c
                WHERE c.id = m.conversation_id
                  AND m.message_thread_id IS NULL
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Message::Table)
                    .modify_column(uuid(Message::MessageThreadId).not_null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-message-message_thread-id")
                    .from(Message::Table, Message::MessageThreadId)
                    .to(MessageThread::Table, MessageThread::Id)
                    .on_delete(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum Message {
    Table,
    MessageThreadId,
}

#[derive(DeriveIden)]
enum MessageThread {
    Table,
    Id,
}
