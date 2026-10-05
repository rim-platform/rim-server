use async_graphql::*;
use rim_services::Actor;
use rim_services::message::command::{
    DeleteMessageCmd, EditMessageCmd, ReplyToMessageCmd, SendMessageCmd,
};
use rim_services::message::service::MessageService;
use rim_services::message_reaction::command::ToggleReactionCmd;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::pin::command::{PinMessageCmd, UnpinMessageCmd};
use rim_services::pin::service::PinService;

use crate::auth_context::require_actor;
use crate::errors::map_service_error;
use crate::ids::parse_uuid_id;
use crate::types::{Message, ReactionGroup};

#[derive(Default)]
pub(crate) struct MessageMutations;

#[Object]
impl MessageMutations {
    async fn send_message(&self, ctx: &Context<'_>, input: SendMessageInput) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let parent_message_id = match input.parent_message_id {
            None => None,
            Some(id) => Some(parse_uuid_id::<rim_core::message::model::MessageId>(&id)?),
        };

        let text_for_preview = input.text.clone();
        let msg = svc
            .send_message(SendMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                text: Some(input.text),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

        // Spawn background link preview fetch
        spawn_link_preview_fetch(ctx, msg.id, server_id, conversation_id, &text_for_preview)?;

        Ok(msg.into())
    }

    async fn reply_to_message(
        &self,
        ctx: &Context<'_>,
        input: ReplyToMessageInput,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let root_message_id =
            parse_uuid_id::<rim_core::message::model::MessageId>(&input.root_message_id)?;
        let parent_message_id = input
            .parent_message_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::message::model::MessageId>)
            .transpose()?;

        let text_for_preview = input.text.clone();
        let msg = svc
            .reply_to_message(ReplyToMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
                text: text_for_preview.clone(),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(map_service_error)?;

        // Spawn background link preview fetch
        spawn_link_preview_fetch(ctx, msg.id, server_id, conversation_id, &text_for_preview)?;

        Ok(msg.into())
    }
}

#[derive(Default)]
pub(crate) struct MessageEditMutations;

#[Object]
impl MessageEditMutations {
    async fn edit_message(&self, ctx: &Context<'_>, input: EditMessageInput) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let msg = svc
            .edit_message(EditMessageCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                text: input.text,
            })
            .await
            .map_err(map_service_error)?;

        Ok(msg.into())
    }

    async fn forward_message(
        &self,
        ctx: &Context<'_>,
        input: ForwardMessageInput,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let source_conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(
            &input.source_conversation_id,
        )?;
        let target_conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(
            &input.target_conversation_id,
        )?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let msg = svc
            .forward_message(rim_services::message::command::ForwardMessageCmd {
                actor: Actor::new(actor),
                server_id,
                source_conversation_id,
                target_conversation_id,
                message_id,
            })
            .await
            .map_err(map_service_error)?;

        Ok(msg.into())
    }

    async fn delete_message(&self, ctx: &Context<'_>, input: DeleteMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.delete_message(DeleteMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
            hard: false,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(Default)]
pub(crate) struct MessageReactionMutations;

#[Object]
impl MessageReactionMutations {
    async fn toggle_reaction(
        &self,
        ctx: &Context<'_>,
        input: ToggleReactionInput,
    ) -> Result<Vec<ReactionGroup>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn MessageReactionService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        let groups = svc
            .toggle_reaction(ToggleReactionCmd {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
                emoji: input.emoji,
            })
            .await
            .map_err(map_service_error)?;

        Ok(groups.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub(crate) struct MessagePinMutations;

#[Object]
impl MessagePinMutations {
    async fn pin_message(&self, ctx: &Context<'_>, input: PinMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.pin_message(PinMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }

    async fn unpin_message(&self, ctx: &Context<'_>, input: UnpinMessageInput) -> Result<bool> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&input.conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&input.message_id)?;

        svc.unpin_message(UnpinMessageCmd {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
        })
        .await
        .map_err(map_service_error)?;

        Ok(true)
    }
}

#[derive(InputObject)]
pub struct SendMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub text: String,
    pub parent_message_id: Option<ID>,
}

#[derive(InputObject)]
pub struct ReplyToMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub root_message_id: ID,
    pub text: String,
    pub parent_message_id: Option<ID>,
}

#[derive(InputObject)]
pub struct EditMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub text: String,
}

#[derive(InputObject)]
pub struct ForwardMessageInput {
    pub server_id: ID,
    pub source_conversation_id: ID,
    pub target_conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct DeleteMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct PinMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct UnpinMessageInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
}

#[derive(InputObject)]
pub struct ToggleReactionInput {
    pub server_id: ID,
    pub conversation_id: ID,
    pub message_id: ID,
    pub emoji: String,
}

fn spawn_link_preview_fetch(
    ctx: &Context<'_>,
    message_id: rim_core::message::model::MessageId,
    workspace_id: rim_core::server::model::ServerId,
    conversation_id: rim_core::conversation::model::ConversationId,
    text: &str,
) -> Result<()> {
    let urls = rim_services::link_preview::extract_urls(text);
    if urls.is_empty() {
        return Ok(());
    }
    let link_svc = ctx
        .data::<std::sync::Arc<dyn rim_services::link_preview::LinkPreviewService>>()?
        .clone();
    let observers = ctx
        .data::<std::sync::Arc<rim_services::events::Observers>>()?
        .clone();

    tokio::spawn(async move {
        match link_svc.fetch_and_store(message_id, urls).await {
            Ok(previews) if !previews.is_empty() => {
                use rim_services::events::EventObserver;
                observers.emit(rim_services::events::WorkspaceEvent::LinkPreviewReady {
                    workspace_id,
                    conversation_id,
                    message_id,
                });
            }
            Ok(_) => {}
            Err(e) => {
                tracing::warn!(error = %e, "link preview fetch failed");
            }
        }
    });

    Ok(())
}
