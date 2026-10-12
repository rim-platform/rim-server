use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::{prelude::*, schema::*};

/// Adds `blocked_email`: addresses, lowercased, that may not sign up here, and the password
/// accounts holding them may not sign in. The operator's complete ban of an account adds its
/// address.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// What must be gone before `down` may run: dropping the table would lift every block silently.
const DOWN_GUARD_SQL: &str = "SELECT count(*) AS n FROM blocked_email";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(BlockedEmail::Table)
                    .col(text(BlockedEmail::Email).primary_key())
                    .col(text(BlockedEmail::Reason))
                    .col(
                        timestamp_with_time_zone(BlockedEmail::CreatedAt)
                            .default(Expr::current_timestamp()),
                    )
                    .to_owned(),
            )
            .await
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let row = manager
            .get_connection()
            .query_one_raw(Statement::from_string(DbBackend::Postgres, DOWN_GUARD_SQL))
            .await?
            .ok_or_else(|| DbErr::Custom("the down guard returned no row".to_string()))?;
        let remaining: i64 = row.try_get("", "n")?;
        if remaining > 0 {
            return Err(DbErr::Migration(
                "blocked emails exist; lift them with `rim_server principal unblock` before \
                 reverting add_blocked_email"
                    .to_string(),
            ));
        }
        manager
            .drop_table(Table::drop().table(BlockedEmail::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum BlockedEmail {
    Table,
    Email,
    Reason,
    CreatedAt,
}
