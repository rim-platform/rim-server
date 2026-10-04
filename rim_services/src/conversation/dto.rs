use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::{ConversationActivityCursor, ConversationCreatedCursor};
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use rim_core::conversation::model::ConversationType;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ConversationDto {
    pub id: ConversationId,
    pub server_id: ServerId,
    pub conversation_type: ConversationType,

    pub title: Option<String>,
    pub topic: Option<String>,

    pub member_ids: Vec<UserId>,
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct InboxPageCursor {
    pub last_activity_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: ConversationId,
}

impl From<ConversationActivityCursor> for InboxPageCursor {
    fn from(value: ConversationActivityCursor) -> Self {
        Self {
            last_activity_at: value.last_activity_at,
            conversation_id: value.conversation_id,
        }
    }
}

impl From<InboxPageCursor> for ConversationActivityCursor {
    fn from(value: InboxPageCursor) -> Self {
        Self {
            last_activity_at: value.last_activity_at,
            conversation_id: value.conversation_id,
        }
    }
}

#[derive(Debug, Clone, Copy)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct ChannelPageCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub conversation_id: ConversationId,
}

impl From<ConversationCreatedCursor> for ChannelPageCursor {
    fn from(value: ConversationCreatedCursor) -> Self {
        Self {
            created_at: value.created_at,
            conversation_id: value.conversation_id,
        }
    }
}

impl From<ChannelPageCursor> for ConversationCreatedCursor {
    fn from(value: ChannelPageCursor) -> Self {
        Self {
            created_at: value.created_at,
            conversation_id: value.conversation_id,
        }
    }
}
