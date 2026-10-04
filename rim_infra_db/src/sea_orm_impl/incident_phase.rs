use async_trait::async_trait;
use entity::incident_phase;
use rim_core::incident::model::{
    IncidentId, IncidentPhaseId, IncidentPhaseName, IncidentPhaseRecord,
};
use rim_core::incident::repository::IncidentPhaseRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmIncidentPhaseRepository {
    pub db: DbHandle,
}

impl SeaOrmIncidentPhaseRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IncidentPhaseRepository for SeaOrmIncidentPhaseRepository {
    async fn create(
        &self,
        incident_id: IncidentId,
        phase: IncidentPhaseName,
        note: Option<String>,
    ) -> RepoResult<IncidentPhaseRecord> {
        let now = chrono::Utc::now();
        let id = IncidentPhaseId::new();

        let active = incident_phase::ActiveModel {
            id: Set(id.into()),
            incident_id: Set(incident_id.into()),
            phase: Set(phase.as_str().to_string()),
            started_at: Set(now.into()),
            ended_at: Set(None),
            note: Set(note),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn end_phase(&self, phase_id: IncidentPhaseId) -> RepoResult<IncidentPhaseRecord> {
        let model = incident_phase::Entity::find_by_id::<Uuid>(phase_id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| {
                rim_core::repository::error::RepoError::new(
                    rim_core::repository::error::RepoErrorKind::NotFound,
                )
            })?;

        let mut active: incident_phase::ActiveModel = model.into();
        active.ended_at = Set(Some(chrono::Utc::now().into()));

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn find_current_phase(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Option<IncidentPhaseRecord>> {
        let model = incident_phase::Entity::find()
            .filter(incident_phase::Column::IncidentId.eq::<Uuid>(incident_id.into()))
            .filter(incident_phase::Column::EndedAt.is_null())
            .order_by_desc(incident_phase::Column::StartedAt)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn list_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Vec<IncidentPhaseRecord>> {
        let models = incident_phase::Entity::find()
            .filter(incident_phase::Column::IncidentId.eq::<Uuid>(incident_id.into()))
            .order_by_asc(incident_phase::Column::StartedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models.into_iter().map(Into::into).collect())
    }
}
