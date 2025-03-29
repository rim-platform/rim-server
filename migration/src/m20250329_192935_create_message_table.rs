use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.create_table(
            Table::create()
                .table(Message::Table)
                .if_not_exists()
                .col(ColumnDef::new(Message::Id).uuid().not_null().primary_key())
                .col(ColumnDef::new(Message::ChannelId).uuid().not_null())
                .col(ColumnDef::new(Message::UserId).uuid().not_null())
                .col(ColumnDef::new(Message::Content).text().not_null())
                .col(ColumnDef::new(Message::CreatedAt).timestamp().not_null())
                // Foreign key (channel)
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-message-channel_id")
                        .from(Message::Table, Message::ChannelId)
                        .to(Channel::Table, Channel::Id)
                        .on_delete(ForeignKeyAction::Cascade)
                )
                // Foreign key (user)
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-message-user_id")
                        .from(Message::Table, Message::UserId)
                        .to(User::Table, User::Id)
                        .on_delete(ForeignKeyAction::SetNull)
                )
                .to_owned(),
        )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.drop_table(Table::drop().table(Message::Table).to_owned()).await
    }
}

#[derive(DeriveIden)]
enum Message {
    Table,
    Id,
    ChannelId,
    UserId,
    Content,
    CreatedAt,
}

#[derive(DeriveIden)]
enum Channel {
    Table,
    Id,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
