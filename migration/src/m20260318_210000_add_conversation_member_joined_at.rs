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
                    .add_column(
                        timestamp_with_time_zone(ConversationMemberExt::JoinedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(ConversationMember::Table)
                    .drop_column(ConversationMemberExt::JoinedAt)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum ConversationMember {
    Table,
}

#[derive(DeriveIden)]
enum ConversationMemberExt {
    JoinedAt,
}
