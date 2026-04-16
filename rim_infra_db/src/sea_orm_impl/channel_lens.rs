use async_trait::async_trait;
use entity::channel_lens;
use rim_core::channel_lens::model::{ChannelLens, ChannelLensId, NewChannelLens};
use rim_core::channel_lens::repository::ChannelLensRepository;
use rim_core::conversation::model::ConversationId;
use rim_core::repository::error::RepoResult;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, ModelTrait, PaginatorTrait, QueryFilter,
    Set,
};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmChannelLensRepository {
    pub db: DbHandle,
}

impl SeaOrmChannelLensRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl ChannelLensRepository for SeaOrmChannelLensRepository {
    async fn create(&self, lens: NewChannelLens) -> RepoResult<ChannelLens> {
        let now = chrono::Utc::now();
        let filter_spec_json = serde_json::to_value(&lens.filter_spec).map_err(|e| {
            rim_core::repository::error::RepoError::with_source(
                rim_core::repository::error::RepoErrorKind::InvalidData,
                e,
            )
        })?;

        let active = channel_lens::ActiveModel {
            id: Set(ChannelLensId::new().into()),
            server_id: Set(lens.server_id.into()),
            conversation_id: Set(lens.conversation_id.into()),
            created_by: Set(lens.created_by.into()),
            name: Set(lens.name),
            filter_spec: Set(filter_spec_json),
            is_shared: Set(lens.is_shared),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn get_by_id(&self, id: ChannelLensId) -> RepoResult<Option<ChannelLens>> {
        let model = channel_lens::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update(&self, lens: &ChannelLens) -> RepoResult<ChannelLens> {
        let filter_spec_json = serde_json::to_value(&lens.filter_spec).map_err(|e| {
            rim_core::repository::error::RepoError::with_source(
                rim_core::repository::error::RepoErrorKind::InvalidData,
                e,
            )
        })?;

        let active = channel_lens::ActiveModel {
            id: Set(lens.id.into()),
            server_id: Set(lens.server_id.into()),
            conversation_id: Set(lens.conversation_id.into()),
            created_by: Set(lens.created_by.into()),
            name: Set(lens.name.clone()),
            filter_spec: Set(filter_spec_json),
            is_shared: Set(lens.is_shared),
            created_at: Set(lens.created_at.into()),
            updated_at: Set(lens.updated_at.into()),
        };

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: ChannelLensId) -> RepoResult<()> {
        let model = channel_lens::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(model) = model {
            model.delete(&self.db).await.map_err(map_db_err)?;
        }

        Ok(())
    }

    async fn list_for_conversation(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<Vec<ChannelLens>> {
        let conv_uuid: Uuid = conversation_id.into();
        let user_uuid: Uuid = user_id.into();

        let models = channel_lens::Entity::find()
            .filter(channel_lens::Column::ConversationId.eq(conv_uuid))
            .filter(
                Condition::any()
                    .add(channel_lens::Column::CreatedBy.eq(user_uuid))
                    .add(channel_lens::Column::IsShared.eq(true)),
            )
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn count_user_lenses(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> RepoResult<u64> {
        let conv_uuid: Uuid = conversation_id.into();
        let user_uuid: Uuid = user_id.into();

        let count = channel_lens::Entity::find()
            .filter(channel_lens::Column::ConversationId.eq(conv_uuid))
            .filter(channel_lens::Column::CreatedBy.eq(user_uuid))
            .count(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(count)
    }
}
