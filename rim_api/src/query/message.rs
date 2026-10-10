use async_graphql::*;
use rim_services::Actor;
use rim_services::conversation::service::ConversationService;
use rim_services::message::command::{
    GetMessageQuery, ListMessageChangesQuery, ListMessagesQuery, MessageChangeCursor,
    SearchMessagesQuery,
};
use rim_services::message::service::MessageService;

use crate::auth_context::require_actor;
use crate::ids::parse_uuid_id;
use crate::pagination::PageDir;
use crate::types::{
    ConversationMessageChanges, ConversationTypeEnum, Message, MessageChangesInput,
    MessageCursorInput, MessagePage, SearchMessagesInput, SearchResultMessage, SearchResultPage,
};

#[derive(Default)]
pub(crate) struct MessageQueries;

#[Object]
impl MessageQueries {
    #[allow(clippy::too_many_arguments)]
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
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let page = svc
            .list_messages(ListMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                dir: dir.into(),
                cursor: cursor.map(TryFrom::try_from).transpose()?,
                limit: (limit.max(1) as u32).min(100),
                include_deleted,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(MessagePage::from_page(page))
    }

    /// Full-text search over the messages of a workspace the caller can read. `input.query`
    /// accepts free text plus the operators `from:@user`, `in:#channel`, `has:link`,
    /// `has:attachment`, `after:YYYY-MM-DD` and `before:YYYY-MM-DD`.
    ///
    /// Results are cursor-paginated by recency, `(sentAt, id)`, like every other message list.
    /// They are not ordered by `rank`: a relevance score is neither unique nor stable, so paging
    /// by it would skip or repeat results. `rank` and `headline` come with every item for display
    /// or a sort within the page.
    async fn search_messages(
        &self,
        ctx: &Context<'_>,
        input: SearchMessagesInput,
    ) -> Result<SearchResultPage> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let conv_svc = ctx.data::<std::sync::Arc<dyn ConversationService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;
        let conversation_id = input
            .conversation_id
            .as_ref()
            .map(parse_uuid_id::<rim_core::conversation::model::ConversationId>)
            .transpose()?;

        let page = svc
            .search_messages(SearchMessagesQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                query: input.query,
                limit: (input.limit.unwrap_or(25).max(1) as u32).min(200),
                cursor: input.cursor.map(TryFrom::try_from).transpose()?,
                dir: input.dir.unwrap_or(PageDir::After).into(),
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        // Batch-load conversation metadata for results
        let unique_conv_ids: Vec<_> = {
            let mut seen = std::collections::HashSet::new();
            page.items
                .iter()
                .filter_map(|hit| {
                    if seen.insert(hit.message.conversation_id) {
                        Some(hit.message.conversation_id)
                    } else {
                        None
                    }
                })
                .collect()
        };

        let conversations = conv_svc
            .get_conversations_by_ids(&unique_conv_ids)
            .await
            .map_err(crate::errors::map_service_error)?;
        let conv_map: std::collections::HashMap<_, _> =
            conversations.into_iter().map(|c| (c.id, c)).collect();

        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        let items = page
            .items
            .into_iter()
            .map(|hit| {
                let dto = hit.message;
                let conv = conv_map.get(&dto.conversation_id);
                SearchResultMessage {
                    id: ID(dto.id.to_string()),
                    server_id: ID(dto.server_id.to_string()),
                    conversation_id: ID(dto.conversation_id.to_string()),
                    conversation_title: conv.and_then(|c| c.title.clone()),
                    conversation_type: conv
                        .map(|c| ConversationTypeEnum::from(c.conversation_type))
                        .unwrap_or(ConversationTypeEnum::Channel),
                    thread_id: dto.thread_id.map(|id| ID(id.to_string())),
                    root_message_id: dto.thread_root_message_id.map(|id| ID(id.to_string())),
                    author_id: ID(dto.author_id.to_string()),
                    text: dto.text,
                    sent_at: dto.sent_at,
                    rank: f64::from(hit.rank),
                    headline: hit.headline,
                }
            })
            .collect();

        Ok(SearchResultPage {
            items,
            next: page.next.map(|c| crate::types::MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: ID(c.message_id.to_string()),
            }),
            prev: page.prev.map(|c| crate::types::MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: ID(c.message_id.to_string()),
            }),
            page_info: crate::types::PageInfo { has_next, has_prev },
        })
    }

    async fn message(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
        message_id: ID,
    ) -> Result<Message> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;
        let message_id = parse_uuid_id::<rim_core::message::model::MessageId>(&message_id)?;

        let dto = svc
            .get_message(GetMessageQuery {
                actor: Actor::new(actor),
                server_id,
                conversation_id,
                message_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(dto.into())
    }

    /// The message change feed: for each conversation, the messages created, edited or deleted
    /// after its cursor (timeline and thread replies alike), oldest change first. A client cache
    /// stores each returned `cursor` and passes it back as `after`.
    async fn message_changes(
        &self,
        ctx: &Context<'_>,
        input: MessageChangesInput,
    ) -> Result<Vec<ConversationMessageChanges>> {
        let actor = require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<dyn MessageService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&input.server_id)?;

        let conversations =
            input
                .conversations
                .iter()
                .map(|c| {
                    let conversation_id = parse_uuid_id::<
                        rim_core::conversation::model::ConversationId,
                    >(&c.conversation_id)?;
                    let after_version = match c.after.as_deref() {
                        None => 0,
                        Some(after) => after.parse::<i64>().map_err(|_| {
                            crate::errors::map_service_error(
                                rim_services::ServiceError::Validation(
                                    "invalid change feed cursor".to_string(),
                                ),
                            )
                        })?,
                    };
                    Ok(MessageChangeCursor {
                        conversation_id,
                        after_version,
                    })
                })
                .collect::<Result<Vec<_>>>()?;

        let feed = svc
            .list_message_changes(ListMessageChangesQuery {
                actor: Actor::new(actor),
                server_id,
                conversations,
                limit: input.limit.unwrap_or(100).max(1) as u32,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(feed.into_iter().map(Into::into).collect())
    }
}

#[derive(Default)]
pub(crate) struct MessagePinQueries;

#[Object]
impl MessagePinQueries {
    async fn pinned_messages(
        &self,
        ctx: &Context<'_>,
        server_id: ID,
        conversation_id: ID,
    ) -> Result<Vec<Message>> {
        let actor = require_actor(ctx)?;

        let svc = ctx.data::<std::sync::Arc<dyn rim_services::pin::service::PinService>>()?;
        let server_id = parse_uuid_id::<rim_core::server::model::ServerId>(&server_id)?;
        let conversation_id =
            parse_uuid_id::<rim_core::conversation::model::ConversationId>(&conversation_id)?;

        let messages = svc
            .list_pinned_messages(rim_services::pin::command::ListPinnedMessagesQuery {
                actor: rim_services::Actor::new(actor),
                server_id,
                conversation_id,
            })
            .await
            .map_err(crate::errors::map_service_error)?;

        Ok(messages.into_iter().map(Into::into).collect())
    }
}
