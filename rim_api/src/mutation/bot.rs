use std::sync::Arc;

use async_graphql::*;
use rim_core::bot::model::BotTokenId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::Actor;
use rim_services::bot::service::BotService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{Bot, BotToken, CreateBotInput, CreatedBot, CreatedBotToken, UpdateBotInput};

#[derive(Default)]
pub(crate) struct BotMutations;

#[Object]
impl BotMutations {
    /// Create a bot, a guest of the workspace in no channel, and its first token, shown this once.
    /// Owner or Moderator; `FORBIDDEN` for anyone else, `VALIDATION` for the direct space, a bad
    /// field or a workspace with 25 active bots, `FEATURE_DISABLED` while the `bots` feature is
    /// off.
    async fn create_bot(&self, ctx: &Context<'_>, input: CreateBotInput) -> Result<CreatedBot> {
        let actor = require_actor(ctx)?;
        let server_id = parse_uuid_id::<ServerId>(&input.server_id)?;
        let created = ctx
            .data::<Arc<dyn BotService>>()?
            .create_bot(
                Actor::new(actor),
                server_id,
                input.name,
                input.run_by,
                input.reads_go_to,
            )
            .await
            .map_err(map_service_error)?;
        Ok(created.into())
    }

    /// Change a bot's name or what is said about it. Owner or Moderator of its workspace.
    async fn update_bot(&self, ctx: &Context<'_>, input: UpdateBotInput) -> Result<Bot> {
        let actor = require_actor(ctx)?;
        let user_id = parse_uuid_id::<UserId>(&input.bot_user_id)?;
        let bot = ctx
            .data::<Arc<dyn BotService>>()?
            .update_bot(
                Actor::new(actor),
                user_id,
                input.name,
                input.run_by,
                input.reads_go_to,
            )
            .await
            .map_err(map_service_error)?;
        Ok(bot.into())
    }

    /// Deactivate a bot: its tokens stop working and it leaves the workspace and its channels. Its
    /// messages stay. Doing it again changes nothing. There is no way back. Owner or Moderator of
    /// its workspace.
    async fn deactivate_bot(&self, ctx: &Context<'_>, bot_user_id: ID) -> Result<Bot> {
        let actor = require_actor(ctx)?;
        let user_id = parse_uuid_id::<UserId>(&bot_user_id)?;
        let bot = ctx
            .data::<Arc<dyn BotService>>()?
            .deactivate_bot(Actor::new(actor), user_id)
            .await
            .map_err(map_service_error)?;
        Ok(bot.into())
    }

    /// Create a token for an active bot, shown this once; a label is 1 to 64 characters and a bot
    /// has at most 5 active tokens (`VALIDATION`). Owner or Moderator of its workspace.
    async fn create_bot_token(
        &self,
        ctx: &Context<'_>,
        bot_user_id: ID,
        label: String,
    ) -> Result<CreatedBotToken> {
        let actor = require_actor(ctx)?;
        let user_id = parse_uuid_id::<UserId>(&bot_user_id)?;
        let created = ctx
            .data::<Arc<dyn BotService>>()?
            .create_token(Actor::new(actor), user_id, label)
            .await
            .map_err(map_service_error)?;
        Ok(created.into())
    }

    /// Revoke a token. Revoking it again changes nothing. `NOT_FOUND` for an unknown token and for
    /// a token of a bot in a workspace the caller does not moderate.
    async fn revoke_bot_token(&self, ctx: &Context<'_>, token_id: ID) -> Result<BotToken> {
        let actor = require_actor(ctx)?;
        let token_id = parse_uuid_id::<BotTokenId>(&token_id)?;
        let token = ctx
            .data::<Arc<dyn BotService>>()?
            .revoke_token(Actor::new(actor), token_id)
            .await
            .map_err(map_service_error)?;
        Ok(token.into())
    }
}
