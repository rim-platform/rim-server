use async_trait::async_trait;
use entity::incident;
use rim_core::conversation::model::ConversationId;
use rim_core::incident::model::{
    Incident, IncidentId, IncidentSeverity, IncidentStatus, NewIncident,
};
use rim_core::incident::repository::{IncidentListCursor, IncidentRepository};
use rim_core::repository::error::RepoResult;
use rim_core::repository::pagination::{Page, PageDir};
use rim_core::server::model::ServerId;
use sea_orm::prelude::{DateTimeWithTimeZone, Uuid};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, Condition, EntityTrait, QueryFilter, QueryOrder, QuerySelect,
    Set,
};

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

    async fn list_incidents(
        &self,
        server_id: ServerId,
        status_filter: Option<&[IncidentStatus]>,
        severity_filter: Option<&[IncidentSeverity]>,
        limit: u32,
        dir: PageDir,
        cursor: Option<IncidentListCursor>,
    ) -> RepoResult<Page<Incident, IncidentListCursor>> {
        let mut q = incident::Entity::find()
            .filter(incident::Column::ServerId.eq::<Uuid>(server_id.into()));

        if let Some(statuses) = status_filter {
            if !statuses.is_empty() {
                let values: Vec<String> = statuses.iter().map(|s| s.as_str().to_string()).collect();
                q = q.filter(incident::Column::Status.is_in(values));
            }
        }

        if let Some(severities) = severity_filter {
            if !severities.is_empty() {
                let values: Vec<String> =
                    severities.iter().map(|s| s.as_str().to_string()).collect();
                q = q.filter(incident::Column::Severity.is_in(values));
            }
        }

        let has_cursor = cursor.is_some();
        if let Some(cursor) = cursor {
            let cursor_id: Uuid = cursor.incident_id.into();
            let cursor_ts: DateTimeWithTimeZone = cursor.created_at.into();

            // DESC ordering: Before = greater than cursor, After = less than cursor.
            let cond = match dir {
                PageDir::Before => Condition::any()
                    .add(incident::Column::CreatedAt.gt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(incident::Column::CreatedAt.eq(cursor_ts))
                            .add(incident::Column::Id.gt(cursor_id)),
                    ),
                PageDir::After => Condition::any()
                    .add(incident::Column::CreatedAt.lt(cursor_ts))
                    .add(
                        Condition::all()
                            .add(incident::Column::CreatedAt.eq(cursor_ts))
                            .add(incident::Column::Id.lt(cursor_id)),
                    ),
            };
            q = q.filter(cond);
        }

        // Fixed ordering: most recent first.
        q = q
            .order_by_desc(incident::Column::CreatedAt)
            .order_by_desc(incident::Column::Id);

        let mut items = q
            .limit((limit + 1) as u64)
            .all(&self.db)
            .await
            .map_err(map_db_err)?
            .into_iter()
            .map(Incident::from)
            .collect::<Vec<Incident>>();

        let has_more_in_dir = items.len() > limit as usize;
        if has_more_in_dir {
            items.truncate(limit as usize);
        }

        // DESC ordering: first item is max, last is min.
        let max = items.first().map(|i| IncidentListCursor {
            created_at: i.created_at,
            incident_id: i.id,
        });
        let min = items.last().map(|i| IncidentListCursor {
            created_at: i.created_at,
            incident_id: i.id,
        });

        let has_opposite = has_cursor && !items.is_empty();
        let (next, prev) = match dir {
            PageDir::After => (
                has_more_in_dir.then_some(min).flatten(),
                has_opposite.then_some(max).flatten(),
            ),
            PageDir::Before => (
                has_more_in_dir.then_some(max).flatten(),
                has_opposite.then_some(min).flatten(),
            ),
        };

        Ok(Page::new(items, next, prev))
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
