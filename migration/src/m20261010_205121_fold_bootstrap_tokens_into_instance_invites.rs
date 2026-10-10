use sea_orm_migration::prelude::*;

/// Gives `instance_invite` an optional `email` binding and drops `bootstrap_token`. Bootstrap
/// tokens still usable at migration time become single-use operator invites bound to their
/// email, under the same token hash, so links already handed out keep working.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared("ALTER TABLE instance_invite ADD COLUMN email varchar(254) NULL")
            .await?;
        db.execute_unprepared(
            r#"INSERT INTO instance_invite
                   (id, created_by_user_id, token_hash, created_at, expires_at, max_uses, use_count, email)
               SELECT id, NULL, token_hash, created_at, expires_at, 1, 0, lower(trim(email))
               FROM bootstrap_token
               WHERE used_at IS NULL AND revoked_at IS NULL AND expires_at > now()
               ON CONFLICT DO NOTHING"#,
        )
        .await?;
        db.execute_unprepared("DROP TABLE IF EXISTS bootstrap_token")
            .await?;
        Ok(())
    }

    /// Recreates `bootstrap_token` empty, as the migration that created it left it, and drops
    /// the binding. Email-bound invites are revoked first so none admits any address.
    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE bootstrap_token (
                id uuid NOT NULL UNIQUE PRIMARY KEY,
                email varchar(254) NOT NULL,
                token_hash varchar(128) NOT NULL,
                created_at timestamptz NOT NULL DEFAULT CURRENT_TIMESTAMP,
                expires_at timestamptz NOT NULL,
                used_at timestamptz NULL,
                consumed_by_user_id uuid NULL,
                revoked_at timestamptz NULL,
                CONSTRAINT "fk-bootstrap-token-consumed-by-user"
                    FOREIGN KEY (consumed_by_user_id) REFERENCES "user" (id)
                    ON DELETE SET NULL ON UPDATE CASCADE
            )"#,
        )
        .await?;
        db.execute_unprepared(
            r#"CREATE UNIQUE INDEX "idx-bootstrap-token-hash-uniq" ON bootstrap_token (token_hash)"#,
        )
        .await?;
        db.execute_unprepared(
            "UPDATE instance_invite SET revoked_at = now() \
             WHERE email IS NOT NULL AND revoked_at IS NULL",
        )
        .await?;
        db.execute_unprepared("ALTER TABLE instance_invite DROP COLUMN email")
            .await?;
        Ok(())
    }
}
