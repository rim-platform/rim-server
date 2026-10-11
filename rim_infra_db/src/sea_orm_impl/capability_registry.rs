//! SeaORM-backed implementation of the capability registry repository.
//!
//! The registry is the source of truth for "does this capability key exist
//! at all?" — `get` is the runtime gate's hot path (a row with `retired_at` set
//! counts as unregistered); `upsert_if_missing` seeds the builtins at startup.

use async_trait::async_trait;
use chrono::Utc;
use entity::capability_registry;
use rim_core::capability::model::RegisteredCapability;
use rim_core::capability::repository::CapabilityRegistryRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::{ActiveModelTrait, EntityTrait, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmCapabilityRegistryRepository {
    db: DbHandle,
}

impl SeaOrmCapabilityRegistryRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl CapabilityRegistryRepository for SeaOrmCapabilityRegistryRepository {
    async fn upsert_if_missing(
        &self,
        capability_key: &str,
        is_self_scoped: bool,
    ) -> RepoResult<()> {
        // Boot-path seed: insert when missing, leave existing rows entirely
        // untouched (an operator soft-retire on a builtin must survive a
        // redeploy). NEVER touches `retired_at` on an existing row.
        let existing = capability_registry::Entity::find_by_id(capability_key.to_string())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if existing.is_some() {
            return Ok(());
        }

        let active = capability_registry::ActiveModel {
            capability_key: Set(capability_key.to_string()),
            source_kind: Set("builtin".to_string()),
            registered_at: Set(Utc::now().into()),
            retired_at: Set(None),
            is_self_scoped: Set(is_self_scoped),
        };
        active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn get(&self, capability_key: &str) -> RepoResult<Option<RegisteredCapability>> {
        let row = capability_registry::Entity::find_by_id(capability_key.to_string())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.map(Into::into))
    }
}
