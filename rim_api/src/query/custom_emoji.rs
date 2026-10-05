use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::types::CustomEmoji;

#[derive(Default)]
pub(crate) struct CustomEmojiQueries;

#[Object]
impl CustomEmojiQueries {
    async fn custom_emojis(&self, ctx: &Context<'_>, server_id: ID) -> Result<Vec<CustomEmoji>> {
        let actor = require_actor(ctx)?;

        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let emojis = svc
            .list(rim_services::custom_emoji::command::ListCustomEmojisQuery {
                actor: Actor::new(actor),
                server_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(emojis.into_iter().map(Into::into).collect())
    }
}
