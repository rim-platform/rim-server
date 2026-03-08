use crate::m20251123_105135_create_threads_message::MessageThread;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(
                r#"
                CREATE UNIQUE INDEX IF NOT EXISTS idx_message_thread_root_message_unique
                ON message_thread (first_message_id)
                WHERE first_message_id IS NOT NULL
                "#,
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_thread_root_message_unique")
                    .table(MessageThread::Table)
                    .to_owned(),
            )
            .await
    }
}
