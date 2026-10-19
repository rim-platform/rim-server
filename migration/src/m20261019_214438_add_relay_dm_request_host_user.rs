use sea_orm_migration::prelude::*;

/// Adds `relay_dm_request.host_user_id`: the id the conversation's host gives the request's
/// user, learnt from the host's events on that user's own stream, so the relay can tell when
/// the host removes them from the conversation. `NULL` until such an event arrives.
///
/// Also bounds the rows a previous release kept forever: an accepted row expiring more than
/// 400 days out now expires 400 days from the migration, and a row accepted as it was filed
/// drops the sender's card, which nothing reads. A previous release keeps working on both.
#[derive(DeriveMigrationName)]
pub struct Migration;

const UP_SQL: &[&str] = &[
    "ALTER TABLE relay_dm_request ADD COLUMN host_user_id uuid NULL",
    "UPDATE relay_dm_request SET expires_at = now() + interval '400 days' \
     WHERE status = 'accepted' AND expires_at > now() + interval '400 days'",
    "UPDATE relay_dm_request SET card_payload = '', card_signature = '' \
     WHERE status = 'accepted' AND decided_at = created_at",
];

const DOWN_SQL: &[&str] = &["ALTER TABLE relay_dm_request DROP COLUMN host_user_id"];

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
