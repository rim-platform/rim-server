use sea_orm_migration::prelude::*;

/// `mls_readd_nonce`: the nonce of every accepted re-add request, per device and incarnation, so
/// a request is accepted once.
#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();
        db.execute_unprepared(
            r#"CREATE TABLE mls_readd_nonce (
                conversation_id uuid NOT NULL,
                incarnation integer NOT NULL,
                device_key_id uuid NOT NULL REFERENCES device_key (id) ON DELETE CASCADE,
                nonce bytea NOT NULL,
                created_at timestamptz NOT NULL DEFAULT now(),
                PRIMARY KEY (conversation_id, incarnation, device_key_id, nonce),
                FOREIGN KEY (conversation_id, incarnation) REFERENCES mls_group ON DELETE CASCADE
            )"#,
        )
        .await?;
        db.execute_unprepared(
            "CREATE INDEX idx_mls_readd_nonce_created_at ON mls_readd_nonce (created_at)",
        )
        .await?;
        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        manager
            .get_connection()
            .execute_unprepared("DROP TABLE mls_readd_nonce")
            .await?;
        Ok(())
    }
}
