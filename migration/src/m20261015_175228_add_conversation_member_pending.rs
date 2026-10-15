use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationMember::Table)
                    .add_column(timestamp_with_time_zone_null(
                        ConversationMember::PendingSince,
                    ))
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationMember::Table)
                    .drop_column(ConversationMember::PendingSince)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationMember {
    Table,
    PendingSince,
}
