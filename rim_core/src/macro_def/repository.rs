use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::macro_def::model::{MacroDef, MacroId, NewMacroDef, UpdateMacroDef};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MacroRepository: Send + Sync {
    async fn get_by_id(&self, id: MacroId) -> RepoResult<Option<MacroDef>>;
    async fn find_by_name(
        &self,
        server_id: ServerId,
        user_id: UserId,
        name: &str,
    ) -> RepoResult<Option<MacroDef>>;
    async fn list_for_user(
        &self,
        server_id: ServerId,
        user_id: UserId,
    ) -> RepoResult<Vec<MacroDef>>;
    async fn count_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;
    async fn create(&self, new_macro: NewMacroDef) -> RepoResult<MacroDef>;
    async fn update(&self, id: MacroId, update: UpdateMacroDef) -> RepoResult<MacroDef>;
    async fn delete(&self, id: MacroId) -> RepoResult<()>;
    /// How many macros `server_id` holds, of every user.
    async fn count_for_server(&self, server_id: ServerId) -> RepoResult<u64>;
    /// Delete every macro `user_id` holds in `server_id`; returns how many were deleted.
    async fn delete_for_user(&self, server_id: ServerId, user_id: UserId) -> RepoResult<u64>;
}
