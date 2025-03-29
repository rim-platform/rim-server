use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(EmailVerificationToken::Table)
                    .if_not_exists()
                    .col(ColumnDef::new(EmailVerificationToken::Id).uuid().not_null().primary_key())
                    .col(ColumnDef::new(EmailVerificationToken::UserId).uuid().not_null())
                    .col(ColumnDef::new(EmailVerificationToken::Token).string().not_null().unique_key())
                    .col(ColumnDef::new(EmailVerificationToken::ExpiresAt).timestamp().not_null())
                    .col(ColumnDef::new(EmailVerificationToken::CreatedAt).timestamp().not_null())
                    .foreign_key(
                        ForeignKey::create()
                            .name("fk-email_verification-user_id")
                            .from(EmailVerificationToken::Table, EmailVerificationToken::UserId)
                            .to(User::Table, User::Id)
                            .on_delete(ForeignKeyAction::Cascade),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .drop_table(Table::drop().table(EmailVerificationToken::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum EmailVerificationToken {
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
