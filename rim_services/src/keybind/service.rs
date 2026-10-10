use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::keybind::dto::KeybindDto;
use crate::keybind::request::{
    CreateKeybindCmd, DeleteKeybindCmd, ResolveKeybindsQuery, UpdateKeybindCmd,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait KeybindService: Send + Sync {
    async fn resolve_keybinds(&self, query: ResolveKeybindsQuery)
    -> ServiceResult<Vec<KeybindDto>>;
    async fn create_keybind(&self, cmd: CreateKeybindCmd) -> ServiceResult<KeybindDto>;
    async fn update_keybind(&self, cmd: UpdateKeybindCmd) -> ServiceResult<KeybindDto>;
    async fn delete_keybind(&self, cmd: DeleteKeybindCmd) -> ServiceResult<()>;
}
