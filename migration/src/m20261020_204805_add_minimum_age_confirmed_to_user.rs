use sea_orm_migration::prelude::*;

/// Adds `user.minimum_age_confirmed`: the minimum age the account confirmed having reached at
/// signup, as the instance asked it then. Self-declared; no birth date or age is stored.
///
/// Nullable, no backfill: accounts created before, or on an instance asking no age, have none.
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
                        ColumnDef::new(User::MinimumAgeConfirmed)
                            .small_integer()
                            .null(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .drop_column(User::MinimumAgeConfirmed)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    MinimumAgeConfirmed,
}
