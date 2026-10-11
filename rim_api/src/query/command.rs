use async_graphql::*;
use rim_core::server_feature::model::ServerFeature;
use rim_services::Actor;
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::service::CommandService;

use crate::auth_context::require_actor;
use crate::features::{feature_state, require_feature, shown_command};
use crate::ids::parse_uuid_id;
use crate::types::{Command, CommandContextInput, CommandSurface};

#[derive(Default)]
pub(crate) struct CommandQueries;

#[Object]
impl CommandQueries {
    async fn commands(
        &self,
        ctx: &Context<'_>,
        surface: Option<CommandSurface>,
        command_context: Option<CommandContextInput>,
    ) -> Result<Vec<Command>> {
        if surface == Some(CommandSurface::Ai) {
            require_feature(ctx, ServerFeature::AiCommands)?;
        }
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;
        let ai_commands = feature_state(ctx, ServerFeature::AiCommands)?;

        let commands = svc
            .list_commands(ListCommandsQuery {
                actor: Actor::new(actor),
                context: command_context.map(TryInto::try_into).transpose()?,
                surface: surface.unwrap_or(CommandSurface::Palette).into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(commands
            .into_iter()
            .map(|c| shown_command(&ai_commands, c.into()))
            .collect())
    }

    /// The AI-callable commands the caller may run, for an AI agent to discover its tools.
    ///
    /// With `intentPhrase` the commands are ranked by how well the phrase matches their
    /// intent samples, title and natural-language description. `limit` defaults to 50 and is
    /// capped at 100. `workspaceId` scopes the list to one workspace, including its AI-callable
    /// macros.
    ///
    /// Limited to 5 calls per 60-second sliding window per user; more fail with
    /// `RATE_LIMITED`. Fails with `FEATURE_DISABLED` while the AI command surface is off.
    async fn command_discover(
        &self,
        ctx: &Context<'_>,
        intent_phrase: Option<String>,
        limit: Option<i32>,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Command>> {
        require_feature(ctx, ServerFeature::AiCommands)?;
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let ws_id = workspace_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::server::model::ServerId>)
            .transpose()?;

        let commands = svc
            .command_discover(
                &Actor::new(actor),
                intent_phrase,
                limit.map(|l| l.max(1) as u32),
                ws_id,
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(commands.into_iter().map(Into::into).collect())
    }
}
