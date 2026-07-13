use async_trait::async_trait;
use entity::on_call_schedule;
use rim_core::oncall::model::{NewOnCallSchedule, OnCallSchedule, OnCallScheduleId};
use rim_core::oncall::repository::OnCallScheduleRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmOnCallScheduleRepository {
    pub db: DbHandle,
}

impl SeaOrmOnCallScheduleRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OnCallScheduleRepository for SeaOrmOnCallScheduleRepository {
    async fn create(&self, new: NewOnCallSchedule) -> RepoResult<OnCallSchedule> {
        let now = chrono::Utc::now();
        let active = on_call_schedule::ActiveModel {
            id: Set(OnCallScheduleId::new().into()),
            server_id: Set(new.server_id.into()),
            name: Set(new.name),
            timezone: Set(new.timezone),
            created_by: Set(new.created_by.into()),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: OnCallScheduleId) -> RepoResult<Option<OnCallSchedule>> {
        let model = on_call_schedule::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<OnCallSchedule>> {
        let models = on_call_schedule::Entity::find()
            .filter(on_call_schedule::Column::ServerId.eq::<Uuid>(server_id.into()))
            .order_by_asc(on_call_schedule::Column::CreatedAt)
            .order_by_asc(on_call_schedule::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn update(
        &self,
        id: OnCallScheduleId,
        name: Option<String>,
        timezone: Option<String>,
    ) -> RepoResult<OnCallSchedule> {
        let model = on_call_schedule::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: on_call_schedule::ActiveModel = model.into();
        if let Some(name) = name {
            active.name = Set(name);
        }
        if let Some(timezone) = timezone {
            active.timezone = Set(timezone);
        }
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: OnCallScheduleId) -> RepoResult<()> {
        let res = on_call_schedule::Entity::delete_by_id::<Uuid>(id.into())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }
}
