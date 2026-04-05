use async_trait::async_trait;
use entity::message_pin;
use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_pin::model::MessagePin;
use rim_core::message_pin::repository::MessagePinRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmMessagePinRepository {
    pub db: DbHandle,
}

impl SeaOrmMessagePinRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MessagePinRepository for SeaOrmMessagePinRepository {
    async fn pin(
        &self,
        message_id: MessageId,
        conversation_id: ConversationId,
        pinned_by: UserId,
    ) -> RepoResult<MessagePin> {
        let active = message_pin::ActiveModel {
            message_id: Set(message_id.into()),
            conversation_id: Set(conversation_id.into()),
            pinned_by: Set(pinned_by.into()),
            ..Default::default()
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn unpin(&self, message_id: MessageId) -> RepoResult<()> {
        let result = message_pin::Entity::delete_by_id(message_id)
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;

        if result.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }

        Ok(())
    }

    async fn list_pinned(&self, conversation_id: ConversationId) -> RepoResult<Vec<MessagePin>> {
        let models = message_pin::Entity::find()
            .filter(message_pin::Column::ConversationId.eq(Uuid::from(conversation_id)))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn is_pinned(&self, message_id: MessageId) -> RepoResult<bool> {
        let count = message_pin::Entity::find_by_id(message_id)
            .count(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(count > 0)
    }

    async fn is_pinned_batch(&self, message_ids: &[MessageId]) -> RepoResult<Vec<MessageId>> {
        if message_ids.is_empty() {
            return Ok(Vec::new());
        }

        let uuids: Vec<Uuid> = message_ids.iter().copied().map(Into::into).collect();
        let models = message_pin::Entity::find()
            .filter(message_pin::Column::MessageId.is_in(uuids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(|m| m.message_id.into()).collect())
    }

    async fn count_pins(&self, conversation_id: ConversationId) -> RepoResult<u64> {
        let count = message_pin::Entity::find()
            .filter(message_pin::Column::ConversationId.eq(Uuid::from(conversation_id)))
            .count(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(count)
    }
}
