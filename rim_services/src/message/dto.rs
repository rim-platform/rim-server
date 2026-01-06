use rim_core::conversation::model::ConversationId;
use rim_core::message::model::MessageId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageDto {
    pub id: MessageId,
    pub server_id: ServerId,
    pub conversation_id: ConversationId,

    pub author_id: UserId,

    pub text: Option<String>,
    pub has_attachments: bool,
    pub attachments: Vec<MessageAttachmentDto>,

    /// Threading / replies:
    pub parent_message_id: Option<MessageId>,

    pub is_deleted: bool,

    /// Use whatever timestamp type you standardize on (chrono/time).
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub updated_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub struct MessageAttachmentDto {
    pub attachment_id: rim_core::message_attachment::model::MessageAttachmentId,

    pub filename: Option<String>,
    pub content_type: Option<String>,
    pub size_bytes: Option<u64>,
}
