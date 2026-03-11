use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

use crate::keybind::model::{
    Keybind, KeybindContext, KeybindId, KeybindScope, KeybindSubject, NewKeybind, UpdateKeybind,
};
use crate::repository::error::RepoResult;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait KeybindRepository: Send + Sync {
    async fn get_by_id(&self, id: KeybindId) -> RepoResult<Option<Keybind>>;
    async fn list_for_subject(&self, subject: KeybindSubject) -> RepoResult<Vec<Keybind>>;
    async fn get_by_binding(
        &self,
        scope: KeybindScope,
        subject: KeybindSubject,
        context: KeybindContext,
        keystroke: &str,
    ) -> RepoResult<Option<Keybind>>;
    async fn create_keybind(&self, new_keybind: NewKeybind) -> RepoResult<Keybind>;
    async fn update_keybind(&self, id: KeybindId, update: UpdateKeybind) -> RepoResult<Keybind>;
    async fn delete_keybind(&self, id: KeybindId) -> RepoResult<()>;
}
