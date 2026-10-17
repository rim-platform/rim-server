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
                    .table(Conversation::Table)
                    .add_column(uuid(Conversation::MainThreadId).null())
                    .to_owned(),
            )
            .await?;

        // No FK from conversation.main_thread_id to message_thread.id:
        // message_thread.conversation_id already references conversation, and a second immediate
        // FK would form a cycle that makes inserts impossible without DEFERRABLE constraints.

        // Backfill main thread for existing conversations.
        //
        // For each conversation, create a dedicated main thread and set `conversation.main_thread_id`.
        // The column starts nullable so existing rows stay valid until this backfill fills it.
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
