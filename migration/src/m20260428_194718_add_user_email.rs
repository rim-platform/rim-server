use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    /// Add `user.email` column (NOT NULL, case-insensitive unique index).
    ///
    /// Four-phase approach so the ALTER TABLE never rejects existing rows:
    /// 1. Add column as NULLable.
    /// 2. Backfill existing rows with a synthetic `<username>@local.invalid` placeholder
    ///    (RFC 2606 reserves `.invalid` for exactly this purpose).
    /// 3. Alter column to NOT NULL.
    /// 4. Create a case-insensitive unique index on `LOWER(email)`.
    ///
    /// Operators with existing accounts MUST update their email via the
    /// profile-edit mutation before relying on password reset or notifications.
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        // Phase 1: add nullable column.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("user"))
                    .add_column(ColumnDef::new(Alias::new("email")).string_len(254).null())
                    .to_owned(),
            )
            .await?;

        // Phase 2: backfill — synthetic placeholder for every existing row.
        manager
            .get_connection()
            .execute_unprepared(
                r#"UPDATE "user" SET email = LOWER(username) || '@local.invalid' WHERE email IS NULL"#,
            )
            .await?;

        // Phase 3: enforce NOT NULL now that every row has a value.
        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("user"))
                    .modify_column(
                        ColumnDef::new(Alias::new("email"))
                            .string_len(254)
                            .not_null(),
                    )
                    .to_owned(),
            )
            .await?;

        // Phase 4: case-insensitive unique index.
        manager
            .get_connection()
            .execute_unprepared(
                r#"CREATE UNIQUE INDEX "idx_user_email_lower_unique" ON "user" (LOWER(email))"#,
            )
            .await?;

        Ok(())
    }

    /// Drop the email column (and its index).
    ///
    /// **Destructive** — all per-user email data is lost. Down migration is
    /// only safe when the deployment is also rolling back to a binary where
    /// `signupWithInvite` does not require `email` in its input.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(r#"DROP INDEX IF EXISTS "idx_user_email_lower_unique""#)
            .await?;

        manager
            .alter_table(
                Table::alter()
                    .table(Alias::new("user"))
                    .drop_column(Alias::new("email"))
                    .to_owned(),
            )
            .await?;

        Ok(())
    }
}
