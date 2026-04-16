use async_trait::async_trait;
use entity::incident;
use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::{Incident, IncidentId, IncidentStatus, NewIncident};
use rim_core::incident::repository::IncidentRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmIncidentRepository {
    pub db: DbHandle,
}

impl SeaOrmIncidentRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IncidentRepository for SeaOrmIncidentRepository {
    async fn create(&self, new: NewIncident) -> RepoResult<Incident> {
        let now = chrono::Utc::now();
        let id = IncidentId::new();

        let active = incident::ActiveModel {
            id: Set(id.into()),
            server_id: Set(new.server_id.into()),
            conversation_id: Set(new.conversation_id.into()),
            started_by: Set(new.started_by.into()),
            status: Set("active".to_string()),
            severity: Set(new.severity.map(|s| s.as_str().to_string())),
            title: Set(new.title),
            original_topic: Set(new.original_topic),
            started_at: Set(now.into()),
            mitigated_at: Set(None),
            resolved_at: Set(None),
            postmortem_draft: Set(None),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: IncidentId) -> RepoResult<Option<Incident>> {
        let model = incident::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn find_active_by_conversation(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<Incident>> {
        let model = incident::Entity::find()
            .filter(incident::Column::ConversationId.eq::<Uuid>(conversation_id.into()))
            .filter(incident::Column::Status.eq("active"))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(model.map(Into::into))
    }

    async fn update_status(
        &self,
        id: IncidentId,
        status: IncidentStatus,
        mitigated_at: Option<chrono::DateTime<chrono::Utc>>,
        resolved_at: Option<chrono::DateTime<chrono::Utc>>,
    ) -> RepoResult<Incident> {
        let model = incident::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| {
                rim_core::repository::error::RepoError::new(
                    rim_core::repository::error::RepoErrorKind::NotFound,
                )
            })?;

        let mut active: incident::ActiveModel = model.into();
        active.status = Set(status.as_str().to_string());
        active.mitigated_at = Set(mitigated_at.map(Into::into));
        active.resolved_at = Set(resolved_at.map(Into::into));
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn update_postmortem_draft(&self, id: IncidentId, draft: String) -> RepoResult<Incident> {
        let model = incident::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| {
                rim_core::repository::error::RepoError::new(
                    rim_core::repository::error::RepoErrorKind::NotFound,
                )
            })?;

        let mut active: incident::ActiveModel = model.into();
        active.postmortem_draft = Set(Some(draft));
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }
}
