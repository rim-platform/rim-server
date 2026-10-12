use async_trait::async_trait;
use entity::pipeline;
use rim_core::pipeline::model::{Pipeline, PipelineId};
use rim_core::pipeline::repository::PipelineRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmPipelineRepository {
    db: DbHandle,
}

impl SeaOrmPipelineRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }

    fn model_to_pipeline(m: pipeline::Model) -> RepoResult<Pipeline> {
        m.try_into().map_err(|e: serde_json::Error| {
            RepoError::with_message(RepoErrorKind::InvalidData, e.to_string())
        })
    }
}

#[async_trait]
impl PipelineRepository for SeaOrmPipelineRepository {
    async fn create(&self, pipeline: &Pipeline) -> RepoResult<Pipeline> {
        let trigger_data = serde_json::to_value(&pipeline.trigger)
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;
        let filter_data = pipeline
            .filter
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;
        let actions_data = serde_json::to_value(&pipeline.actions)
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;

        let model = pipeline::ActiveModel {
            id: Set(pipeline.id.into()),
            server_id: Set(pipeline.server_id.into()),
            name: Set(pipeline.name.clone()),
            description: Set(pipeline.description.clone()),
            trigger_data: Set(trigger_data),
            filter_data: Set(filter_data),
            actions_data: Set(actions_data),
            enabled: Set(pipeline.enabled),
            cooldown_secs: Set(pipeline.cooldown_secs),
            created_by: Set(pipeline.created_by.into()),
            schema_version: Set(pipeline.schema_version),
            last_fired_at: Set(pipeline.last_fired_at.map(|dt| dt.into())),
            created_at: Set(pipeline.created_at.into()),
            updated_at: Set(pipeline.updated_at.into()),
        };

        let stored = model.insert(&self.db).await.map_err(map_db_err)?;
        Self::model_to_pipeline(stored)
    }

    async fn get_by_id(&self, id: PipelineId) -> RepoResult<Option<Pipeline>> {
        let model = pipeline::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        model.map(Self::model_to_pipeline).transpose()
    }

    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>> {
        let models = pipeline::Entity::find()
            .filter(pipeline::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        models.into_iter().map(Self::model_to_pipeline).collect()
    }

    async fn list_enabled_for_server(&self, server_id: ServerId) -> RepoResult<Vec<Pipeline>> {
        let models = pipeline::Entity::find()
            .filter(pipeline::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(pipeline::Column::Enabled.eq(true))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        models.into_iter().map(Self::model_to_pipeline).collect()
    }

    async fn update(&self, pipeline: &Pipeline) -> RepoResult<Pipeline> {
        let model = pipeline::Entity::find_by_id(pipeline.id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let trigger_data = serde_json::to_value(&pipeline.trigger)
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;
        let filter_data = pipeline
            .filter
            .as_ref()
            .map(serde_json::to_value)
            .transpose()
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;
        let actions_data = serde_json::to_value(&pipeline.actions)
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))?;

        let mut active: pipeline::ActiveModel = model.into();
        active.name = Set(pipeline.name.clone());
        active.description = Set(pipeline.description.clone());
        active.trigger_data = Set(trigger_data);
        active.filter_data = Set(filter_data);
        active.actions_data = Set(actions_data);
        active.enabled = Set(pipeline.enabled);
        active.cooldown_secs = Set(pipeline.cooldown_secs);
        active.schema_version = Set(pipeline.schema_version);
        active.last_fired_at = Set(pipeline.last_fired_at.map(|dt| dt.into()));
        active.updated_at = Set(chrono::Utc::now().into());

        let stored = active.update(&self.db).await.map_err(map_db_err)?;
        Self::model_to_pipeline(stored)
    }

    async fn delete(&self, id: PipelineId) -> RepoResult<()> {
        let model = pipeline::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let active: pipeline::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64> {
        let count = pipeline::Entity::find()
            .filter(pipeline::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(pipeline::Column::CreatedBy.eq::<Uuid>(user_id.into()))
            .count(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(count)
    }

    async fn set_enabled(&self, id: PipelineId, enabled: bool) -> RepoResult<Pipeline> {
        let model = pipeline::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let mut active: pipeline::ActiveModel = model.into();
        active.enabled = Set(enabled);
        active.updated_at = Set(chrono::Utc::now().into());
        let stored = active.update(&self.db).await.map_err(map_db_err)?;
        Self::model_to_pipeline(stored)
    }

    async fn set_last_fired_at(
        &self,
        id: PipelineId,
        fired_at: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<()> {
        let model = pipeline::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let mut active: pipeline::ActiveModel = model.into();
        active.last_fired_at = Set(Some(fired_at.into()));
        active.updated_at = Set(chrono::Utc::now().into());
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn count_for_server(&self, server_id: ServerId) -> RepoResult<u64> {
        pipeline::Entity::find()
            .filter(pipeline::Column::ServerId.eq::<Uuid>(server_id.into()))
            .count(&self.db)
            .await
            .map_err(map_db_err)
    }
}
