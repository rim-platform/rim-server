use sea_orm_migration::prelude::*;

/// Adds an optional `user.timezone` (IANA name, e.g. `"America/Chicago"`), used only for (a)
/// rendering on-call times in the viewer's local timezone on the FE and (b) defaulting a new
/// on-call schedule's timezone. Rotation handoff math lives on the **schedule's** timezone, not
/// the user's — see `docs/oncall.md` and `rim_core::oncall::model::OnCallSchedule` for why.
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
