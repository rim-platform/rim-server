use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::{prelude::*, schema::*};

use crate::m20251012_135522_create_user::User;

/// Removes the username and password accounts: the table `password_reset_token` of their email
/// reset, the column `user.password_hash` and the check that let an account have a password
/// instead of a principal. A live account now always has a principal:
/// `chk_user_principal CHECK (principal_id IS NOT NULL OR deleted_at IS NOT NULL)`.
///
/// Refuses to run while a live account has no key identity: this release needs a fresh database.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// The live accounts without a key identity, which the new check would refuse.
const UP_GUARD_SQL: &str =
    r#"SELECT count(*) AS n FROM "user" WHERE principal_id IS NULL AND deleted_at IS NULL"#;

const UP_SQL: &[&str] = &[
    "DROP TABLE password_reset_token",
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_password_or_principal"#,
    r#"ALTER TABLE "user" DROP COLUMN password_hash"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal
       CHECK (principal_id IS NOT NULL OR deleted_at IS NOT NULL)"#,
];

const DOWN_SQL: &[&str] = &[
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal"#,
    r#"ALTER TABLE "user" ADD COLUMN password_hash varchar(256) NULL"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_password_or_principal
       CHECK (password_hash IS NOT NULL OR principal_id IS NOT NULL OR deleted_at IS NOT NULL)"#,
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let row = conn
            .query_one_raw(Statement::from_string(DbBackend::Postgres, UP_GUARD_SQL))
            .await?
            .ok_or_else(|| DbErr::Custom("the up guard returned no row".to_string()))?;
        let without_key: i64 = row.try_get("", "n")?;
        if without_key > 0 {
            return Err(DbErr::Custom(format!(
                "this release needs a fresh database: {without_key} local accounts have no key \
                 identity"
            )));
        }
        for sql in UP_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    /// Restores the column as nullable, the old check, and the reset token table as
    /// `create_password_reset_token` created it, empty.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
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
