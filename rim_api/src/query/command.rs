use async_graphql::*;
use rim_services::Actor;
use rim_services::command::request::ListCommandsQuery;
use rim_services::command::service::CommandService;

use crate::auth_context::require_actor;
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
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let commands = svc
            .list_commands(ListCommandsQuery {
                actor: Actor::new(actor),
                context: command_context.map(TryInto::try_into).transpose()?,
                surface: surface.unwrap_or(CommandSurface::Palette).into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(commands.into_iter().map(Into::into).collect())
    }

    /// P5-A7 — AI Agent command-discovery meta-tool (v2 §5 R1 #3, M6).
    ///
    /// Returns the actor's accessible AI-callable command surface, optionally
    /// ranked by `intent_phrase` relevance (literal substring + token-overlap
    /// against `intent_samples`, `title`, `nl_description`), truncated to
    /// `limit` (default 50, hard-capped at 100), and scoped to a workspace for
    /// macro synthesis.
    ///
    /// Rate-limited: 5 calls / 60-second sliding window per actor (M6).
    /// Returns `RATE_LIMITED` on overage.
    async fn command_discover(
        &self,
        ctx: &Context<'_>,
        intent_phrase: Option<String>,
        limit: Option<i32>,
        workspace_id: Option<ID>,
    ) -> Result<Vec<Command>> {
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

    async fn command(&self, ctx: &Context<'_>, command_id: String) -> Result<Option<Command>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn CommandService>>()?;

        let command = svc
            .get_command(rim_services::command::request::GetCommandQuery {
                actor: Actor::new(actor),
                command_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(command.map(Into::into))
    }
}
