use sea_orm_migration::prelude::*;

#[derive(DeriveMigrationName)]
pub struct Migration;

#[async_trait::async_trait]
impl MigrationTrait for Migration {
    async fn up(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // message_pin.pinned_by → SET NULL (preserve pin, clear who pinned it)
        db.execute_unprepared("ALTER TABLE message_pin ALTER COLUMN pinned_by DROP NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE message_pin DROP CONSTRAINT IF EXISTS fk_message_pin_user",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE message_pin ADD CONSTRAINT fk_message_pin_user \
             FOREIGN KEY (pinned_by) REFERENCES \"user\"(id) ON DELETE SET NULL",
        )
        .await?;

        // message_mention.mentioned_user_id → CASCADE
        db.execute_unprepared(
            "ALTER TABLE message_mention DROP CONSTRAINT IF EXISTS fk_message_mention_mentioned_user",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE message_mention ADD CONSTRAINT fk_message_mention_mentioned_user \
             FOREIGN KEY (mentioned_user_id) REFERENCES \"user\"(id) ON DELETE CASCADE",
        )
        .await?;

        // custom_emoji.uploaded_by → SET NULL (preserve emoji, clear uploader)
        db.execute_unprepared("ALTER TABLE custom_emoji ALTER COLUMN uploaded_by DROP NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE custom_emoji DROP CONSTRAINT IF EXISTS fk_custom_emoji_uploaded_by",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE custom_emoji ADD CONSTRAINT fk_custom_emoji_uploaded_by \
             FOREIGN KEY (uploaded_by) REFERENCES \"user\"(id) ON DELETE SET NULL",
        )
        .await?;

        // incident.started_by → SET NULL (preserve incident record, clear starter)
        db.execute_unprepared("ALTER TABLE incident ALTER COLUMN started_by DROP NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE incident DROP CONSTRAINT IF EXISTS fk_incident_started_by",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE incident ADD CONSTRAINT fk_incident_started_by \
             FOREIGN KEY (started_by) REFERENCES \"user\"(id) ON DELETE SET NULL",
        )
        .await?;

        // incident_message_tag.message_id → CASCADE (tags follow message lifecycle)
        db.execute_unprepared(
            "ALTER TABLE incident_message_tag DROP CONSTRAINT IF EXISTS fk_incident_msg_tag_message",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE incident_message_tag ADD CONSTRAINT fk_incident_msg_tag_message \
             FOREIGN KEY (message_id) REFERENCES message(id) ON DELETE CASCADE",
        )
        .await?;

        Ok(())
    }

    async fn down(&self, manager: &SchemaManager) -> Result<(), DbErr> {
        let db = manager.get_connection();

        // Revert to original NO ACTION constraints

        db.execute_unprepared("ALTER TABLE message_pin ALTER COLUMN pinned_by SET NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE message_pin DROP CONSTRAINT IF EXISTS fk_message_pin_user",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE message_pin ADD CONSTRAINT fk_message_pin_user \
             FOREIGN KEY (pinned_by) REFERENCES \"user\"(id)",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE message_mention DROP CONSTRAINT IF EXISTS fk_message_mention_mentioned_user",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE message_mention ADD CONSTRAINT fk_message_mention_mentioned_user \
             FOREIGN KEY (mentioned_user_id) REFERENCES \"user\"(id)",
        )
        .await?;

        db.execute_unprepared("ALTER TABLE custom_emoji ALTER COLUMN uploaded_by SET NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE custom_emoji DROP CONSTRAINT IF EXISTS fk_custom_emoji_uploaded_by",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE custom_emoji ADD CONSTRAINT fk_custom_emoji_uploaded_by \
             FOREIGN KEY (uploaded_by) REFERENCES \"user\"(id)",
        )
        .await?;

        db.execute_unprepared("ALTER TABLE incident ALTER COLUMN started_by SET NOT NULL")
            .await?;
        db.execute_unprepared(
            "ALTER TABLE incident DROP CONSTRAINT IF EXISTS fk_incident_started_by",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE incident ADD CONSTRAINT fk_incident_started_by \
             FOREIGN KEY (started_by) REFERENCES \"user\"(id)",
        )
        .await?;

        db.execute_unprepared(
            "ALTER TABLE incident_message_tag DROP CONSTRAINT IF EXISTS fk_incident_msg_tag_message",
        )
        .await?;
        db.execute_unprepared(
            "ALTER TABLE incident_message_tag ADD CONSTRAINT fk_incident_msg_tag_message \
             FOREIGN KEY (message_id) REFERENCES message(id)",
        )
        .await?;

        Ok(())
    }
}
