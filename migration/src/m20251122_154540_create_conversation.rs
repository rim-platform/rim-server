use sea_orm_migration::{prelude::*, schema::*};
use sea_orm_migration::sea_query::extension::postgres::Type;
use crate::m20251012_135522_create_user::User;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Create conversation type
        manager
            .create_type(
                Type::create()
                    .as_enum(ConversationType::Table)
                    .values([
                        ConversationType::Direct,
                        ConversationType::Group,
                        ConversationType::Channel,
                    ])
                    .to_owned()
            )
            .await?;
        // Create conversation table
        manager
            .create_table(
                Table::create()
                    .table(Conversation::Table)
                    .if_not_exists()
                    .col(pk_auto(Conversation::Id))
                    .col(
                        enumeration(
                            Conversation::ConversationType,
                            ConversationType::Table,
                            [
                                ConversationType::Direct,
                                ConversationType::Group,
                                ConversationType::Channel,
                            ]
                        )
                    )
                    .col(
                        timestamp_with_time_zone(Conversation::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp())
                    )
                    .to_owned()
            ).await?;
        // Create conversation member table
        manager.create_table(
            Table::create()
                .table(ConversationMember::Table)
                .if_not_exists()
                .col(
                    pk_auto(
                        ConversationMember::Id
                    )
                )
                .col(
                    integer(
                        ConversationMember::UserId
                    ).not_null()
                )
                .col(
                    integer(
                        ConversationMember::ConversationId
                    ).not_null()
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-conversation-member")
                        .from(ConversationMember::Table, ConversationMember::ConversationId)
                        .to(Conversation::Table, Conversation::Id)
                        .on_delete(ForeignKeyAction::Cascade)
                        .on_update(ForeignKeyAction::Cascade)
                )
                .foreign_key(
                    ForeignKey::create()
                        .name("fk-user-conversation")
                        .from(ConversationMember::Table, ConversationMember::UserId)
                        .to(User::Table, User::Id)
                        .on_delete(ForeignKeyAction::Cascade)
                        .on_update(ForeignKeyAction::Cascade)
                )
                .to_owned()
        ).await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(ConversationType::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(Conversation::Table).to_owned())
            .await?;
        manager
            .drop_table(Table::drop().table(ConversationMember::Table).to_owned())
            .await

    }
}

#[derive(DeriveIden)]
enum ConversationType {
    #[sea_orm(iden = "conversation_type")]
    Table,
    #[sea_orm(iden = "direct")]
    Direct,
    #[sea_orm(iden = "group")]
    Group,
    #[sea_orm(iden = "channel")]
    Channel,
}

#[derive(DeriveIden)]
pub enum Conversation {
    Table,
    Id,
    ConversationType,
    CreatedAt,
}


#[derive(DeriveIden)]
enum ConversationMember {
    Table,
    Id,
    ConversationId,
    UserId,
}