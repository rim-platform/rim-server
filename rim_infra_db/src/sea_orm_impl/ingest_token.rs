use async_trait::async_trait;
use chrono::{DateTime, Utc};
use entity::workspace_ingest_token;
use rim_core::ingest_token::model::{IngestToken, IngestTokenId, NewIngestToken};
use rim_core::ingest_token::repository::IngestTokenRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, QueryOrder, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmIngestTokenRepository {
    db: DbHandle,
}

impl SeaOrmIngestTokenRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl IngestTokenRepository for SeaOrmIngestTokenRepository {
    async fn create(&self, new: NewIngestToken) -> RepoResult<IngestToken> {
        let active = workspace_ingest_token::ActiveModel {
            id: Set(IngestTokenId::new().into()),
            server_id: Set(new.server_id.into()),
            conversation_id: Set(new.conversation_id.into()),
            created_by: Set(new.created_by.into()),
            label: Set(new.label),
            token_hash: Set(new.token_hash),
            severity_default: Set(new.severity_default.map(|s| s.as_str().to_string())),
            created_at: Set(Utc::now().into()),
            revoked_at: Set(None),
            last_used_at: Set(None),
        };

        let inserted = active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(inserted.into())
    }

    async fn find_by_id(&self, id: IngestTokenId) -> RepoResult<Option<IngestToken>> {
        let model = workspace_ingest_token::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn find_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<IngestToken>> {
        let model = workspace_ingest_token::Entity::find()
            .filter(workspace_ingest_token::Column::TokenHash.eq(token_hash))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<IngestToken>> {
        let models = workspace_ingest_token::Entity::find()
            .filter(workspace_ingest_token::Column::ServerId.eq::<Uuid>(server_id.into()))
            .order_by_desc(workspace_ingest_token::Column::CreatedAt)
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn revoke(&self, id: IngestTokenId) -> RepoResult<()> {
        let model = workspace_ingest_token::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: workspace_ingest_token::ActiveModel = model.into();
        active.revoked_at = Set(Some(Utc::now().into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn touch_last_used(&self, id: IngestTokenId, at: DateTime<Utc>) -> RepoResult<()> {
        let model = workspace_ingest_token::Entity::find_by_id::<Uuid>(id.into())
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let mut active: workspace_ingest_token::ActiveModel = model.into();
        active.last_used_at = Set(Some(at.into()));
        active.update(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
