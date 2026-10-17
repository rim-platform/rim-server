use std::sync::Arc;

use async_graphql::*;
use rim_services::Actor;
use rim_services::dm_policy::service::DmPolicyService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{DmMembers, DmOutside, DmOutsideFloor, MyDmSettings, WorkspaceDmPolicy};

#[derive(Default)]
pub(crate) struct DmPolicyMutations;

#[Object]
impl DmPolicyMutations {
    /// Set a workspace's DM rules: the floor for DMs to its members from outside their
    /// workspaces, and how a DM from a fellow member arrives unless the recipient chose. The
    /// workspace's Owner only; anyone else gets `FORBIDDEN`.
    async fn set_workspace_dm_policy(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
        outside_floor: DmOutsideFloor,
        member_default: DmMembers,
    ) -> Result<WorkspaceDmPolicy> {
        let actor = require_actor(ctx)?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;
        Ok(ctx
            .data::<Arc<dyn DmPolicyService>>()?
            .set_workspace_dm_policy(
                Actor::new(actor),
                workspace_id,
                outside_floor.into(),
                member_default.into(),
            )
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Set who outside the caller's workspaces may start a DM with them: `REQUESTS`, or
    /// `NONE`. `REQUESTS` under a floor that refuses such DMs is `VALIDATION`, naming it.
    async fn set_my_dm_settings(
        &self,
        ctx: &Context<'_>,
        outside: DmOutside,
    ) -> Result<MyDmSettings> {
        let actor = require_actor(ctx)?;
        Ok(ctx
            .data::<Arc<dyn DmPolicyService>>()?
            .set_my_dm_settings(Actor::new(actor), outside.into())
            .await
            .map_err(map_service_error)?
            .into())
    }

    /// Set how a DM from the members of one of the caller's workspaces arrives, whatever the
    /// workspace's default. `NOT_FOUND` unless the caller is a member.
    async fn set_my_workspace_dm_setting(
        &self,
        ctx: &Context<'_>,
        workspace_id: ID,
        members: DmMembers,
    ) -> Result<MyDmSettings> {
        let actor = require_actor(ctx)?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;
        Ok(ctx
            .data::<Arc<dyn DmPolicyService>>()?
            .set_my_workspace_dm_setting(Actor::new(actor), workspace_id, members.into())
            .await
            .map_err(map_service_error)?
            .into())
    }
}
