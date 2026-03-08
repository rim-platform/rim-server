use async_graphql::{InputObject, SimpleObject};

use rim_core::repository::pagination::Page;

#[derive(SimpleObject, Clone, Debug)]
pub struct User {
    pub id: async_graphql::ID,
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Conversation {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_type: String,
    pub title: Option<String>,
    pub member_ids: Vec<async_graphql::ID>,
}

impl From<rim_services::conversation::dto::ConversationDto> for Conversation {
    fn from(value: rim_services::conversation::dto::ConversationDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_type: format!("{:?}", value.conversation_type),
            title: value.title,
            member_ids: value
                .member_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Message {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub thread_id: Option<async_graphql::ID>,
    pub root_thread_id: Option<async_graphql::ID>,
    pub author_id: async_graphql::ID,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub parent_message_id: Option<async_graphql::ID>,
    pub is_deleted: bool,
    pub sent_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::message::dto::MessageDto> for Message {
    fn from(value: rim_services::message::dto::MessageDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            thread_id: value.thread_id.map(|id| async_graphql::ID(id.to_string())),
            root_thread_id: value
                .root_thread_id
                .map(|id| async_graphql::ID(id.to_string())),
            author_id: async_graphql::ID(value.author_id.to_string()),
            text: value.text,
            has_attachments: value.has_attachments,
            parent_message_id: value
                .parent_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            is_deleted: value.is_deleted,
            sent_at: value.sent_at,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Thread {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_id: async_graphql::ID,
    pub root_message_id: Option<async_graphql::ID>,
    pub root_author_id: Option<async_graphql::ID>,
    pub root_text: Option<String>,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::message_thread::dto::ThreadDto> for Thread {
    fn from(value: rim_services::message_thread::dto::ThreadDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            root_message_id: value
                .root_message_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_author_id: value
                .root_author_id
                .map(|id| async_graphql::ID(id.to_string())),
            root_text: value.root_text,
            created_at: value.created_at,
            last_message_at: value.last_message_at,
        }
    }
}

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
        Self {
            items: page.items.into_iter().map(Into::into).collect(),
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
