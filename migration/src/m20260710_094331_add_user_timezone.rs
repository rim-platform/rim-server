use sea_orm_migration::prelude::*;

/// Adds an optional `user.timezone` (IANA name, e.g. `"America/Chicago"`) that clients use to
/// render times in the user's local timezone.
///
/// Nullable, no backfill: existing users simply have no timezone until they set one.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .add_column(ColumnDef::new(User::Timezone).string_len(64).null())
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .drop_column(User::Timezone)
                    .to_owned(),
            )
            .await
    }
}

#[derive(DeriveIden)]
enum User {
    Table,
    Timezone,
}
