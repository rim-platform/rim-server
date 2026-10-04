use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::keybind::dto::{DefaultKeybindDto, KeybindDto, KeybindLayersDto};
use crate::keybind::request::{
    CreateKeybindCmd, DeleteKeybindCmd, InspectKeybindLayersQuery, ListDefaultKeybindsQuery,
    ListKeybindsQuery, ResolveKeybindQuery, ResolveKeybindsQuery, UpdateKeybindCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait KeybindService: Send + Sync {
    async fn list_default_keybinds(
        &self,
        query: ListDefaultKeybindsQuery,
    ) -> ServiceResult<Vec<DefaultKeybindDto>>;
    async fn list_keybinds(&self, query: ListKeybindsQuery) -> ServiceResult<Vec<KeybindDto>>;
    async fn resolve_keybinds(&self, query: ResolveKeybindsQuery)
    -> ServiceResult<Vec<KeybindDto>>;
    async fn resolve_keybind(
        &self,
        query: ResolveKeybindQuery,
    ) -> ServiceResult<Option<KeybindDto>>;
    async fn inspect_keybind_layers(
        &self,
        query: InspectKeybindLayersQuery,
    ) -> ServiceResult<KeybindLayersDto>;
    async fn create_keybind(&self, cmd: CreateKeybindCmd) -> ServiceResult<KeybindDto>;
    async fn update_keybind(&self, cmd: UpdateKeybindCmd) -> ServiceResult<KeybindDto>;
    async fn delete_keybind(&self, cmd: DeleteKeybindCmd) -> ServiceResult<()>;
}
