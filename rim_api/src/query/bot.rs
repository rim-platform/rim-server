use std::sync::Arc;

use async_graphql::*;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use rim_services::Actor;
use rim_services::bot::service::BotService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{
    Bot, BotToken, IncomingWebhook, WebhookFormatInfo, WebhookFormatPreview, WebhookHeader,
};

#[derive(Default)]
pub(crate) struct BotQueries;

#[Object]
impl BotQueries {
    /// The workspace's bots, deactivated ones included. Any member; `FORBIDDEN` for anyone else,
    /// `FEATURE_DISABLED` while the `bots` feature is off.
    async fn bots(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<Bot>> {
        let actor = require_actor(ctx)?;
        let server_id = parse_uuid_id::<ServerId>(&server_id)?;
        let bots = ctx
            .data::<Arc<dyn BotService>>()?
            .list_bots(Actor::new(actor), server_id)
            .await
            .map_err(map_service_error)?;
        Ok(bots.into_iter().map(Into::into).collect())
    }

    /// The bot's tokens, revoked ones included, never their secret. Owner or Moderator of its
    /// workspace; `FORBIDDEN` for anyone else, `NOT_FOUND` for an unknown bot, `FEATURE_DISABLED`
    /// while the `bots` feature is off.
    async fn bot_tokens(&self, ctx: &Context<'_>, bot_user_id: ID) -> Result<Vec<BotToken>> {
        let actor = require_actor(ctx)?;
        let user_id = parse_uuid_id::<UserId>(&bot_user_id)?;
        let tokens = ctx
            .data::<Arc<dyn BotService>>()?
            .list_tokens(Actor::new(actor), user_id)
            .await
            .map_err(map_service_error)?;
        Ok(tokens.into_iter().map(Into::into).collect())
    }

    /// The workspace's incoming webhooks, revoked ones included, never their token. Owner or
    /// Moderator; `FORBIDDEN` for anyone else, `FEATURE_DISABLED` while the `bots` feature is off.
    async fn incoming_webhooks(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<IncomingWebhook>> {
        let actor = require_actor(ctx)?;
        let server_id = parse_uuid_id::<ServerId>(&server_id)?;
        let webhooks = ctx
            .data::<Arc<dyn BotService>>()?
            .list_webhooks(Actor::new(actor), server_id)
            .await
            .map_err(map_service_error)?;
        Ok(webhooks.into_iter().map(Into::into).collect())
    }

    /// The ready-made webhook formats, the same for everyone signed in. `FEATURE_DISABLED` while
    /// the `bots` feature is off.
    async fn webhook_formats(&self, ctx: &Context<'_>) -> Result<Vec<WebhookFormatInfo>> {
        require_actor(ctx)?;
        let formats = ctx
            .data::<Arc<dyn BotService>>()?
            .webhook_formats()
            .map_err(map_service_error)?;
        Ok(formats.into_iter().map(Into::into).collect())
    }

    /// What a sample delivery would come to under `template`, as a delivery runs it. Nothing is
    /// posted or stored. Owner or Moderator of the workspace; `FORBIDDEN` for anyone else,
    /// `VALIDATION` for a sample over 256 KiB.
    async fn preview_webhook_format(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        template: Json<serde_json::Value>,
        headers: Vec<WebhookHeader>,
        body: String,
    ) -> Result<WebhookFormatPreview> {
        let actor = require_actor(ctx)?;
        let server_id = parse_uuid_id::<ServerId>(&server_id)?;
        let preview = ctx
            .data::<Arc<dyn BotService>>()?
            .preview_webhook_format(
                Actor::new(actor),
                server_id,
                template.0,
                headers
                    .into_iter()
                    .map(|header| (header.name, header.value))
                    .collect(),
                body,
            )
            .await
            .map_err(map_service_error)?;
        Ok(preview.into())
    }
}
