use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::ConnectionTrait;

/// Adds each person's choice of who may start a DM with them.
///
/// - `user_dm_setting`: DMs from outside their workspaces: `requests` or `none`.
/// - `user_workspace_dm_setting`: per workspace, DMs from its members: `direct` or `requests`.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    r#"CREATE TABLE user_dm_setting (
     user_id uuid PRIMARY KEY REFERENCES "user" ON DELETE CASCADE,
     outside text NOT NULL DEFAULT 'requests' CHECK (outside IN ('requests', 'none')),
     updated_at timestamptz NOT NULL DEFAULT now())"#,
    r#"CREATE TABLE user_workspace_dm_setting (
     user_id uuid NOT NULL REFERENCES "user" ON DELETE CASCADE,
     server_id uuid NOT NULL REFERENCES server ON DELETE CASCADE,
     members text NOT NULL CHECK (members IN ('direct', 'requests')),
     updated_at timestamptz NOT NULL DEFAULT now(),
     PRIMARY KEY (user_id, server_id))"#,
    "CREATE INDEX idx_user_workspace_dm_setting_server ON user_workspace_dm_setting (server_id)",
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE user_workspace_dm_setting",
    "DROP TABLE user_dm_setting",
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
