use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_member_user_conversation")
                    .table(ConversationMember::Table)
                    .col(ConversationMember::UserId)
                    .col(ConversationMember::ConversationId)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_server_type_last_activity_id")
                    .table(Conversation::Table)
                    .col(Conversation::ServerId)
                    .col(Conversation::ConversationType)
                    .col(Conversation::LastActivityAt)
                    .col(Conversation::Id)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_server_type_created_id")
                    .table(Conversation::Table)
                    .col(Conversation::ServerId)
                    .col(Conversation::ConversationType)
                    .col(Conversation::CreatedAt)
                    .col(Conversation::Id)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_thread_conversation_last_message_id")
                    .table(MessageThread::Table)
                    .col(MessageThread::ConversationId)
                    .col(MessageThread::LastMessageAt)
                    .col(MessageThread::Id)
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_message_attachment_message_id")
                    .table(MessageAttachment::Table)
                    .col(MessageAttachment::MessageId)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE message_thread
                SET created_at = COALESCE(created_at, CURRENT_TIMESTAMP),
                    last_message_at = COALESCE(last_message_at, CURRENT_TIMESTAMP)
                WHERE created_at IS NULL OR last_message_at IS NULL
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(MessageThread::Table)
                    .modify_column(
                        ColumnDef::new(MessageThread::CreatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .modify_column(
                        ColumnDef::new(MessageThread::LastMessageAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await?;

        manager
            .create_foreign_key(
                ForeignKey::create()
                    .name("fk-message-sent-by-user-id")
                    .from(Message::Table, Message::SentBy)
                    .to(User::Table, User::Id)
                    .on_delete(ForeignKeyAction::Restrict)
                    .to_owned(),
            )
            .await?;

        manager
            .get_connection()
            .execute_unprepared(
                r#"
                UPDATE user_command
                SET updated_at = COALESCE(updated_at, created_at, CURRENT_TIMESTAMP)
                WHERE updated_at IS NULL
                "#,
            )
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(UserCommand::Table)
                    .modify_column(
                        ColumnDef::new(UserCommand::UpdatedAt)
                            .timestamp_with_time_zone()
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_foreign_key(
                ForeignKey::drop()
                    .name("fk-message-sent-by-user-id")
                    .table(Message::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_attachment_message_id")
                    .table(MessageAttachment::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_message_thread_conversation_last_message_id")
                    .table(MessageThread::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_server_type_created_id")
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_server_type_last_activity_id")
                    .table(Conversation::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_member_user_conversation")
                    .table(ConversationMember::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
enum Conversation {
    Table,
    Id,
    ServerId,
    ConversationType,
    CreatedAt,
    LastActivityAt,
}

#[derive(DeriveIden)]
enum ConversationMember {
    Table,
    UserId,
    ConversationId,
}

#[derive(DeriveIden)]
enum MessageThread {
    Table,
    Id,
    ConversationId,
    CreatedAt,
    LastMessageAt,
}

#[derive(DeriveIden)]
enum MessageAttachment {
    Table,
    MessageId,
}

#[derive(DeriveIden)]
enum Message {
    Table,
    SentBy,
}

#[derive(DeriveIden)]
enum UserCommand {
    Table,
    UpdatedAt,
}
