use sea_orm_migration::prelude::*;

/// `message_franking_revision`: the franking commitment, send time and sender device of every
/// version of an encrypted message (revision 0 when sent, one more per edit), so a report of a
/// version the sender later edited still verifies. Existing encrypted messages get revision 0
/// from their row.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE message_franking_revision (
                message_id uuid NOT NULL REFERENCES message (id) ON DELETE CASCADE,
                revision integer NOT NULL CHECK (revision >= 0),
                franking_commitment bytea NOT NULL,
                sent_at_us bigint NOT NULL,
                sender_device_id uuid NOT NULL,
                created_at timestamptz NOT NULL DEFAULT now(),
                PRIMARY KEY (message_id, revision)
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "INSERT INTO message_franking_revision \
             (message_id, revision, franking_commitment, sent_at_us, sender_device_id) \
             SELECT m.id, 0, m.franking_commitment, \
                    (extract(epoch FROM m.sent_at) * 1000000)::bigint, d.device_id \
             FROM message m JOIN device_key d ON d.id = m.sender_device_key_id \
             WHERE m.encrypted AND m.franking_commitment IS NOT NULL",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE message_franking_revision")
            .await?;
        Ok(())
    }
}
