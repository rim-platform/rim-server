use crate::message::model::MessageId;
use crate::model_id;

model_id!(MessageAttachmentId);

pub struct MessageAttachment {
    pub id: MessageAttachmentId,
    pub storage_url: String,
    pub message_id: MessageId,
    pub file_size: i64,
    pub file_name: String,
    pub mime_type: String,
    pub metadata: Option<serde_json::Value>,
}

pub struct NewMessageAttachment {
    pub storage_url: String,
    pub message_id: MessageId,
    pub file_size: i64,
    pub file_name: String,
    pub mime_type: String,
    pub metadata: Option<serde_json::Value>,
}
