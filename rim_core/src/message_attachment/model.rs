use crate::message::model::MessageId;
use crate::model_id;

model_id!(MessageAttachmentId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MessageAttachment {
    pub id: MessageAttachmentId,
    pub storage_url: String,
    pub message_id: MessageId,
    pub file_size: i64,
    pub file_name: String,
    pub mime_type: String,
    pub metadata: Option<serde_json::Value>,
    /// SHA-256 of the blob of an encrypted conversation's attachment.
    pub blob_sha256: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewMessageAttachment {
    pub id: MessageAttachmentId,
    pub storage_url: String,
    pub message_id: MessageId,
    pub file_size: i64,
    pub file_name: String,
    pub mime_type: String,
    pub metadata: Option<serde_json::Value>,
    /// SHA-256 of the blob of an encrypted conversation's attachment.
    pub blob_sha256: Option<Vec<u8>>,
}

/// Bytes and number of attachments counted against one workspace's storage quota.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AttachmentUsage {
    pub total_bytes: u64,
    pub count: u64,
}
