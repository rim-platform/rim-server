use async_graphql::*;

use crate::pagination::PageDir;
use rim_services::conversation::command::{
    GetConversationQuery, ListChannelsQuery, ListInboxConversationsQuery,
};
use crate::types::{
    ChannelCursorInput, ChannelPage, Conversation, ConversationPage, InboxCursorInput, Message,
    MessageCursorInput, MessagePage, Thread, ThreadCursorInput, ThreadPage,
};
use rim_services::conversation::service::ConversationService;
use rim_services::message::command::{GetMessageQuery, ListMessagesQuery};
use rim_services::message::service::MessageService;
use rim_services::message_thread::command::{GetThreadQuery, ListThreadsQuery};
use rim_services::message_thread::service::ThreadService;
use rim_services::Actor;

use crate::ids::parse_uuid_id;

#[derive(Default)]
pub struct AuthActorCtx {
    actor: Option<rim_core::user::model::UserId>,
    session_id: Option<rim_core::auth::model::SessionId>,
}

impl AuthActorCtx {
    pub fn new(
        actor: Option<rim_core::user::model::UserId>,
        session_id: Option<rim_core::auth::model::SessionId>,
    ) -> Self {
        Self { actor, session_id }
    }

    pub fn actor(&self) -> Option<rim_core::user::model::UserId> {
        self.actor
    }

    pub fn session_id(&self) -> Option<rim_core::auth::model::SessionId> {
        self.session_id
    }

    // Cookie/CSRF signaling is handled via GraphQL response extensions.
}

pub struct Query;

#[Object]
impl Query {
    async fn hello_world(&self, _ctx: &Context<'_>) -> String {
        "Hello world!".to_string()
    }

    async fn conversation(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Conversation> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;

        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let conv = svc
            .get_conversation(GetConversationQuery {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

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
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_inbox_conversations(ListInboxConversationsQuery {
            actor: Actor::new(actor),
            server_id,
            dir: dir.into(),
            cursor: cursor.map(TryFrom::try_from).transpose()?,
            limit: limit.max(1) as u32,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

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
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;

        let page = svc
            .list_channels(ListChannelsQuery {
            actor: Actor::new(actor),
            server_id,
            dir: dir.into(),
            cursor: cursor.map(TryFrom::try_from).transpose()?,
            limit: limit.max(1) as u32,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

        Ok(ChannelPage::from_channel_page(page))
    }

    async fn messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        dir: PageDir,
        cursor: Option<MessageCursorInput>,
        limit: i32,
        include_deleted: bool,
    ) -> Result<MessagePage> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_messages(ListMessagesQuery {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            dir: dir.into(),
            cursor: cursor.map(TryFrom::try_from).transpose()?,
            limit: limit.max(1) as u32,
            include_deleted,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

        Ok(MessagePage::from_page(page))
    }

    async fn message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        message_id: ID,
    ) -> Result<Message> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&message_id)?;

        let msg = svc
            .get_message(GetMessageQuery {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            message_id,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

        Ok(msg.into())
    }

    async fn thread(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        thread_id: ID,
    ) -> Result<Thread> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let thread_id = parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&thread_id)?;

        let t = svc
            .get_thread(GetThreadQuery {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            thread_id,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

        Ok(t.into())
    }

    async fn threads(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        dir: PageDir,
        cursor: Option<ThreadCursorInput>,
        limit: i32,
    ) -> Result<ThreadPage> {
        let actor = ctx
            .data_unchecked::<AuthActorCtx>()
            .actor()
            .ok_or_else(|| Error::new("unauthenticated"))?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id = parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_threads(ListThreadsQuery {
            actor: Actor::new(actor),
            server_id,
            conversation_id,
            dir: dir.into(),
            cursor: cursor.map(TryFrom::try_from).transpose()?,
            limit: limit.max(1) as u32,
        })
            .await
            .map_err(crate::mutation::map_service_error)?;

        Ok(ThreadPage::from_page(page))
    }
}
