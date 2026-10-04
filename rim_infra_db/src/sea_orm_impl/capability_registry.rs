//! SeaORM-backed implementation of the capability registry repository
//! introduced in Phase 1 §3.3 (closes Black Widow C1).
//!
//! The registry is the source of truth for "does this capability key exist
//! at all?" — `is_registered` is the runtime gate's hot path. The remaining
//! methods (`upsert`, `retire_for_plugin`, `get`) cover startup seeding +
//! Phase 5 plugin lifecycle.

use async_trait::async_trait;
use chrono::Utc;
use entity::capability_registry;
use rim_core::capability::model::{CapabilitySourceKind, RegisteredCapability};
use rim_core::capability::repository::CapabilityRegistryRepository;
use rim_core::repository::error::RepoResult;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, prelude::Uuid,
};

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
    async fn is_registered(&self, capability_key: &str) -> RepoResult<bool> {
        // Hot-path gate query — primary-key lookup with active-row filter.
        // SeaORM rewrites this to `SELECT 1 FROM capability_registry WHERE
        // capability_key = $1 AND retired_at IS NULL LIMIT 1`. PK index
        // covers it; cost is constant.
        let row = capability_registry::Entity::find_by_id(capability_key.to_string())
            .filter(capability_registry::Column::RetiredAt.is_null())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(row.is_some())
    }

    async fn upsert_if_missing(
        &self,
        capability_key: &str,
        source_kind: CapabilitySourceKind,
        plugin_id: Option<Uuid>,
        is_self_scoped: bool,
    ) -> RepoResult<()> {
        // Boot-path seed: insert when missing, leave existing rows entirely
        // untouched (Gate 4 M3 — operator soft-retire on a builtin must
        // survive redeploy). NEVER touches `retired_at` on an existing row.
        let existing = capability_registry::Entity::find_by_id(capability_key.to_string())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        if existing.is_some() {
            return Ok(());
        }

        let active = capability_registry::ActiveModel {
            capability_key: Set(capability_key.to_string()),
            source_kind: Set(source_kind.as_str().to_string()),
            plugin_id: Set(plugin_id),
            registered_at: Set(Utc::now().into()),
            retired_at: Set(None),
            is_self_scoped: Set(is_self_scoped),
        };
        active.insert(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }

    async fn upsert(
        &self,
        capability_key: &str,
        source_kind: CapabilitySourceKind,
        plugin_id: Option<Uuid>,
        is_self_scoped: bool,
    ) -> RepoResult<()> {
        // Active upsert (plugin lifecycle): insert when missing; on existing
        // row, refresh `source_kind` / `plugin_id` / `is_self_scoped` AND
        // clear `retired_at`. The clear-retired_at branch is the whole point —
        // a plugin re-install must reactivate caps that were soft-retired on
        // the prior uninstall. Do NOT call this from the boot seed path.
        let existing = capability_registry::Entity::find_by_id(capability_key.to_string())
            .one(&self.db)
            .await
            .map_err(map_db_err)?;

        match existing {
            Some(model) => {
                let mut active: capability_registry::ActiveModel = model.into_active_model();
                active.source_kind = Set(source_kind.as_str().to_string());
                active.plugin_id = Set(plugin_id);
                active.retired_at = Set(None);
                active.is_self_scoped = Set(is_self_scoped);
                active.update(&self.db).await.map_err(map_db_err)?;
            }
            None => {
                let active = capability_registry::ActiveModel {
                    capability_key: Set(capability_key.to_string()),
                    source_kind: Set(source_kind.as_str().to_string()),
                    plugin_id: Set(plugin_id),
                    registered_at: Set(Utc::now().into()),
                    retired_at: Set(None),
                    is_self_scoped: Set(is_self_scoped),
                };
                active.insert(&self.db).await.map_err(map_db_err)?;
            }
        }
        Ok(())
    }

    async fn retire_for_plugin(&self, plugin_id: Uuid) -> RepoResult<()> {
        // Phase 5 surface (plugin uninstall). Implemented now so the trait
        // surface is stable and tests can exercise the soft-retire path
        // ahead of the plugin-install wiring.
        let now = Utc::now();
        let rows = capability_registry::Entity::find()
            .filter(capability_registry::Column::PluginId.eq(plugin_id))
            .filter(capability_registry::Column::RetiredAt.is_null())
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        for model in rows {
            let mut active: capability_registry::ActiveModel = model.into_active_model();
            active.retired_at = Set(Some(now.into()));
            active.update(&self.db).await.map_err(map_db_err)?;
        }
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
