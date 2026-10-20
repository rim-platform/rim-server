use sea_orm_migration::prelude::*;

/// Adds what handles are built from for people from other servers.
///
/// - `user.home_username`, `user.home_handle_domain`: a remote user's username at their home
///   relay and that relay's verified handle domain, set together or not at all.
/// - `idx_user_home_handle`: finds a remote user by handle.
/// - `peer_server.handle_domain`: the handle domain a peer's descriptor declares, once the
///   domain was checked to point back at the peer; null before and when it does not.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"ALTER TABLE "user"
        ADD COLUMN home_username text,
        ADD COLUMN home_handle_domain text,
        ADD CONSTRAINT chk_user_home_handle
            CHECK ((home_username IS NULL) = (home_handle_domain IS NULL))"#,
    r#"CREATE INDEX idx_user_home_handle ON "user" (home_handle_domain, home_username)
        WHERE home_username IS NOT NULL"#,
    "ALTER TABLE peer_server ADD COLUMN handle_domain text",
];

const DOWN_SQL: &[&str] = &[
    "ALTER TABLE peer_server DROP COLUMN handle_domain",
    "DROP INDEX idx_user_home_handle",
    r#"ALTER TABLE "user"
        DROP CONSTRAINT chk_user_home_handle,
        DROP COLUMN home_handle_domain,
        DROP COLUMN home_username"#,
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
