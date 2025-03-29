use sea_orm_migration::prelude::*;

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
                    .col(ColumnDef::new(PasswordResetToken::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(PasswordResetToken::UserId).uuid().not_null())
                    .col(ColumnDef::new(PasswordResetToken::Token).string().not_null().unique_key())
                    .col(ColumnDef::new(PasswordResetToken::ExpiresAt).timestamp().not_null())
                    .col(ColumnDef::new(PasswordResetToken::CreatedAt).timestamp().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-password_reset-user_id")
                            .from(PasswordResetToken::Table, PasswordResetToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
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
    Token,
    ExpiresAt,
    CreatedAt,
}

#[derive(DeriveIden)]
enum User {
    Table,
    Id,
}
