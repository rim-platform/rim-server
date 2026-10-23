use async_graphql::*;
use rim_services::Actor;
use rim_services::server::command::{
    CreateServerCmd, DeleteWorkspaceCmd, TransferWorkspaceOwnershipCmd,
};
use rim_services::server::service::ServerService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    DeleteWorkspaceInput, DeleteWorkspacePayload, Server, TransferWorkspaceOwnershipInput,
};

#[derive(Default)]
pub(crate) struct WorkspaceMutations;

#[Object]
impl WorkspaceMutations {
    /// Join the current default workspace as Member with its public channels. Active bans
    /// are refused; existing membership is unchanged.
    async fn join_default_workspace(&self, ctx: &Context<'_>) -> Result<ID> {
        let actor = require_actor(ctx)?;
        let service = ctx
            .data::<std::sync::Arc<dyn rim_services::default_workspace::DefaultWorkspaceService>>(
            )?;
        let workspace = service
            .join(actor)
            .await
            .map_err(map_service_error)?
            .ok_or_else(|| map_service_error(rim_services::ServiceError::NotFound))?;
        Ok(ID(workspace.to_string()))
    }

    async fn create_server(&self, ctx: &Context<'_>, input: CreateServerInput) -> Result<Server> {
        let actor = require_actor(ctx)?;
        crate::federation::ensure_may_create_workspace(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let server = svc
            .create_server(CreateServerCmd {
                actor: Actor::new(actor),
                name: input.name,
            })
            .await
            .map_err(map_service_error)?;

        Ok(server.into())
    }
}

#[derive(InputObject)]
pub struct CreateServerInput {
    pub name: String,
}

/// The owner's way out of a workspace: hand it over or delete it.
#[derive(Default)]
pub(crate) struct WorkspaceOwnerMutations;

#[Object]
impl WorkspaceOwnerMutations {
    /// Make another member the workspace's owner. The caller becomes a moderator; a workspace
    /// has exactly one owner, so this is the only way the owner role moves (`setMemberRole`
    /// never grants it). Both role changes are announced with `MemberRoleChanged`.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED`: no session.
    /// - `FORBIDDEN`: the caller is not the owner.
    /// - `NOT_FOUND`: `userId` is not a member.
    /// - `VALIDATION`: the direct space, a guest as the new owner, the caller themself, or a new
    ///   owner who already owns as many workspaces as the instance allows.
    async fn transfer_workspace_ownership(
        &self,
        ctx: &Context<'_>,
        input: TransferWorkspaceOwnershipInput,
    ) -> Result<Server> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        svc.transfer_ownership(TransferWorkspaceOwnershipCmd {
            actor: Actor::new(actor),
            server_id: parse_uuid_id(&input.server_id)?,
            user_id: parse_uuid_id(&input.user_id)?,
        })
        .await
        .map(Into::into)
        .map_err(map_service_error)
    }

    /// Delete the workspace for good: its members, channels and conversations with their
    /// messages and files, invites, pins, workspace state, keybinds, pipelines, macros, plugins
    /// and custom emoji, and the notification rules scoped to it. Every member gets
    /// `MemberLeft` for themself and their event stream for the workspace ends; after that the
    /// workspace answers `NOT_FOUND` or `FORBIDDEN` like one they never joined.
    ///
    /// # Errors
    ///
    /// - `UNAUTHENTICATED`: no session.
    /// - `FORBIDDEN`: the caller is not the owner.
    /// - `VALIDATION`: the direct space, or `confirmName` is not the workspace's name.
    async fn delete_workspace(
        &self,
        ctx: &Context<'_>,
        input: DeleteWorkspaceInput,
    ) -> Result<DeleteWorkspacePayload> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ServerService>>()?;
        let deleted = svc
            .delete_workspace(DeleteWorkspaceCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id(&input.server_id)?,
                confirm_name: input.confirm_name,
            })
            .await
            .map_err(map_service_error)?;
        Ok(DeleteWorkspacePayload { deleted })
    }
}
