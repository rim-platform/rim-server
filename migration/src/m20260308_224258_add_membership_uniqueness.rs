use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_index(
                Index::create()
                    .name("idx_server_member_server_user_unique")
                    .table(ServerMember::Table)
                    .col(ServerMember::ServerId)
                    .col(ServerMember::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        manager
            .create_index(
                Index::create()
                    .name("idx_conversation_member_conversation_user_unique")
                    .table(ConversationMember::Table)
                    .col(ConversationMember::ConversationId)
                    .col(ConversationMember::UserId)
                    .unique()
                    .to_owned(),
            )
            .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_index(
                Index::drop()
                    .name("idx_conversation_member_conversation_user_unique")
                    .table(ConversationMember::Table)
                    .to_owned(),
            )
            .await?;

        manager
            .drop_index(
                Index::drop()
                    .name("idx_server_member_server_user_unique")
                    .table(ServerMember::Table)
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}

#[derive(DeriveIden)]
enum ServerMember {
    Table,
    ServerId,
    UserId,
}

#[derive(DeriveIden)]
enum ConversationMember {
    Table,
    ConversationId,
    UserId,
}
