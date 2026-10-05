use async_graphql::*;
use chrono::{DateTime, Utc};
use rim_services::Actor;
use rim_services::conversation::command::{CreateConversationCmd, UpdateConversationTopicCmd};
use rim_services::conversation::service::ConversationService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::Conversation;

#[derive(Default)]
pub(crate) struct ConversationMutations;

#[Object]
impl ConversationMutations {
    async fn create_conversation(
        &self,
        ctx: &Context<'_>,
        input: CreateConversationInput,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let participants = if input.is_channel.unwrap_or(false) {
            rim_services::conversation::types::ConversationParticipants::Channel
        } else if let Some(other_user_id) = input.other_user_id {
            rim_services::conversation::types::ConversationParticipants::DirectMessage {
                other_user_id: parse_uuid_id::<rim_core::user::model::UserId>(&other_user_id)?,
            }
        } else {
            let member_ids = input.member_ids.unwrap_or_default();
            let mut ids = Vec::with_capacity(member_ids.len());
            for id in member_ids {
                ids.push(parse_uuid_id::<rim_core::user::model::UserId>(&id)?);
            }
            rim_services::conversation::types::ConversationParticipants::Group { member_ids: ids }
        };

        let conv = svc
            .create_conversation(CreateConversationCmd {
                actor: Actor::new(actor),
                server_id,
                participants,
                title: input.title,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }

    async fn update_conversation_topic(
        &self,
        ctx: &Context<'_>,
        input: UpdateConversationTopicInput,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        let conv = svc
            .update_topic(UpdateConversationTopicCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                topic: input.topic,
            })
            .await
            .map_err(map_service_error)?;

        Ok(conv.into())
    }
}

#[derive(Default)]
pub(crate) struct ReadStateMutations;

#[Object]
impl ReadStateMutations {
    async fn mark_as_read(&self, ctx: &Context<'_>, input: MarkAsReadInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.mark_as_read(rim_services::read_state::dto::MarkAsReadCmd {
            actor: Actor::new(actor),
            workspace_id,
            conversation_id,
            read_at: input.read_at,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn mark_all_as_read(&self, ctx: &Context<'_>, input: MarkAllAsReadInput) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.workspace_id)?;

        svc.mark_all_as_read(rim_services::read_state::dto::MarkAllAsReadCmd {
            actor: Actor::new(actor),
            workspace_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(Default)]
pub(crate) struct ConversationMuteMutations;

#[Object]
impl ConversationMuteMutations {
    // ── Mute / Unmute ────────────────────────────────────────────────────

    async fn mute_conversation(
        &self,
        ctx: &Context<'_>,
        input: MuteConversationInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.mute_conversation(
            rim_services::notification_preference::command::MuteConversationCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                muted_until: input.muted_until,
            },
        )
        .await
        .map_err(map_service_error)
    }

    async fn unmute_conversation(
        &self,
        ctx: &Context<'_>,
        input: UnmuteConversationInput,
    ) -> Result<bool> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;

        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;

        svc.unmute_conversation(
            rim_services::notification_preference::command::UnmuteConversationCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            },
        )
        .await
        .map_err(map_service_error)
    }
}

#[derive(InputObject)]
pub struct CreateConversationInput {
    pub server_id: ID,

    /// For DM conversations.
    pub other_user_id: Option<ID>,

    /// For Group conversations.
    pub member_ids: Option<Vec<ID>>,

    /// For Channel conversations.
    pub is_channel: Option<bool>,

    pub title: Option<String>,
}

#[derive(InputObject)]
pub struct UpdateConversationTopicInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub topic: Option<String>,
}

#[derive(InputObject)]
pub struct MarkAsReadInput {
    pub workspace_id: ID,
    pub conversation_id: ID,
    pub read_at: DateTime<Utc>,
}

#[derive(InputObject)]
pub struct MarkAllAsReadInput {
    pub workspace_id: ID,
}

#[derive(InputObject)]
struct MuteConversationInput {
    server_id: ID,
    conversation_id: ID,
    muted_until: Option<DateTime<Utc>>,
}

#[derive(InputObject)]
struct UnmuteConversationInput {
    server_id: ID,
    conversation_id: ID,
}
