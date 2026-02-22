use crate::m20251122_154540_create_conversation::Conversation;
use crate::m20251123_105135_create_threads_message::MessageThread;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .add_column(uuid(Conversation::MainThreadId).null())
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-conversation-main-thread-id")
                    .from(Conversation::Table, Conversation::MainThreadId)
                    .to(MessageThread::Table, MessageThread::Id)
                    .on_delete(ForeignKeyAction::SetNull)
                    .on_update(ForeignKeyAction::Cascade)
                    .to_owned(),
            )
            .await?;

        // Backfill main thread for existing conversations.
        //
        // For each conversation, create a dedicated main thread and set `conversation.main_thread_id`.
        // We keep it nullable for now to avoid failing on partially migrated environments.
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
                WHERE c.id = nt.conversation_id
                "#,
            )
            .await?;

        // Enforce NOT NULL after backfill.
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .modify_column(uuid(Conversation::MainThreadId).not_null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(Conversation::Table)
                    .drop_column(Conversation::MainThreadId)
                    .to_owned(),
            )
            .await
    }
}
