use sea_orm_migration::sea_orm::{ConnectionTrait, DbBackend, Statement};
use sea_orm_migration::{prelude::*, schema::*};

/// Adds `ip_block`: client addresses and ranges, IPv4 or IPv6, this server refuses on every
/// route but `/healthz`, each until `expires_at` or until removed.
#[derive(DeriveMigrationName)]
pub struct Migration;

/// What must be gone before `down` may run: dropping the table would lift every block silently.
const DOWN_GUARD_SQL: &str = "SELECT count(*) AS n FROM ip_block";

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .create_table(
                Table::create()
                    .table(IpBlock::Table)
                    .col(uuid(IpBlock::Id).primary_key())
                    .col(text(IpBlock::Cidr).unique_key())
                    .col(text(IpBlock::Reason))
                    .col(timestamp_with_time_zone_null(IpBlock::ExpiresAt))
                    .col(
                        timestamp_with_time_zone(IpBlock::CreatedAt)
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
                "IP blocks exist; remove them with `rim_server ip unblock` before reverting \
                 add_ip_block"
                    .to_string(),
            ));
        }
        manager
            .drop_table(Table::drop().table(IpBlock::Table).to_owned())
            .await
    }
}

#[derive(DeriveIden)]
enum IpBlock {
    Table,
    Id,
    Cidr,
    Reason,
    ExpiresAt,
    CreatedAt,
}
