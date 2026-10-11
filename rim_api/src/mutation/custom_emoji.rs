use async_graphql::*;
use base64::Engine;
use rim_core::server_feature::model::ServerFeature;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::features::require_feature;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct CustomEmojiMutations;

#[Object]
impl CustomEmojiMutations {
    async fn create_custom_emoji(
        &self,
        ctx: &Context<'_>,
        input: CreateCustomEmojiInput,
    ) -> Result<crate::types::CustomEmoji> {
        require_feature(ctx, ServerFeature::CustomEmoji)?;
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let image_data = base64::engine::general_purpose::STANDARD
            .decode(&input.image_base64)
            .map_err(|e| {
                map_service_error(rim_services::ServiceError::Validation(format!(
                    "invalid base64: {e}"
                )))
            })?;

        let emoji = svc
            .create(rim_services::custom_emoji::command::CreateCustomEmojiCmd {
                actor: Actor::new(actor),
                server_id,
                name: input.name,
                image_data,
            })
            .await
            .map_err(map_service_error)?;

        Ok(emoji.into())
    }

    async fn delete_custom_emoji(&self, ctx: &Context<'_>, emoji_id: ID) -> Result<bool> {
        require_feature(ctx, ServerFeature::CustomEmoji)?;
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>>(
            )?;
        let emoji_id = parse_uuid_id::<rim_core::custom_emoji::model::CustomEmojiId>(&emoji_id)?;

        svc.delete(rim_services::custom_emoji::command::DeleteCustomEmojiCmd {
            actor: Actor::new(actor),
            emoji_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
struct CreateCustomEmojiInput {
    server_id: ID,
    name: String,
    image_base64: String,
}
