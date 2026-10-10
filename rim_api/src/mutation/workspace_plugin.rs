use async_graphql::*;
use rim_core::server::model::ServerId;
use rim_core::server_feature::model::ServerFeature;
use rim_services::Actor;
use rim_services::workspace_plugin::command::{
    AddWorkspacePluginCmd, RemoveWorkspacePluginCmd, UpdateWorkspacePluginCmd,
};
use rim_services::workspace_plugin::service::WorkspacePluginService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::features::require_feature;
use crate::ids::parse_uuid_id;
use crate::types::{AddWorkspacePluginInput, UpdateWorkspacePluginInput, WorkspacePlugin};

#[derive(Default)]
pub(crate) struct WorkspacePluginMutations;

#[Object]
impl WorkspacePluginMutations {
    /// Add the statement's plugin to the workspace's registry. Owner or Moderator.
    async fn add_workspace_plugin(
        &self,
        ctx: &Context<'_>,
        input: AddWorkspacePluginInput,
    ) -> Result<WorkspacePlugin> {
        require_feature(ctx, ServerFeature::WorkspacePlugins)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn WorkspacePluginService>>()?;
        let dto = svc
            .add(AddWorkspacePluginCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id::<ServerId>(&input.server_id)?,
                statement: input.statement.into(),
                status: input.status.into(),
                bundle_url: input.bundle_url,
                publisher_key_log: input
                    .publisher_key_log
                    .map(|log| log.into_iter().map(Into::into).collect()),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Change an entry's statement, status or bundle URL. Owner or Moderator.
    async fn update_workspace_plugin(
        &self,
        ctx: &Context<'_>,
        input: UpdateWorkspacePluginInput,
    ) -> Result<WorkspacePlugin> {
        require_feature(ctx, ServerFeature::WorkspacePlugins)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn WorkspacePluginService>>()?;
        let dto = svc
            .update(UpdateWorkspacePluginCmd {
                actor: Actor::new(actor),
                server_id: parse_uuid_id::<ServerId>(&input.server_id)?,
                plugin_id: input.plugin_id,
                statement: input.statement.map(Into::into),
                status: input.status.map(Into::into),
                bundle_url: input.bundle_url,
                publisher_key_log: input
                    .publisher_key_log
                    .map(|log| log.into_iter().map(Into::into).collect()),
            })
            .await
            .map_err(map_service_error)?;
        Ok(dto.into())
    }

    /// Remove an entry from the workspace's registry. Owner or Moderator.
    async fn remove_workspace_plugin(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        plugin_id: String,
    ) -> Result<bool> {
        require_feature(ctx, ServerFeature::WorkspacePlugins)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn WorkspacePluginService>>()?;
        svc.remove(RemoveWorkspacePluginCmd {
            actor: Actor::new(actor),
            server_id: parse_uuid_id::<ServerId>(&server_id)?,
            plugin_id,
        })
        .await
        .map_err(map_service_error)?;
        Ok(true)
    }
}
