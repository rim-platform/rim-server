use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::{ChannelLens, Message};

#[derive(Default)]
pub(crate) struct ChannelLensQueries;

#[Object]
impl ChannelLensQueries {
    // ── Channel Lenses ────────────────────────────────────────────────────

    async fn channel_lenses(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Vec<ChannelLens>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let lenses = svc
            .list_lenses(rim_services::channel_lens::command::ListLensesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(lenses.into_iter().map(Into::into).collect())
    }

    async fn apply_channel_lens(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        lens_id: ID,
        #[graphql(default = 50)] limit: i32,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let lens_id = parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&lens_id)?;

        let messages = svc
            .apply_lens(rim_services::channel_lens::command::ApplyLensQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                lens_id,
                limit: (limit.max(1) as u32).min(50),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(messages.into_iter().map(Into::into).collect())
    }
}
