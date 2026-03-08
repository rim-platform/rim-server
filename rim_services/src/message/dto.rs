use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message::repository::MessageCursor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageDto {
    pub id: MessageId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,

    pub author_id: UserId,

    pub text: Option<String>,
    pub has_attachments: bool,

    /// Threading / replies:
    pub parent_message_id: Option<MessageId>,

    pub is_deleted: bool,

    pub sent_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessagePageCursor {
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub message_id: MessageId,
}

impl From<MessageCursor> for MessagePageCursor {
    fn from(value: MessageCursor) -> Self {
        Self {
            sent_at: value.sent_at,
            message_id: value.message_id,
        }
    }
}

impl From<MessagePageCursor> for MessageCursor {
    fn from(value: MessagePageCursor) -> Self {
        Self {
            sent_at: value.sent_at,
            message_id: value.message_id,
        }
    }
}
