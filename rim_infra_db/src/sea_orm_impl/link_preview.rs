use std::collections::HashMap;

use async_trait::async_trait;
use entity::link_preview;
use rim_core::link_preview::model::{LinkPreview, LinkPreviewId, NewLinkPreview};
use rim_core::link_preview::repository::LinkPreviewRepository;
use rim_core::message::model::MessageId;
use rim_core::repository::error::RepoResult;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmLinkPreviewRepository {
    pub db: DbHandle,
}

impl SeaOrmLinkPreviewRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl LinkPreviewRepository for SeaOrmLinkPreviewRepository {
    async fn create(&self, preview: NewLinkPreview) -> RepoResult<LinkPreview> {
        let active = link_preview::ActiveModel {
            id: Set(LinkPreviewId::new().into()),
            message_id: Set(preview.message_id.into()),
            url: Set(preview.url),
            title: Set(preview.title),
            description: Set(preview.description),
            image_url: Set(preview.image_url),
            site_name: Set(preview.site_name),
            fetched_at: Set(chrono::Utc::now().into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn get_for_message(&self, message_id: MessageId) -> RepoResult<Vec<LinkPreview>> {
        let previews = link_preview::Entity::find()
            .filter(link_preview::Column::MessageId.eq::<Uuid>(message_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(previews.into_iter().map(Into::into).collect())
    }

    async fn get_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<LinkPreview>>> {
        if message_ids.is_empty() {
            return Ok(HashMap::new());
        }

        let ids = message_ids
            .iter()
            .copied()
            .map(Into::into)
            .collect::<Vec<Uuid>>();

        let previews = link_preview::Entity::find()
            .filter(link_preview::Column::MessageId.is_in(ids))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        let mut out: HashMap<MessageId, Vec<LinkPreview>> = HashMap::new();
        for preview in previews.into_iter().map(Into::<LinkPreview>::into) {
            out.entry(preview.message_id).or_default().push(preview);
        }
        Ok(out)
    }
}
