use async_trait::async_trait;
use entity::on_call_override;
use rim_core::oncall::model::{
    NewOnCallOverride, OnCallOverride, OnCallOverrideId, OnCallScheduleId,
};
use rim_core::oncall::repository::OnCallOverrideRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmOnCallOverrideRepository {
    pub db: DbHandle,
}

impl SeaOrmOnCallOverrideRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl OnCallOverrideRepository for SeaOrmOnCallOverrideRepository {
    async fn create(&self, new: NewOnCallOverride) -> RepoResult<OnCallOverride> {
        let now = chrono::Utc::now();
        let active = on_call_override::ActiveModel {
            id: Set(OnCallOverrideId::new().into()),
            schedule_id: Set(new.schedule_id.into()),
            user_id: Set(new.user_id.into()),
            start_at: Set(new.start_at.into()),
            end_at: Set(new.end_at.into()),
            created_by: Set(new.created_by.into()),
            created_at: Set(now.into()),
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: OnCallOverrideId) -> RepoResult<Option<OnCallOverride>> {
        let model = on_call_override::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_by_schedule(
        &self,
        schedule_id: OnCallScheduleId,
    ) -> RepoResult<Vec<OnCallOverride>> {
        let models = on_call_override::Entity::find()
            .filter(on_call_override::Column::ScheduleId.eq::<Uuid>(schedule_id.into()))
            .order_by_asc(on_call_override::Column::StartAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn delete(&self, id: OnCallOverrideId) -> RepoResult<()> {
        let res = on_call_override::Entity::delete_by_id::<Uuid>(id.into())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }
}
