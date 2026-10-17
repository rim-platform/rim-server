use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::dm_policy::model::{MemberDm, OutsideDm, OutsideFloor};
use rim_core::server::model::ServerId;

use crate::dm_policy::dto::{MyDmSettingsDto, WorkspaceDmPolicyDto};
use crate::{Actor, ServiceResult};

/// The DM rules of workspaces and each person's choice of who may start a DM with them.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DmPolicyService: Send + Sync {
    /// A workspace's rules, for its members. `NotFound` for anyone else.
    async fn workspace_dm_policy(
        &self,
        actor: Actor,
        workspace_id: ServerId,
    ) -> ServiceResult<WorkspaceDmPolicyDto>;

    /// Set a workspace's rules. `Forbidden` unless the actor is its Owner.
    async fn set_workspace_dm_policy(
        &self,
        actor: Actor,
        workspace_id: ServerId,
        outside_floor: OutsideFloor,
        member_default: MemberDm,
    ) -> ServiceResult<WorkspaceDmPolicyDto>;

    async fn my_dm_settings(&self, actor: Actor) -> ServiceResult<MyDmSettingsDto>;

    /// Set the actor's choice for DMs from outside their workspaces. `Validation` naming the
    /// floor when `requests` is chosen under a floor that refuses them.
    async fn set_my_dm_settings(
        &self,
        actor: Actor,
        outside: OutsideDm,
    ) -> ServiceResult<MyDmSettingsDto>;

    /// Set the actor's choice for DMs from the members of one of their workspaces. `NotFound`
    /// unless they are a member of it.
    async fn set_my_workspace_dm_setting(
        &self,
        actor: Actor,
        workspace_id: ServerId,
        members: MemberDm,
    ) -> ServiceResult<MyDmSettingsDto>;
}
