use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(ConversationReadState::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(ConversationReadState::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationReadState::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(ConversationReadState::LastReadAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .primary_key(
                        Index::create()
                            .col(ConversationReadState::UserId)
                            .col(ConversationReadState::ConversationId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_read_state_user")
                            .from(ConversationReadState::Table, ConversationReadState::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_read_state_conversation")
                            .from(
                                ConversationReadState::Table,
                                ConversationReadState::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_read_state_user")
                    .table(ConversationReadState::Table)
                    .col(ConversationReadState::UserId)
                    .to_owned(),
            )
            .await?;

        // Partial index for efficient unread count queries.
        // SeaORM doesn't support WHERE clauses on indexes, so this is raw SQL.
        manager
            .get_connection()
            .execute_unprepared(
                "CREATE INDEX IF NOT EXISTS idx_message_unread_count \
                 ON message (conversation_id, sent_at) \
                 WHERE thread_id IS NULL AND is_deleted = false",
            )
            .await
            .map_err(|e| DbErr::Custom(format!("Failed to create partial index: {e}")))?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP INDEX IF EXISTS idx_message_unread_count")
            .await
            .map_err(|e| DbErr::Custom(format!("Failed to drop partial index: {e}")))?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_read_state_user")
                    .table(ConversationReadState::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_table(Table::drop().table(ConversationReadState::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum ConversationReadState {
    Table,
    UserId,
    ConversationId,
    LastReadAt,
}

#[derive(DeriveIden)]
pub enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
pub enum Conversation {
    Table,
    Id,
}
