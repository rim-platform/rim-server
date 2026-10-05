use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::notification_preference;
use entity::sea_orm_active_enums::NotificationLevel as DbNotificationLevel;
use rim_core::conversation::model::ConversationId;
use rim_core::notification::model::NotificationLevel;
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbBackend, EntityTrait, QueryFilter, Statement,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmNotificationPreferenceRepository {
    pub db: DbHandle,
}

impl SeaOrmNotificationPreferenceRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    async fn find(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<NotificationPreference>> {
        let model = notification_preference::Entity::find_by_id((
            Uuid::from(user_id),
            Uuid::from(conversation_id),
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    /// Delete the row once it carries neither a mute nor a level override.
    async fn delete_if_empty(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<()> {
        notification_preference::Entity::delete_many()
            .filter(notification_preference::Column::UserId.eq(Uuid::from(user_id)))
            .filter(notification_preference::Column::ConversationId.eq(Uuid::from(conversation_id)))
            .filter(notification_preference::Column::MutedUntil.is_null())
            .filter(notification_preference::Column::Level.is_null())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(())
    }
}

#[async_trait]
impl NotificationPreferenceRepository for SeaOrmNotificationPreferenceRepository {
    async fn set_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        muted_until: Option<DateTime<Utc>>,
    ) -> RepoResult<NotificationPreference> {
        let user_uuid: Uuid = user_id.into();
        let conv_uuid: Uuid = conversation_id.into();
        let stmt = match muted_until {
            // Timed mute: a `none` override would outlive it, so it is cleared.
            Some(until) => {
                let until: DateTimeWithTimeZone = until.into();
                Statement::from_sql_and_values(
                    DbBackend::Postgres,
                    r#"INSERT INTO notification_preference (user_id, conversation_id, muted_until)
                       VALUES ($1, $2, $3)
                       ON CONFLICT (user_id, conversation_id) DO UPDATE
                       SET muted_until = EXCLUDED.muted_until,
                           level = CASE WHEN notification_preference.level = 'none'
                                        THEN NULL ELSE notification_preference.level END"#,
                    [user_uuid.into(), conv_uuid.into(), until.into()],
                )
            }
            // Permanent mute: an explicit `none` level without an expiry.
            None => Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"INSERT INTO notification_preference (user_id, conversation_id, muted_until, level)
                   VALUES ($1, $2, NULL, 'none')
                   ON CONFLICT (user_id, conversation_id) DO UPDATE
                   SET muted_until = NULL, level = 'none'"#,
                [user_uuid.into(), conv_uuid.into()],
            ),
        };
        self.db.execute_raw(stmt).await.map_err(map_db_err)?;
        self.find(user_id, conversation_id).await?.ok_or_else(|| {
            rim_core::repository::error::RepoError::new(
                rim_core::repository::error::RepoErrorKind::NotFound,
            )
        })
    }

    async fn remove_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<()> {
        let user_uuid: Uuid = user_id.into();
        let conv_uuid: Uuid = conversation_id.into();
        self.db
            .execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                r#"UPDATE notification_preference
                   SET muted_until = NULL,
                       level = CASE WHEN level = 'none' THEN NULL ELSE level END
                   WHERE user_id = $1 AND conversation_id = $2"#,
                [user_uuid.into(), conv_uuid.into()],
            ))
            .await
            .map_err(map_db_err)?;
        self.delete_if_empty(user_id, conversation_id).await
    }

    async fn is_muted(&self, user_id: UserId, conversation_id: ConversationId) -> RepoResult<bool> {
        Ok(self
            .find(user_id, conversation_id)
            .await?
            .is_some_and(|pref| pref.is_muted_at(Utc::now())))
    }

    async fn get_muted_conversations(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<NotificationPreference>> {
        let models = notification_preference::Entity::find()
            .filter(notification_preference::Column::UserId.eq(Uuid::from(user_id)))
            .filter(
                Condition::any()
                    .add(notification_preference::Column::MutedUntil.is_not_null())
                    .add(notification_preference::Column::Level.eq(DbNotificationLevel::None)),
            )
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn set_level(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
        level: Option<NotificationLevel>,
    ) -> RepoResult<Option<NotificationPreference>> {
        let user_uuid: Uuid = user_id.into();
        let conv_uuid: Uuid = conversation_id.into();
        match level {
            Some(level) => {
                self.db
                    .execute_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        r#"INSERT INTO notification_preference (user_id, conversation_id, level)
                           VALUES ($1, $2, $3::notification_level)
                           ON CONFLICT (user_id, conversation_id) DO UPDATE
                           SET level = EXCLUDED.level"#,
                        [user_uuid.into(), conv_uuid.into(), level.as_str().into()],
                    ))
                    .await
                    .map_err(map_db_err)?;
            }
            None => {
                self.db
                    .execute_raw(Statement::from_sql_and_values(
                        DbBackend::Postgres,
                        r#"UPDATE notification_preference SET level = NULL
                           WHERE user_id = $1 AND conversation_id = $2"#,
                        [user_uuid.into(), conv_uuid.into()],
                    ))
                    .await
                    .map_err(map_db_err)?;
                self.delete_if_empty(user_id, conversation_id).await?;
            }
        }
        self.find(user_id, conversation_id).await
    }

    async fn get_preference(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<NotificationPreference>> {
        self.find(user_id, conversation_id).await
    }

    async fn list_for_conversation(
        &self,
        conversation_id: ConversationId,
        user_ids: &[UserId],
    ) -> RepoResult<Vec<NotificationPreference>> {
        if user_ids.is_empty() {
            return Ok(vec![]);
        }
        let user_uuids: Vec<Uuid> = user_ids.iter().copied().map(Into::into).collect();
        let models = notification_preference::Entity::find()
            .filter(notification_preference::Column::ConversationId.eq(Uuid::from(conversation_id)))
            .filter(notification_preference::Column::UserId.is_in(user_uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }
}
