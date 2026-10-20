use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::auth::model::InstanceInviteId;
use crate::default_workspace::model::DefaultWorkspaceRecord;
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// The instance's record of its claimed default workspace and the claim waiting for it.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DefaultWorkspaceRepository: Send + Sync {
    /// The record; every field `None` when nothing was recorded yet.
    async fn record(&self) -> RepoResult<DefaultWorkspaceRecord>;

    /// Make `invite` the invite that claims the workspace, replacing the previous one.
    async fn set_claim_invite(&self, invite: InstanceInviteId) -> RepoResult<()>;

    /// Take the claim for `invite`: clear the claim invite when it is `invite` and no workspace
    /// is recorded, locking the row for the caller's transaction. Returns whether it was taken.
    async fn take_claim(&self, invite: InstanceInviteId) -> RepoResult<bool>;

    /// Record `workspace` as the instance's default workspace, claimed by `owner` now.
    async fn record_claim(&self, workspace: ServerId, owner: UserId) -> RepoResult<()>;

    /// The local accounts other than `except` that are not deleted, oldest first.
    async fn other_local_accounts(&self, except: UserId) -> RepoResult<Vec<UserId>>;
}
