use crate::repository::error::RepoResult;
use crate::server::model::{DeletedWorkspaces, Server, ServerId};
use crate::user::model::UserId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait ServerRepository: Send + Sync {
    async fn get_by_id(&self, id: ServerId) -> RepoResult<Option<Server>>;

    async fn create_server(&self, name: &str) -> RepoResult<Server>;

    /// Give the workspace `id` the name `name`. Returns whether it exists.
    async fn rename(&self, id: ServerId, name: &str) -> RepoResult<bool>;

    async fn get_servers_for_user(&self, user_id: UserId) -> RepoResult<Vec<Server>>;

    /// The workspace `owner` owns whose trimmed name equals `name` ignoring case.
    async fn find_owned_by_name(&self, owner: UserId, name: &str) -> RepoResult<Option<Server>>;

    /// How many workspaces `owner` owns; the direct space is not one.
    async fn count_owned(&self, owner: UserId) -> RepoResult<u64>;

    /// Delete the workspaces in `ids` with everything in them: members, channels and
    /// conversations with their messages, attachments, pins and threads, invites, bans, workspace
    /// state, keybinds, pipelines, macros, the plugin registry and bundles, custom emoji and the
    /// notification rules scoped to them. The direct space is never deleted. Run it in a
    /// transaction and delete the returned files after it commits.
    async fn delete_workspaces(&self, ids: &[ServerId]) -> RepoResult<DeletedWorkspaces>;
}
