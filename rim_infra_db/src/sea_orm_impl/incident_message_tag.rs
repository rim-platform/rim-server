use async_trait::async_trait;
use entity::incident_message_tag;
use rim_core::incident::model::{IncidentId, IncidentMessageTag, NewIncidentMessageTag};
use rim_core::incident::repository::IncidentMessageTagRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmIncidentMessageTagRepository {
    pub db: DbHandle,
}

impl SeaOrmIncidentMessageTagRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IncidentMessageTagRepository for SeaOrmIncidentMessageTagRepository {
    async fn create(&self, tag: NewIncidentMessageTag) -> RepoResult<IncidentMessageTag> {
        let active = incident_message_tag::ActiveModel {
            message_id: Set(tag.message_id.into()),
            incident_id: Set(tag.incident_id.into()),
            phase: Set(tag.phase.as_str().to_string()),
            tag_type: Set(tag.tag_type.map(|t| t.as_str().to_string())),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn list_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Vec<IncidentMessageTag>> {
        let models = incident_message_tag::Entity::find()
            .filter(incident_message_tag::Column::IncidentId.eq::<Uuid>(incident_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }
}
