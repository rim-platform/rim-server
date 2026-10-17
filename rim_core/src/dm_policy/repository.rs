use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::dm_policy::model::{
    MemberDm, OutsideDm, OutsideFloor, SharedWorkspace, UserWorkspaceDm, WorkspaceDmPolicy,
};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// The workspaces' DM rules and each user's choices.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DmPolicyRepository: Send + Sync {
    /// A workspace's rules; `None` when there is no such workspace (the direct space included).
    async fn workspace_policy(&self, server_id: ServerId) -> RepoResult<Option<WorkspaceDmPolicy>>;

    /// Set a workspace's rules. Returns `None` when there is no such workspace.
    async fn set_workspace_policy(
        &self,
        server_id: ServerId,
        outside_floor: OutsideFloor,
        member_default: MemberDm,
    ) -> RepoResult<Option<WorkspaceDmPolicy>>;

    /// A user's choice for DMs from outside their workspaces; `Requests` until they chose.
    async fn outside(&self, user_id: UserId) -> RepoResult<OutsideDm>;

    async fn set_outside(&self, user_id: UserId, outside: OutsideDm) -> RepoResult<()>;

    /// Set a user's choice for DMs from a workspace's members.
    async fn set_member_choice(
        &self,
        user_id: UserId,
        server_id: ServerId,
        members: MemberDm,
    ) -> RepoResult<()>;

    /// The workspaces a user is a member of, by name, with their rules and the user's choice.
    async fn user_workspaces(&self, user_id: UserId) -> RepoResult<Vec<UserWorkspaceDm>>;

    /// The workspaces a user shares with the holder of `principal_id`, with that holder's role.
    async fn shared_workspaces(
        &self,
        user_id: UserId,
        principal_id: &str,
    ) -> RepoResult<Vec<SharedWorkspace>>;
}
