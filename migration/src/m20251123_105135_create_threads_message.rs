use crate::m20251122_154540_create_conversation::Conversation;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(MessageThread::Table)
                    .if_not_exists()
                    .col(uuid_uniq(MessageThread::Id).primary_key())
                    .col(uuid(MessageThread::ConversationId).not_null())
                    .col(uuid(MessageThread::FirstMessageId).null())
                    .col(timestamp_with_time_zone(MessageThread::CreatedAt))
                    .col(timestamp_with_time_zone(MessageThread::LastMessageAt))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message_thread-conversation-id")
                            .from(MessageThread::Table, MessageThread::ConversationId)
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(Message::Table)
                    .if_not_exists()
                    .col(uuid_uniq(Message::Id).primary_key())
                    .col(uuid(Message::MessageThreadId).not_null())
                    .col(uuid(Message::ParentMessageId).null())
                    .col(
                        timestamp_with_time_zone(Message::SentAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .col(uuid(Message::SentBy).not_null())
                    .col(string(Message::Text).string_len(1024).null())
                    .col(boolean(Message::IsDeleted).not_null().default(false))
                    .col(boolean(Message::HasAttachments).not_null().default(false))
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-message-message_thread-id")
                            .from(Message::Table, Message::MessageThreadId)
                            .to(MessageThread::Table, MessageThread::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-parent-message-id")
                            .from(Message::Table, Message::ParentMessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .check(
                        Expr::cust("length(trim(text)) > 0")
                            .or(Expr::col(Message::HasAttachments).eq(true)),
                    )
                    .to_owned(),
            )
            .await?;
        manager
            .create_table(
                Table::create()
                    .table(MessageAttachment::Table)
                    .if_not_exists()
                    .col(uuid_uniq(MessageAttachment::Id).primary_key())
                    .col(
                        string(MessageAttachment::StorageUrl)
                            .not_null()
                            .string_len(1024),
                    )
                    .col(uuid(MessageAttachment::MessageId).not_null())
                    .col(big_integer(MessageAttachment::FileSize).not_null())
                    .col(string(MessageAttachment::FileName).not_null())
                    .col(string(MessageAttachment::MimeType).not_null())
                    .col(json(MessageAttachment::Metadata).null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-attachment-message-id")
                            .from(MessageAttachment::Table, MessageAttachment::MessageId)
                            .to(Message::Table, Message::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(MessageThread::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
pub enum MessageThread {
    Table,
    Id,
    ConversationId,
    FirstMessageId,
    CreatedAt,
    LastMessageAt,
}

#[derive(DeriveIden)]
#[allow(clippy::enum_variant_names)]
enum Message {
    Table,
    Id,
    MessageThreadId,
    ParentMessageId,
    SentAt,
    SentBy,
    Text,
    IsDeleted,
    HasAttachments,
}

#[derive(DeriveIden)]
enum MessageAttachment {
    Table,
    Id,
    MessageId,
    StorageUrl,
    FileName,
    FileSize,
    MimeType,
    Metadata,
}
