use async_trait::async_trait;
use chrono::{DateTime, NaiveTime, Utc};
use entity::on_call_rotation_layer;
use rim_core::oncall::model::{
    NewRotationLayer, OnCallScheduleId, RotationKind, RotationLayer, RotationLayerId,
    weekday_to_i16,
};
use rim_core::oncall::repository::RotationLayerRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmRotationLayerRepository {
    pub db: DbHandle,
}

impl SeaOrmRotationLayerRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

/// Decomposes a [`RotationKind`] into the DB columns that encode it: the discriminator string,
/// plus the (mutually exclusive) per-kind fields.
struct KindColumns {
    kind: &'static str,
    interval_seconds: Option<i64>,
    handoff_time: Option<NaiveTime>,
    handoff_weekday: Option<i16>,
}

fn kind_columns(kind: &RotationKind) -> KindColumns {
    match *kind {
        RotationKind::Daily { handoff_time } => KindColumns {
            kind: "daily",
            interval_seconds: None,
            handoff_time: Some(handoff_time),
            handoff_weekday: None,
        },
        RotationKind::Weekly {
            handoff_weekday,
            handoff_time,
        } => KindColumns {
            kind: "weekly",
            interval_seconds: None,
            handoff_time: Some(handoff_time),
            handoff_weekday: Some(weekday_to_i16(handoff_weekday)),
        },
        RotationKind::Custom { interval_seconds } => KindColumns {
            kind: "custom",
            interval_seconds: Some(interval_seconds),
            handoff_time: None,
            handoff_weekday: None,
        },
    }
}

#[async_trait]
impl RotationLayerRepository for SeaOrmRotationLayerRepository {
    async fn create(&self, new: NewRotationLayer) -> RepoResult<RotationLayer> {
        let now = chrono::Utc::now();
        let cols = kind_columns(&new.kind);
        let active = on_call_rotation_layer::ActiveModel {
            id: Set(RotationLayerId::new().into()),
            schedule_id: Set(new.schedule_id.into()),
            name: Set(new.name),
            priority: Set(new.priority),
            kind: Set(cols.kind.to_string()),
            interval_seconds: Set(cols.interval_seconds),
            handoff_time: Set(cols.handoff_time),
            handoff_weekday: Set(cols.handoff_weekday),
            anchor_at: Set(new.anchor_at.into()),
            created_at: Set(now.into()),
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: RotationLayerId) -> RepoResult<Option<RotationLayer>> {
        let model = on_call_rotation_layer::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_by_schedule(
        &self,
        schedule_id: OnCallScheduleId,
    ) -> RepoResult<Vec<RotationLayer>> {
        let models = on_call_rotation_layer::Entity::find()
            .filter(on_call_rotation_layer::Column::ScheduleId.eq::<Uuid>(schedule_id.into()))
            .order_by_desc(on_call_rotation_layer::Column::Priority)
            .order_by_asc(on_call_rotation_layer::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn update(
        &self,
        id: RotationLayerId,
        name: Option<String>,
        priority: Option<i32>,
        kind: Option<RotationKind>,
        anchor_at: Option<DateTime<Utc>>,
    ) -> RepoResult<RotationLayer> {
        let model = on_call_rotation_layer::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: on_call_rotation_layer::ActiveModel = model.into();
        if let Some(name) = name {
            active.name = Set(name);
        }
        if let Some(priority) = priority {
            active.priority = Set(priority);
        }
        if let Some(kind) = &kind {
            let cols = kind_columns(kind);
            active.kind = Set(cols.kind.to_string());
            active.interval_seconds = Set(cols.interval_seconds);
            active.handoff_time = Set(cols.handoff_time);
            active.handoff_weekday = Set(cols.handoff_weekday);
        }
        if let Some(anchor_at) = anchor_at {
            active.anchor_at = Set(anchor_at.into());
        }

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: RotationLayerId) -> RepoResult<()> {
        let res = on_call_rotation_layer::Entity::delete_by_id::<Uuid>(id.into())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }
}
