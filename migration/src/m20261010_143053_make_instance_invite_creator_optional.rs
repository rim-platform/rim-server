use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// Lets an instance invite have no creating user: the operator issues invites from the command
/// line, also on a relay where no one owns a workspace.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &str = "ALTER TABLE instance_invite ALTER COLUMN created_by_user_id DROP NOT NULL";

const DOWN_SQL: &str = "ALTER TABLE instance_invite ALTER COLUMN created_by_user_id SET NOT NULL";

/// Operator invites, which `down` cannot keep.
const DOWN_GUARD_SQL: &str =
    "SELECT count(*)::int8 AS n FROM instance_invite WHERE created_by_user_id IS NULL";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager.get_connection().execute_unprepared(UP_SQL).await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let conn = manager.get_connection();
        let row = conn
            .query_one_raw(Statement::from_string(DbBackend::Postgres, DOWN_GUARD_SQL))
            .await?
            .ok_or_else(|| DbErr::Custom("the down guard returned no row".to_string()))?;
        let remaining: i64 = row.try_get("", "n")?;
        if remaining > 0 {
            return Err(DbErr::Migration(
                "instance invites issued from the command line exist; delete them before \
                 reverting make_instance_invite_creator_optional"
                    .to_string(),
            ));
        }
        conn.execute_unprepared(DOWN_SQL).await?;
        Ok(())
    }
}
