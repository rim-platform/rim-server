use sea_orm_migration::prelude::*;

/// Adds `instance_default_workspace`: the one row (`id = 1`) that records the default workspace
/// a definition file created, and the owner claim waiting for it.
///
/// - `workspace_id`: the workspace the claim created; null until it is claimed, and again when
///   the workspace is deleted.
/// - `claim_invite_id`: the single-use instance invite that claims it; null once claimed.
/// - `claimed_by_user_id`, `claimed_at`: the account that claimed it and when.
///
/// A new table: the previous release's server never reads it.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = r#"CREATE TABLE instance_default_workspace (
    id smallint PRIMARY KEY CHECK (id = 1),
    workspace_id uuid NULL REFERENCES server (id) ON DELETE SET NULL,
    claim_invite_id uuid NULL REFERENCES instance_invite (id) ON DELETE SET NULL,
    claimed_by_user_id uuid NULL REFERENCES "user" (id) ON DELETE SET NULL,
    claimed_at timestamptz NULL,
    updated_at timestamptz NOT NULL DEFAULT now()
)"#;

const DOWN_SQL: &str = "DROP TABLE instance_default_workspace";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared(DOWN_SQL)
            .await?;
        Ok(())
    }
}
