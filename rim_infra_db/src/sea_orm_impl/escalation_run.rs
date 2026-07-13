use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::escalation_run;
use rim_core::escalation::model::{
    EscalationRun, EscalationRunId, EscalationRunStatus, NewEscalationRun,
};
use rim_core::escalation::repository::EscalationRunRepository;
use rim_core::incident::model::IncidentId;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmEscalationRunRepository {
    pub db: DbHandle,
}

impl SeaOrmEscalationRunRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl EscalationRunRepository for SeaOrmEscalationRunRepository {
    async fn create(&self, new: NewEscalationRun) -> RepoResult<EscalationRun> {
        let now = chrono::Utc::now();
        let active = escalation_run::ActiveModel {
            id: Set(EscalationRunId::new().into()),
            incident_id: Set(new.incident_id.into()),
            policy_id: Set(new.policy_id.into()),
            current_step_index: Set(0),
            next_fire_at: Set(new.next_fire_at.map(Into::into)),
            status: Set(EscalationRunStatus::Active.as_str().to_string()),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: EscalationRunId) -> RepoResult<Option<EscalationRun>> {
        let model = escalation_run::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn find_active_by_incident(
        &self,
        incident_id: IncidentId,
    ) -> RepoResult<Option<EscalationRun>> {
        let model = escalation_run::Entity::find()
            .filter(escalation_run::Column::IncidentId.eq::<Uuid>(incident_id.into()))
            .filter(escalation_run::Column::Status.eq(EscalationRunStatus::Active.as_str()))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_active(&self) -> RepoResult<Vec<EscalationRun>> {
        let models = escalation_run::Entity::find()
            .filter(escalation_run::Column::Status.eq(EscalationRunStatus::Active.as_str()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn advance(
        &self,
        id: EscalationRunId,
        current_step_index: i32,
        next_fire_at: Option<DateTime<Utc>>,
    ) -> RepoResult<EscalationRun> {
        let model = escalation_run::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: escalation_run::ActiveModel = model.into();
        active.current_step_index = Set(current_step_index);
        active.next_fire_at = Set(next_fire_at.map(Into::into));
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn set_status(
        &self,
        id: EscalationRunId,
        status: EscalationRunStatus,
    ) -> RepoResult<EscalationRun> {
        let model = escalation_run::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: escalation_run::ActiveModel = model.into();
        active.status = Set(status.as_str().to_string());
        if status != EscalationRunStatus::Active {
            active.next_fire_at = Set(None);
        }
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }
}
