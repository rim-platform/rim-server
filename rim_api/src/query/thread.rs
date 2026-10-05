use async_graphql::*;
use rim_services::Actor;
use rim_services::message::command::ListThreadMessagesQuery;
use rim_services::message::service::MessageService;
use rim_services::message_thread::command::{
    GetThreadByRootMessageQuery, GetThreadQuery, ListThreadsQuery,
};
use rim_services::message_thread::service::ThreadService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{MessageCursorInput, MessagePage, Thread, ThreadCursorInput, ThreadPage};

#[derive(Default)]
pub(crate) struct ThreadQueries;

#[Object]
impl ThreadQueries {
    async fn thread(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        thread_id: ID,
    ) -> Result<Thread> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let thread_id =
            parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&thread_id)?;

        let t = svc
            .get_thread(GetThreadQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                thread_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(t.into())
    }

    async fn thread_by_root_message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        root_message_id: ID,
    ) -> Result<Option<Thread>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let root_message_id =
            parse_uuid_id::<rim_core::message::model::MessageId>(&root_message_id)?;

        let t = svc
            .get_thread_by_root_message(GetThreadByRootMessageQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                root_message_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(t.map(Into::into))
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
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn ThreadService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_threads(ListThreadsQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(ThreadPage::from_page(page))
    }

    #[allow(clippy::too_many_arguments)]
    async fn thread_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        thread_id: ID,
        dir: PageDir,
        cursor: Option<MessageCursorInput>,
        limit: i32,
        include_deleted: bool,
    ) -> Result<MessagePage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let thread_id =
            parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(&thread_id)?;

        let page = svc
            .list_thread_messages(ListThreadMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                thread_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page))
    }
}
