use sea_orm_migration::prelude::*;

/// Makes `user.password_hash` and `user.email` nullable, for accounts created from a key
/// identity alone (no password, no email). A row must still have a way to sign in: a password
/// hash or a principal id (a key-only account always has a principal). The unique index on
/// `LOWER(email)` is unchanged; Postgres lets any number of rows hold NULL.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user" ALTER COLUMN password_hash DROP NOT NULL"#,
    r#"ALTER TABLE "user" ALTER COLUMN email DROP NOT NULL"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_password_or_principal
       CHECK (password_hash IS NOT NULL OR principal_id IS NOT NULL)"#,
];

/// Back to NOT NULL. Refuses to run once an account without a password or an email exists,
/// because the previous schema cannot hold it.
const DOWN_SQL: &[&str] = &[
    r#"DO $$
    BEGIN
        IF EXISTS (SELECT 1 FROM "user" WHERE password_hash IS NULL OR email IS NULL)
        THEN
            RAISE EXCEPTION 'accounts without a password or an email exist; the previous schema cannot hold them';
        END IF;
    END
    $$"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_password_or_principal"#,
    r#"ALTER TABLE "user" ALTER COLUMN email SET NOT NULL"#,
    r#"ALTER TABLE "user" ALTER COLUMN password_hash SET NOT NULL"#,
];

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in UP_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
