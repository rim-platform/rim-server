use sea_orm_migration::prelude::*;

/// Self-service account deletion:
///
/// - `user.deletion_requested_at` and `user.deletion_scheduled_for`: when the account asked to
///   be deleted and when its grace period ends; both null while no deletion is pending.
/// - `user.deleted_at`: set once the account is purged. The row stays as a tombstone that
///   encrypted messages and other kept rows still point at; it has no password, no email and
///   no key identity, so it cannot sign in.
/// - The two sign-in checks on `user` (a password or a principal; a principal with its identity
///   key) no longer apply to a tombstone. A tombstone keeps its principal id so encrypted
///   groups can still match its revoked devices' leaves, while its identity key is cleared.
/// - A partial index over pending deletions, which the hourly purge scans.
///
/// Nullable columns and looser checks: the previous release's server keeps working on it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user" ADD COLUMN deletion_requested_at timestamptz NULL"#,
    r#"ALTER TABLE "user" ADD COLUMN deletion_scheduled_for timestamptz NULL"#,
    r#"ALTER TABLE "user" ADD COLUMN deleted_at timestamptz NULL"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_password_or_principal"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_password_or_principal
       CHECK (password_hash IS NOT NULL OR principal_id IS NOT NULL OR deleted_at IS NOT NULL)"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal_has_identity_key"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal_has_identity_key
       CHECK ((principal_id IS NULL) = (identity_key IS NULL) OR deleted_at IS NOT NULL)"#,
    r#"CREATE INDEX idx_user_deletion_scheduled_for ON "user" (deletion_scheduled_for)
       WHERE deletion_scheduled_for IS NOT NULL AND deleted_at IS NULL"#,
];

/// Back to the previous schema. Refuses to run once a purged account exists, because the
/// previous checks cannot hold its tombstone.
const DOWN_SQL: &[&str] = &[
    r#"DO $$
    BEGIN
        IF EXISTS (SELECT 1 FROM "user" WHERE deleted_at IS NOT NULL)
        THEN
            RAISE EXCEPTION 'purged accounts exist; the previous schema cannot hold their tombstones';
        END IF;
    END
    $$"#,
    "DROP INDEX idx_user_deletion_scheduled_for",
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_principal_has_identity_key"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_principal_has_identity_key
       CHECK ((principal_id IS NULL) = (identity_key IS NULL))"#,
    r#"ALTER TABLE "user" DROP CONSTRAINT chk_user_password_or_principal"#,
    r#"ALTER TABLE "user" ADD CONSTRAINT chk_user_password_or_principal
       CHECK (password_hash IS NOT NULL OR principal_id IS NOT NULL)"#,
    r#"ALTER TABLE "user" DROP COLUMN deleted_at"#,
    r#"ALTER TABLE "user" DROP COLUMN deletion_scheduled_for"#,
    r#"ALTER TABLE "user" DROP COLUMN deletion_requested_at"#,
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
