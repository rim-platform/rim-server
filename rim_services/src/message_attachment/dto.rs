use rim_core::message_attachment::model::MessageAttachmentId;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AttachmentStatus {
    PendingUpload,
    Uploaded,
    Ready,
    Failed,
}

#[derive(Debug, Clone)]
pub struct AttachmentDto {
    pub id: MessageAttachmentId,
    pub status: AttachmentStatus,

    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,

    /// Storage key for downloading the file (e.g. via `/api/files/{storage_key}`).
    /// `None` when the attachment is still pending upload.
    pub storage_key: Option<String>,

    /// Timestamp extracted from the UUID v7 attachment ID.
    pub created_at: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct UploadTicketDto {
    pub attachment_id: MessageAttachmentId,

    pub upload_token: String,

    pub max_size_bytes: Option<u64>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}
