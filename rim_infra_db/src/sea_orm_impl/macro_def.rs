use async_trait::async_trait;
use entity::macro_def;
use rim_core::macro_def::model::{MacroDef, MacroId, NewMacroDef, UpdateMacroDef};
use rim_core::macro_def::repository::MacroRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, PaginatorTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmMacroRepository {
    db: DbHandle,
}

impl SeaOrmMacroRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

#[async_trait]
impl MacroRepository for SeaOrmMacroRepository {
    async fn get_by_id(&self, id: MacroId) -> RepoResult<Option<MacroDef>> {
        let model = macro_def::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn find_by_name(
        &self,
        server_id: ServerId,
        user_id: UserId,
        name: &str,
    ) -> RepoResult<Option<MacroDef>> {
        let model = macro_def::Entity::find()
            .filter(macro_def::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(macro_def::Column::UserId.eq::<Uuid>(user_id.into()))
            .filter(macro_def::Column::Name.eq(name))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_for_user(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<MacroDef>> {
        let models = macro_def::Entity::find()
            .filter(macro_def::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(macro_def::Column::UserId.eq::<Uuid>(user_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<MacroDef>> {
        let models = macro_def::Entity::find()
            .filter(macro_def::Column::ServerId.eq::<Uuid>(server_id.into()))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64> {
        let count = macro_def::Entity::find()
            .filter(macro_def::Column::ServerId.eq::<Uuid>(server_id.into()))
            .filter(macro_def::Column::UserId.eq::<Uuid>(user_id.into()))
            .count(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(count)
    }

    async fn create(&self, new_macro: NewMacroDef) -> RepoResult<MacroDef> {
        let intent_samples = serde_json::to_value(&new_macro.intent_samples)
            .unwrap_or(serde_json::Value::Array(vec![]));
        let model = macro_def::ActiveModel {
            id: Set(MacroId::new().into()),
            server_id: Set(new_macro.server_id.into()),
            user_id: Set(new_macro.user_id.into()),
            name: Set(new_macro.name),
            description: Set(new_macro.description),
            steps: Set(new_macro.steps_json),
            ai_callable: Set(new_macro.ai_callable),
            nl_description: Set(new_macro.nl_description),
            intent_samples: Set(intent_samples),
            sensitivity: Set(new_macro.sensitivity),
            ..Default::default()
        };

        let created = macro_def::Entity::insert(model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(created.into())
    }

    async fn update(&self, id: MacroId, update: UpdateMacroDef) -> RepoResult<MacroDef> {
        let model = macro_def::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;

        let intent_samples = serde_json::to_value(&update.intent_samples)
            .unwrap_or(serde_json::Value::Array(vec![]));
        let mut active: macro_def::ActiveModel = model.into();
        active.name = Set(update.name);
        active.description = Set(update.description);
        active.steps = Set(update.steps_json);
        active.ai_callable = Set(update.ai_callable);
        active.nl_description = Set(update.nl_description);
        active.intent_samples = Set(intent_samples);
        active.sensitivity = Set(update.sensitivity);
        active.updated_at = Set(chrono::Utc::now().into());

        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete(&self, id: MacroId) -> RepoResult<()> {
        let model = macro_def::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let active: macro_def::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
