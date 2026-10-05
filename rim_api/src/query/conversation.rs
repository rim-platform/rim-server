use async_graphql::*;
use rim_services::Actor;
use rim_services::conversation::command::{
    GetConversationQuery, ListChannelsQuery, ListInboxConversationsQuery,
};
use rim_services::conversation::service::ConversationService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    ChannelCursorInput, ChannelPage, Conversation, ConversationPage, InboxCursorInput,
    MutedConversation, UnreadCount,
};

#[derive(Default)]
pub(crate) struct ConversationQueries;

#[Object]
impl ConversationQueries {
    async fn conversation(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Conversation> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let conv = svc
            .get_conversation(GetConversationQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(conv.into())
    }

    async fn inbox_conversations(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: PageDir,
        cursor: Option<InboxCursorInput>,
        limit: i32,
    ) -> Result<ConversationPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_inbox_conversations(ListInboxConversationsQuery {
                actor: Actor::new(actor),
                server_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ConversationPage::from_inbox_page(page))
    }

    async fn channels(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        dir: PageDir,
        cursor: Option<ChannelCursorInput>,
        limit: i32,
    ) -> Result<ChannelPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_channels(ListChannelsQuery {
                actor: Actor::new(actor),
                server_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ChannelPage::from_channel_page(page))
    }
}

#[derive(Default)]
pub(crate) struct ReadStateQueries;

#[Object]
impl ReadStateQueries {
    async fn unread_counts(&self, ctx: &Context<'_>, workspace_id: ID) -> Result<Vec<UnreadCount>> {
        let actor = require_actor(ctx)?;
        let svc =
            ctx.data::<std::sync::Arc<dyn rim_services::read_state::service::ReadStateService>>()?;
        let workspace_id = parse_uuid_id::<rim_core::server::model::ServerId>(&workspace_id)?;

        let counts = svc
            .get_unread_counts(rim_services::read_state::dto::UnreadCountsQuery {
                actor: Actor::new(actor),
                workspace_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(counts.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub(crate) struct ConversationMuteQueries;

#[Object]
impl ConversationMuteQueries {
    async fn muted_conversations(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
    ) -> Result<Vec<MutedConversation>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let items = svc
            .list_muted_conversations(
                rim_services::notification_preference::command::ListMutedConversationsQuery {
                    actor: rim_services::Actor::new(actor),
                    server_id,
                },
            )
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(items.into_iter().map(Into::into).collect())
    }
}
