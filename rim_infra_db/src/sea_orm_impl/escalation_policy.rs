use async_trait::async_trait;
use entity::escalation_policy;
use rim_core::escalation::model::{EscalationPolicy, EscalationPolicyId, NewEscalationPolicy};
use rim_core::escalation::repository::EscalationPolicyRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmEscalationPolicyRepository {
    pub db: DbHandle,
}

impl SeaOrmEscalationPolicyRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl EscalationPolicyRepository for SeaOrmEscalationPolicyRepository {
    async fn create(&self, new: NewEscalationPolicy) -> RepoResult<EscalationPolicy> {
        let now = chrono::Utc::now();
        let active = escalation_policy::ActiveModel {
            id: Set(EscalationPolicyId::new().into()),
            server_id: Set(new.server_id.into()),
            name: Set(new.name),
            created_by: Set(new.created_by.into()),
            created_at: Set(now.into()),
            updated_at: Set(now.into()),
        };
        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: EscalationPolicyId) -> RepoResult<Option<EscalationPolicy>> {
        let model = escalation_policy::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_by_server(&self, server_id: ServerId) -> RepoResult<Vec<EscalationPolicy>> {
        let models = escalation_policy::Entity::find()
            .filter(escalation_policy::Column::ServerId.eq::<Uuid>(server_id.into()))
            .order_by_asc(escalation_policy::Column::CreatedAt)
            .order_by_asc(escalation_policy::Column::Id)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn update_name(
        &self,
        id: EscalationPolicyId,
        name: String,
    ) -> RepoResult<EscalationPolicy> {
        let model = escalation_policy::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: escalation_policy::ActiveModel = model.into();
        active.name = Set(name);
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: EscalationPolicyId) -> RepoResult<()> {
        let res = escalation_policy::Entity::delete_by_id::<Uuid>(id.into())
            .exec(&self.db)
            .await
            .map_err(map_db_err)?;
        if res.rows_affected == 0 {
            return Err(RepoError::new(RepoErrorKind::NotFound));
        }
        Ok(())
    }
}
