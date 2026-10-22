use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use crate::workspace_state::model::{
    NewWorkspaceState, WorkspaceState, WorkspaceStateScope, WorkspaceStateUsage,
};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceStateRepository: Send + Sync {
    async fn get_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
    ) -> RepoResult<Option<WorkspaceState>>;

    async fn list_by_prefix(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        prefix: &str,
        limit: u64,
    ) -> RepoResult<Vec<WorkspaceState>>;

    async fn count(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
    ) -> RepoResult<u64>;

    /// The unexpired workspace-scoped entries of `server_id` that `creator` created, counted
    /// with the bytes of their values.
    async fn creator_usage(
        &self,
        server_id: ServerId,
        creator: UserId,
    ) -> RepoResult<WorkspaceStateUsage>;

    /// Insert `new`, or overwrite the value and expiry of the entry stored under its key. An
    /// expired entry not yet purged is replaced as a new one: its creator becomes `created_by`,
    /// its version 1 and its creation time now.
    async fn upsert(&self, new: NewWorkspaceState) -> RepoResult<WorkspaceState>;

    async fn delete_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
    ) -> RepoResult<bool>;

    async fn delete_expired(&self) -> RepoResult<u64>;

    /// Atomically increment a numeric value by `delta`, inserting with `delta` if the key does not
    /// exist or its entry expired (then created by `actor_id`, version 1, without expiry). Uses a
    /// single SQL statement to avoid TOCTOU race conditions.
    async fn increment_by_key(
        &self,
        server_id: ServerId,
        scope: WorkspaceStateScope,
        user_id: Option<UserId>,
        key: &str,
        delta: i64,
        actor_id: UserId,
    ) -> RepoResult<WorkspaceState>;
}
