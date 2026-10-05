use async_graphql::*;
use rim_services::Actor;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;

#[derive(Default)]
pub(crate) struct ChannelLensMutations;

#[Object]
impl ChannelLensMutations {
    // ── Channel Lenses ────────────────────────────────────────────────────

    async fn create_channel_lens(
        &self,
        ctx: &Context<'_>,
        input: CreateChannelLensInput,
    ) -> Result<crate::types::ChannelLens> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let lens = svc
            .create_lens(rim_services::channel_lens::command::CreateLensCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                name: input.name,
                filter_spec: input.filter_spec.into(),
                is_shared: input.is_shared.unwrap_or(false),
            })
            .await
            .map_err(map_service_error)?;

        Ok(lens.into())
    }

    async fn update_channel_lens(
        &self,
        ctx: &Context<'_>,
        input: UpdateChannelLensInput,
    ) -> Result<crate::types::ChannelLens> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let lens_id =
            parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&input.lens_id)?;

        let lens = svc
            .update_lens(rim_services::channel_lens::command::UpdateLensCmd {
                actor: Actor::new(actor),
                server_id,
                lens_id,
                name: input.name,
                filter_spec: input.filter_spec.map(Into::into),
                is_shared: input.is_shared,
            })
            .await
            .map_err(map_service_error)?;

        Ok(lens.into())
    }

    async fn delete_channel_lens(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        lens_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::channel_lens::service::ChannelLensService>>(
            )?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let lens_id = parse_uuid_id::<rim_core::channel_lens::model::ChannelLensId>(&lens_id)?;

        svc.delete_lens(rim_services::channel_lens::command::DeleteLensCmd {
            actor: Actor::new(actor),
            server_id,
            lens_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
struct CreateChannelLensInput {
    server_id: ID,
    conversation_id: ID,
    name: String,
    filter_spec: crate::types::FilterSpecInput,
    is_shared: Option<bool>,
}

#[derive(InputObject)]
struct UpdateChannelLensInput {
    server_id: ID,
    lens_id: ID,
    name: Option<String>,
    filter_spec: Option<crate::types::FilterSpecInput>,
    is_shared: Option<bool>,
}
