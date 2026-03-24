use serde::{Deserialize, Serialize};

/// Rich event types with full data for real-time updates.
/// Clients receive these directly — no need to re-fetch.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkspaceEvent {
    NewMessage {
        workspace_id: String,
        conversation_id: String,
        conversation_type: String,
        member_user_ids: Vec<String>,
        message: MessageEventData,
    },
    MessageUpdated {
        workspace_id: String,
        conversation_id: String,
        conversation_type: String,
        member_user_ids: Vec<String>,
        message: MessageEventData,
    },
    MessageDeleted {
        workspace_id: String,
        conversation_id: String,
        conversation_type: String,
        member_user_ids: Vec<String>,
        message_id: String,
    },
    ReactionsUpdated {
        workspace_id: String,
        conversation_id: String,
        conversation_type: String,
        member_user_ids: Vec<String>,
        message_id: String,
        reactions: Vec<ReactionGroupData>,
    },
    ConversationCreated {
        workspace_id: String,
        conversation_id: String,
        title: Option<String>,
        conversation_type: String,
        member_user_ids: Vec<String>,
    },
    MemberJoined {
        workspace_id: String,
        user_id: String,
        username: String,
    },
    MemberAddedToConversation {
        workspace_id: String,
        conversation_id: String,
        user_id: String,
    },
    MemberRemovedFromConversation {
        workspace_id: String,
        conversation_id: String,
        user_id: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MessageEventData {
    pub id: String,
    pub server_id: String,
    pub conversation_id: String,
    pub thread_id: Option<String>,
    pub root_thread_id: Option<String>,
    pub author_id: String,
    pub text: Option<String>,
    pub has_attachments: bool,
    pub parent_message_id: Option<String>,
    pub is_deleted: bool,
    pub sent_at: String,
    pub reactions: Vec<ReactionGroupData>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReactionGroupData {
    pub emoji: String,
    pub count: u32,
}

impl WorkspaceEvent {
    pub fn workspace_id(&self) -> &str {
        match self {
            Self::NewMessage { workspace_id, .. }
            | Self::MessageUpdated { workspace_id, .. }
            | Self::MessageDeleted { workspace_id, .. }
            | Self::ReactionsUpdated { workspace_id, .. }
            | Self::ConversationCreated { workspace_id, .. }
            | Self::MemberJoined { workspace_id, .. }
            | Self::MemberAddedToConversation { workspace_id, .. }
            | Self::MemberRemovedFromConversation { workspace_id, .. } => workspace_id,
        }
    }
}
