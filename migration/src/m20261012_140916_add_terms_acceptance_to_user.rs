use sea_orm_migration::prelude::*;

/// Adds `user.terms_accepted_at` and `user.terms_accepted_url`: when an account accepted the
/// instance's terms at signup, and the terms URL it accepted.
///
/// Nullable, no backfill: accounts created before, or on an instance without terms, have none.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .add_column(
                        ColumnDef::new(User::TermsAcceptedAt)
                            .timestamp_with_time_zone()
                            .null(),
                    )
                    .add_column(ColumnDef::new(User::TermsAcceptedUrl).text().null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .drop_column(User::TermsAcceptedAt)
                    .drop_column(User::TermsAcceptedUrl)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    TermsAcceptedAt,
    TermsAcceptedUrl,
}
