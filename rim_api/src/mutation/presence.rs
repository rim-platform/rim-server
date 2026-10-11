use async_graphql::*;
use rim_services::Actor;
use rim_services::conversation::service::ConversationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::PresenceStatus;

#[derive(Default)]
pub(crate) struct PresenceMutations;

#[Object]
impl PresenceMutations {
    async fn start_typing(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        crate::auth_context::require_server_member(ctx, actor, server_id).await?;

        let typing_tracker =
            ctx.data::<std::sync::Arc<dyn rim_services::typing::TypingService>>()?;

        if typing_tracker.should_broadcast(actor, conversation_id) {
            let conv_svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
            let conversation = conv_svc
                .get_writable_conversation(
                    rim_services::conversation::command::GetConversationQuery {
                        actor: Actor::new(actor),
                        server_id,
                        conversation_id,
                    },
                )
                .await
                .map_err(map_service_error)?;
            let conversation_type = conversation.conversation_type.as_str().to_owned();
            let is_private = conversation.is_private;
            let member_user_ids = conversation.member_ids;

            let observers = ctx.data::<std::sync::Arc<rim_services::events::Observers>>()?;
            use rim_services::events::EventObserver;
            observers.emit(rim_services::events::WorkspaceEvent::TypingStarted {
                workspace_id: server_id,
                conversation_id,
                user_id: actor,
                conversation_type,
                member_user_ids,
                is_private,
            });
        }

        Ok(true)
    }

    async fn heartbeat(&self, ctx: &Context<'_>, input: HeartbeatInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let status: rim_services::presence::PresenceStatus = input.status.into();
        if status == rim_services::presence::PresenceStatus::Offline {
            return Err(map_service_error(rim_services::ServiceError::Validation(
                "Clients cannot set OFFLINE status".to_string(),
            )));
        }

        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;

        crate::auth_context::require_server_member(ctx, actor, workspace_id).await?;

        let presence_mgr =
            ctx.data::<std::sync::Arc<dyn rim_services::presence::PresenceService>>()?;
        presence_mgr.heartbeat(actor, workspace_id, status);

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct HeartbeatInput {
    pub workspace_id: ID,
    pub status: PresenceStatus,
}
