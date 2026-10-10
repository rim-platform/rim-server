use sea_orm_migration::prelude::*;
use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};

/// Adds the operator's abuse tools of a public relay.
///
/// - `relay_principal_block`: principals this server refuses: their sessions, signups and
///   deliveries.
/// - `relay_abuse_report`: reports from users (`reportDmRequest`) and from the abuse address,
///   open until an operator resolves them.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "CREATE TABLE relay_principal_block (principal_id text PRIMARY KEY, reason text NOT NULL, \
     created_at timestamptz NOT NULL DEFAULT now())",
    r#"CREATE TABLE relay_abuse_report (id uuid PRIMARY KEY,
     reporter_user_id uuid NULL REFERENCES "user" ON DELETE SET NULL,
     subject_principal_id text NULL, subject_server_id text NULL,
     request_id uuid NULL REFERENCES relay_dm_request ON DELETE SET NULL, reason text NOT NULL,
     created_at timestamptz NOT NULL DEFAULT now(), resolved_at timestamptz NULL)"#,
];

const DOWN_SQL: &[&str] = &[
    "DROP TABLE relay_abuse_report",
    "DROP TABLE relay_principal_block",
];

/// What must be gone before `down` may run: dropping a block would lift it and dropping a
/// report would lose it, both silently.
const DOWN_GUARD_SQL: &str = "SELECT (SELECT count(*) FROM relay_principal_block) \
     + (SELECT count(*) FROM relay_abuse_report) AS n";

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
        let row = conn
            .query_one_raw(Statement::from_string(DbBackend::Postgres, DOWN_GUARD_SQL))
            .await?
            .ok_or_else(|| DbErr::Custom("the down guard returned no row".to_string()))?;
        let remaining: i64 = row.try_get("", "n")?;
        if remaining > 0 {
            return Err(DbErr::Migration(
                "principal blocks or abuse reports exist; remove them before reverting \
                 add_relay_abuse"
                    .to_string(),
            ));
        }
        for sql in DOWN_SQL {
            conn.execute_unprepared(sql).await?;
        }
        Ok(())
    }
}
