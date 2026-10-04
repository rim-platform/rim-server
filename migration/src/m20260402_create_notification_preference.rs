use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(NotificationPreference::Table)
                    .if_not_exists()
                    .col(
                        ColumnDef::new(NotificationPreference::UserId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NotificationPreference::ConversationId)
                            .uuid()
                            .not_null(),
                    )
                    .col(
                        ColumnDef::new(NotificationPreference::MutedUntil)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .primary_key(
                        Index::create()
                            .col(NotificationPreference::UserId)
                            .col(NotificationPreference::ConversationId),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_notification_preference_user")
                            .from(
                                NotificationPreference::Table,
                                NotificationPreference::UserId,
                            )
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk_notification_preference_conversation")
                            .from(
                                NotificationPreference::Table,
                                NotificationPreference::ConversationId,
                            )
                            .to(Conversation::Table, Conversation::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(
                Table::drop()
                    .table(NotificationPreference::Table)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
pub enum NotificationPreference {
    Table,
    UserId,
    ConversationId,
    MutedUntil,
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
