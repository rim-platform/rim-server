use serde::{Deserialize, Serialize};

use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

use crate::message::dto::MessageDto;

/// Rich event types with full data for real-time updates.
/// Clients receive these directly — no need to re-fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkspaceEvent {
    NewMessage {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    },
    MessageUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
        message: MessageEventData,
    },
    MessageDeleted {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
    },
    ReactionsUpdated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
        message_id: MessageId,
        reactions: Vec<ReactionGroupData>,
    },
    ConversationCreated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        title: Option<String>,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
    },
    MemberJoined {
        workspace_id: ServerId,
        user_id: UserId,
        username: String,
    },
    ThreadCreated {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        thread_id: MessageThreadId,
        root_message_id: MessageId,
        conversation_type: String,
        member_user_ids: Vec<UserId>,
    },
    MemberAddedToConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
    },
    MemberRemovedFromConversation {
        workspace_id: ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
    },
}

/// Event payload for message-related events.
///
/// **Frontend note:** all fields listed here are sent over the WebSocket transport.
/// Ensure the client maps every field — in particular `server_id`, `thread_id`,
/// `root_thread_id`, `has_attachments`, and `parent_message_id` which are easy to
/// overlook when converting to the client-side cache model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEventData {
    pub id: MessageId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,
    /// Thread this message belongs to, if it is a thread reply.
    pub thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    /// Thread rooted at this message, if one exists (only set for root messages).
    pub root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
    pub author_id: UserId,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub parent_message_id: Option<MessageId>,
    pub is_deleted: bool,
    pub sent_at: chrono::DateTime<chrono::Utc>,
    pub reactions: Vec<ReactionGroupData>,
}

impl From<&MessageDto> for MessageEventData {
    fn from(dto: &MessageDto) -> Self {
        Self {
            id: dto.id,
            server_id: dto.server_id,
            conversation_id: dto.conversation_id,
            thread_id: dto.thread_id,
            root_thread_id: dto.root_thread_id,
            author_id: dto.author_id,
            text: dto.text.clone(),
            has_attachments: dto.has_attachments,
            parent_message_id: dto.parent_message_id,
            is_deleted: dto.is_deleted,
            sent_at: dto.sent_at,
            reactions: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionGroupData {
    pub emoji: String,
    pub count: u32,
    pub user_ids: Vec<UserId>,
}

impl WorkspaceEvent {
    /// Returns the workspace (server) ID as a string for subscription filtering.
    pub fn workspace_id(&self) -> String {
        match self {
            Self::NewMessage { workspace_id, .. }
            | Self::MessageUpdated { workspace_id, .. }
            | Self::MessageDeleted { workspace_id, .. }
            | Self::ReactionsUpdated { workspace_id, .. }
            | Self::ConversationCreated { workspace_id, .. }
            | Self::ThreadCreated { workspace_id, .. }
            | Self::MemberJoined { workspace_id, .. }
            | Self::MemberAddedToConversation { workspace_id, .. }
            | Self::MemberRemovedFromConversation { workspace_id, .. } => workspace_id.to_string(),
        }
    }

    /// Returns the member user IDs for visibility filtering, if applicable.
    pub fn member_user_ids(&self) -> Option<&[UserId]> {
        match self {
            Self::NewMessage {
                member_user_ids, ..
            }
            | Self::MessageUpdated {
                member_user_ids, ..
            }
            | Self::MessageDeleted {
                member_user_ids, ..
            }
            | Self::ReactionsUpdated {
                member_user_ids, ..
            }
            | Self::ConversationCreated {
                member_user_ids, ..
            }
            | Self::ThreadCreated {
                member_user_ids, ..
            } => Some(member_user_ids),
            Self::MemberJoined { .. }
            | Self::MemberAddedToConversation { .. }
            | Self::MemberRemovedFromConversation { .. } => None,
        }
    }
}
