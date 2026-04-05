use async_trait::async_trait;
use entity::custom_emoji;
use rim_core::custom_emoji::model::{CustomEmoji, CustomEmojiId, NewCustomEmoji};
use rim_core::custom_emoji::repository::CustomEmojiRepository;
use rim_core::repository::error::RepoResult;
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, ModelTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmCustomEmojiRepository {
    pub db: DbHandle,
}

impl SeaOrmCustomEmojiRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CustomEmojiRepository for SeaOrmCustomEmojiRepository {
    async fn create(&self, emoji: NewCustomEmoji) -> RepoResult<CustomEmoji> {
        let active = custom_emoji::ActiveModel {
            id: Set(CustomEmojiId::new().into()),
            server_id: Set(emoji.server_id.into()),
            name: Set(emoji.name),
            image_url: Set(emoji.image_url),
            uploaded_by: Set(emoji.uploaded_by.into()),
            created_at: Set(chrono::Utc::now().into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn delete(&self, id: CustomEmojiId) -> RepoResult<()> {
        let model = custom_emoji::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if let Some(model) = model {
            model.delete(&self.db).await.map_err(map_db_err)?;
        }

        Ok(())
    }

    async fn find_by_id(&self, id: CustomEmojiId) -> RepoResult<Option<CustomEmoji>> {
        let model = custom_emoji::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<CustomEmoji>> {
        let emojis = custom_emoji::Entity::find()
            .filter(custom_emoji::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(emojis.into_iter().map(Into::into).collect())
    }
}
