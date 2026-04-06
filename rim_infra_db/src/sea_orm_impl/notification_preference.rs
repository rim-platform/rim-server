use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::notification_preference;
use rim_core::conversation::model::ConversationId;
use rim_core::notification_preference::model::NotificationPreference;
use rim_core::notification_preference::repository::NotificationPreferenceRepository;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmNotificationPreferenceRepository {
    pub db: DbHandle,
}

impl SeaOrmNotificationPreferenceRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
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
        let muted_until_tz = muted_until.map(|dt| dt.fixed_offset());

        // Try to find existing
        let existing = notification_preference::Entity::find_by_id((
            Uuid::from(user_id),
            Uuid::from(conversation_id),
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;

        if let Some(_existing) = existing {
            // Update
            let active = notification_preference::ActiveModel {
                user_id: Set(user_id.into()),
                conversation_id: Set(conversation_id.into()),
                muted_until: Set(muted_until_tz),
            };
            active.save(&self.db).await.map_err(map_db_err)?;
            let model = notification_preference::Entity::find_by_id((
                Uuid::from(user_id),
                Uuid::from(conversation_id),
            ))
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .expect("just saved");
            Ok(model.into())
        } else {
            // Insert
            let active = notification_preference::ActiveModel {
                user_id: Set(user_id.into()),
                conversation_id: Set(conversation_id.into()),
                muted_until: Set(muted_until_tz),
            };
            let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
            Ok(inserted.into())
        }
    }

    async fn remove_mute(
        &self,
        user_id: UserId,
        conversation_id: ConversationId,
    ) -> RepoResult<()> {
        notification_preference::Entity::delete_by_id((
            Uuid::from(user_id),
            Uuid::from(conversation_id),
        ))
        .exec(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(())
    }

    async fn is_muted(&self, user_id: UserId, conversation_id: ConversationId) -> RepoResult<bool> {
        let pref = notification_preference::Entity::find_by_id((
            Uuid::from(user_id),
            Uuid::from(conversation_id),
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;

        match pref {
            None => Ok(false),
            Some(p) => {
                // If muted_until is None, it's a permanent mute.
                // If muted_until is Some, check if it's still in the future.
                match p.muted_until {
                    None => Ok(true),
                    Some(until) => Ok(chrono::Utc::now() < DateTime::<Utc>::from(until)),
                }
            }
        }
    }

    async fn get_muted_conversations(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<NotificationPreference>> {
        let models = notification_preference::Entity::find()
            .filter(notification_preference::Column::UserId.eq(Uuid::from(user_id)))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }
}
