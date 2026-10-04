use async_trait::async_trait;
use entity::keybind;
use rim_core::keybind::model::{
    Keybind, KeybindContext, KeybindId, KeybindScope, KeybindSubject, NewKeybind, UpdateKeybind,
};
use rim_core::keybind::repository::KeybindRepository;
use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use sea_orm::prelude::Uuid;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, QueryFilter, Set};

use crate::DbHandle;
use crate::error::map_db_err;

pub struct SeaOrmKeybindRepository {
    db: DbHandle,
}

impl SeaOrmKeybindRepository {
    pub fn new(db: DbHandle) -> Self {
        Self { db }
    }
}

fn subject_uuid(subject: KeybindSubject) -> Uuid {
    match subject {
        KeybindSubject::User(id) => id.into(),
        KeybindSubject::Workspace(id) => id.into(),
    }
}

#[async_trait]
impl KeybindRepository for SeaOrmKeybindRepository {
    async fn get_by_id(&self, id: KeybindId) -> RepoResult<Option<Keybind>> {
        let model = keybind::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn list_for_subject(&self, subject: KeybindSubject) -> RepoResult<Vec<Keybind>> {
        let scope = match subject {
            KeybindSubject::User(_) => KeybindScope::User,
            KeybindSubject::Workspace(_) => KeybindScope::Workspace,
        };
        let models = keybind::Entity::find()
            .filter(keybind::Column::Scope.eq(scope.to_string()))
            .filter(keybind::Column::SubjectId.eq(subject_uuid(subject)))
            .all(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(models.into_iter().map(Into::into).collect())
    }

    async fn get_by_binding(
        &self,
        scope: KeybindScope,
        subject: KeybindSubject,
        context: KeybindContext,
        keystroke: &str,
    ) -> RepoResult<Option<Keybind>> {
        let model = keybind::Entity::find()
            .filter(keybind::Column::Scope.eq(scope.to_string()))
            .filter(keybind::Column::SubjectId.eq(subject_uuid(subject)))
            .filter(keybind::Column::Context.eq(context.to_string()))
            .filter(keybind::Column::Keystroke.eq(keystroke))
            .one(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(model.map(Into::into))
    }

    async fn create_keybind(&self, new_keybind: NewKeybind) -> RepoResult<Keybind> {
        let model = keybind::ActiveModel {
            id: Set(KeybindId::new().into()),
            scope: Set(new_keybind.scope.to_string()),
            subject_id: Set(subject_uuid(new_keybind.subject)),
            keystroke: Set(new_keybind.keystroke),
            command_id: Set(new_keybind.command_id),
            context: Set(new_keybind.context.to_string()),
            preset_args: Set(new_keybind.preset_args),
            is_enabled: Set(new_keybind.is_enabled),
            ..Default::default()
        };

        let created = keybind::Entity::insert(model)
            .exec_with_returning(&self.db)
            .await
            .map_err(map_db_err)?;
        Ok(created.into())
    }

    async fn update_keybind(&self, id: KeybindId, update: UpdateKeybind) -> RepoResult<Keybind> {
        let model = keybind::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let mut active: keybind::ActiveModel = model.into();
        active.keystroke = Set(update.keystroke);
        active.command_id = Set(update.command_id);
        active.context = Set(update.context.to_string());
        active.preset_args = Set(update.preset_args);
        active.is_enabled = Set(update.is_enabled);
        active.updated_at = Set(chrono::Utc::now().into());
        let updated = active.update(&self.db).await.map_err(map_db_err)?;
        Ok(updated.into())
    }

    async fn delete_keybind(&self, id: KeybindId) -> RepoResult<()> {
        let model = keybind::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .map_err(map_db_err)?
            .ok_or_else(|| RepoError::new(RepoErrorKind::NotFound))?;
        let active: keybind::ActiveModel = model.into();
        active.delete(&self.db).await.map_err(map_db_err)?;
        Ok(())
    }
}
