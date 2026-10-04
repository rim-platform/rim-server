use crate::m20251012_135522_create_user::User;
use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Fresh installs already use 256, but existing DBs may still have 64.
        manager
            .alter_table(
                Table::alter()
                    .table(User::Table)
                    .modify_column(
                        ColumnDef::new(User::PasswordHash)
                            .string_len(256)
                            .not_null(),
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
                    .modify_column(ColumnDef::new(User::PasswordHash).string_len(64).not_null())
                    .to_owned(),
            )
            .await
    }
}
