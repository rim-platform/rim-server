use crate::m20251012_135522_create_user::User;
use sea_orm_migration::{prelude::*, schema::*};

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(PasswordResetToken::Table)
                    .if_not_exists()
                    .col(uuid_uniq(PasswordResetToken::Id).primary_key())
                    .col(uuid(PasswordResetToken::UserId).not_null())
                    .col(
                        string(PasswordResetToken::TokenHash)
                            .not_null()
                            .string_len(128),
                    )
                    .col(timestamp_with_time_zone(PasswordResetToken::ExpiresAt).not_null())
                    .col(timestamp_with_time_zone(PasswordResetToken::UsedAt).null())
                    .col(
                        timestamp_with_time_zone(PasswordResetToken::CreatedAt)
                            .not_null()
                            .default(Expr::current_timestamp()),
                    )
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-password-reset-token-user")
                            .from(PasswordResetToken::Table, PasswordResetToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade)
                            .on_update(ForeignKeyAction::Cascade),
                    )
                    .index(
                        Index::create()
                            .name("idx-password-reset-token-hash-uniq")
                            .table(PasswordResetToken::Table)
                            .col(PasswordResetToken::TokenHash)
                            .unique(),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(PasswordResetToken::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum PasswordResetToken {
    Table,
    Id,
    UserId,
    TokenHash,
    ExpiresAt,
    UsedAt,
    CreatedAt,
}
