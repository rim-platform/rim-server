use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::message_thread::repository::ThreadCursor;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
pub struct ThreadDto {
    pub id: MessageThreadId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub root_message_id: Option<MessageId>,
    pub root_author_id: Option<UserId>,
    pub root_text: Option<String>,

    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_message_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone, Copy)]
pub struct ThreadPageCursor {
    pub last_message_at: chrono::DateTime<chrono::Utc>,
    pub thread_id: MessageThreadId,
}

impl From<ThreadCursor> for ThreadPageCursor {
    fn from(value: ThreadCursor) -> Self {
        Self {
            last_message_at: value.last_message_at,
            thread_id: value.thread_id,
        }
    }
}

impl From<ThreadPageCursor> for ThreadCursor {
    fn from(value: ThreadPageCursor) -> Self {
        Self {
            last_message_at: value.last_message_at,
            thread_id: value.thread_id,
        }
    }
}
