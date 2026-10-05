use async_graphql::{InputObject, Result, SimpleObject};
use rim_core::repository::pagination::Page;

use super::{Conversation, Message, SearchResultMessage, Thread};

#[derive(InputObject, Clone, Debug)]
pub struct InboxCursorInput {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

impl TryFrom<InboxCursorInput> for rim_services::conversation::dto::InboxPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: InboxCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            last_activity_at: value.last_activity_at,
            conversation_id: crate::ids::parse_uuid_id::<
                rim_core::conversation::model::ConversationId,
            >(&value.conversation_id)?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct ChannelCursorInput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

impl TryFrom<ChannelCursorInput> for rim_services::conversation::dto::ChannelPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: ChannelCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            created_at: value.created_at,
            conversation_id: crate::ids::parse_uuid_id::<
                rim_core::conversation::model::ConversationId,
            >(&value.conversation_id)?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct MessageCursorInput {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: async_graphql::ID,
}

impl TryFrom<MessageCursorInput> for rim_services::message::dto::MessagePageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: MessageCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            sent_at: value.sent_at,
            message_id: crate::ids::parse_uuid_id::<rim_core::message::model::MessageId>(
                &value.message_id,
            )?,
        })
    }
}

#[derive(InputObject, Clone, Debug)]
pub struct ThreadCursorInput {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: async_graphql::ID,
}

impl TryFrom<ThreadCursorInput> for rim_services::message_thread::dto::ThreadPageCursor {
    type Error = async_graphql::Error;

    fn try_from(value: ThreadCursorInput) -> Result<Self, Self::Error> {
        Ok(Self {
            last_message_at: value.last_message_at,
            thread_id: crate::ids::parse_uuid_id::<rim_core::message_thread::model::MessageThreadId>(
                &value.thread_id,
            )?,
        })
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct PageInfo {
    pub has_next: bool,
    pub has_prev: bool,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ConversationPage {
    pub items: Vec<Conversation>,
    pub next: Option<InboxCursorOutput>,
    pub prev: Option<InboxCursorOutput>,
    pub page_info: PageInfo,
}

impl ConversationPage {
    pub fn from_inbox_page(
        page: Page<
            rim_services::conversation::dto::ConversationDto,
            rim_services::conversation::dto::InboxPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| InboxCursorOutput {
                last_activity_at: c.last_activity_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            prev: page.prev.map(|c| InboxCursorOutput {
                last_activity_at: c.last_activity_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelPage {
    pub items: Vec<Conversation>,
    pub next: Option<ChannelCursorOutput>,
    pub prev: Option<ChannelCursorOutput>,
    pub page_info: PageInfo,
}

impl ChannelPage {
    pub fn from_channel_page(
        page: Page<
            rim_services::conversation::dto::ConversationDto,
            rim_services::conversation::dto::ChannelPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| ChannelCursorOutput {
                created_at: c.created_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            prev: page.prev.map(|c| ChannelCursorOutput {
                created_at: c.created_at,
                conversation_id: async_graphql::ID(c.conversation_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessagePage {
    pub items: Vec<Message>,
    pub next: Option<MessageCursorOutput>,
    pub prev: Option<MessageCursorOutput>,
    pub page_info: PageInfo,
}

impl MessagePage {
    pub fn from_page(
        page: Page<
            rim_services::message::dto::MessageDto,
            rim_services::message::dto::MessagePageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        let items = page.items.into_iter().map(Into::into).collect();
        Self {
            items,
            next: page.next.map(|c| MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: async_graphql::ID(c.message_id.to_string()),
            }),
            prev: page.prev.map(|c| MessageCursorOutput {
                sent_at: c.sent_at,
                message_id: async_graphql::ID(c.message_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

/// Cursor-paginated `searchMessages` result. Mirrors [`MessagePage`] — same `(sentAt, messageId)`
/// cursor contract — because search results are paginated by recency, not relevance; see the
/// `rank` field on [`SearchResultMessage`] and `docs/search.md` for why.
#[derive(SimpleObject, Clone, Debug)]
pub struct SearchResultPage {
    pub items: Vec<SearchResultMessage>,
    pub next: Option<MessageCursorOutput>,
    pub prev: Option<MessageCursorOutput>,
    pub page_info: PageInfo,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ThreadPage {
    pub items: Vec<Thread>,
    pub next: Option<ThreadCursorOutput>,
    pub prev: Option<ThreadCursorOutput>,
    pub page_info: PageInfo,
}

impl ThreadPage {
    pub fn from_page(
        page: Page<
            rim_services::message_thread::dto::ThreadDto,
            rim_services::message_thread::dto::ThreadPageCursor,
        >,
    ) -> Self {
        let has_next = page.next.is_some();
        let has_prev = page.prev.is_some();
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
            next: page.next.map(|c| ThreadCursorOutput {
                last_message_at: c.last_message_at,
                thread_id: async_graphql::ID(c.thread_id.to_string()),
            }),
            prev: page.prev.map(|c| ThreadCursorOutput {
                last_message_at: c.last_message_at,
                thread_id: async_graphql::ID(c.thread_id.to_string()),
            }),
            page_info: PageInfo { has_next, has_prev },
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct InboxCursorOutput {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ChannelCursorOutput {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MessageCursorOutput {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct ThreadCursorOutput {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: async_graphql::ID,
}
