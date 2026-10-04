use async_trait::async_trait;
use chrono::Utc;
use entity::workspace_feature;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_core::workspace_feature::model::{WorkspaceFeature, WorkspaceFeatureState};
use rim_core::workspace_feature::repository::WorkspaceFeatureRepository;
use sea_orm::prelude::Uuid;
use sea_orm::sea_query::OnConflict;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmWorkspaceFeatureRepository {
    db: DbHandle,
}

impl SeaOrmWorkspaceFeatureRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl WorkspaceFeatureRepository for SeaOrmWorkspaceFeatureRepository {
    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<WorkspaceFeatureState>> {
        let models = workspace_feature::Entity::find()
            .filter(workspace_feature::Column::ServerId.eq::<Uuid>(server_id.into()))
            .order_by_asc(workspace_feature::Column::Feature)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;

        Ok(models
            .into_iter()
            .filter_map(|m| {
                let feature = m.feature.clone();
                WorkspaceFeatureState::try_from(m)
                    .map_err(|e| {
                        tracing::warn!(
                            server_id = %server_id,
                            feature = %feature,
                            error = %e,
                            "skipping workspace_feature row with unknown feature"
                        );
                    })
                    .ok()
            })
            .collect())
    }

    async fn set(
        &self,
        server_id: ServerId,
        feature: WorkspaceFeature,
        enabled: bool,
        updated_by: Option<UserId>,
    ) -> RepoResult<WorkspaceFeatureState> {
        let active = workspace_feature::ActiveModel {
            server_id: Set(server_id.into()),
            feature: Set(feature.as_str().to_string()),
            enabled: Set(enabled),
            updated_by: Set(updated_by.map(Into::into)),
            updated_at: Set(Utc::now().into()),
        };
        // Upsert via `ON CONFLICT` on the composite primary key: avoids a find-then-save race
        // between two owners toggling the same feature concurrently.
        let model = workspace_feature::Entity::insert(active)
            .on_conflict(
                OnConflict::columns([
                    workspace_feature::Column::ServerId,
                    workspace_feature::Column::Feature,
                ])
                .update_columns([
                    workspace_feature::Column::Enabled,
                    workspace_feature::Column::UpdatedBy,
                    workspace_feature::Column::UpdatedAt,
                ])
                .to_owned(),
            )
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;

        WorkspaceFeatureState::try_from(model)
            .map_err(|e| RepoError::with_message(RepoErrorKind::InvalidData, e.to_string()))
    }

    async fn is_enabled(&self, server_id: ServerId, feature: WorkspaceFeature) -> RepoResult<bool> {
        let model = workspace_feature::Entity::find_by_id::<(Uuid, String)>((
            server_id.into(),
            feature.as_str().to_string(),
        ))
        .one(&self.db)
        .await
        .map_err(map_db_err)?;
        Ok(model.map_or_else(|| feature.default_enabled(), |m| m.enabled))
    }
}
