use async_graphql::{Enum, SimpleObject};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum AttachmentStatusEnum {
    PendingUpload,
    Uploaded,
    Ready,
    Failed,
}

impl From<rim_services::message_attachment::dto::AttachmentStatus> for AttachmentStatusEnum {
    fn from(value: rim_services::message_attachment::dto::AttachmentStatus) -> Self {
        use rim_services::message_attachment::dto::AttachmentStatus as D;
        match value {
            D::PendingUpload => Self::PendingUpload,
            D::Uploaded => Self::Uploaded,
            D::Ready => Self::Ready,
            D::Failed => Self::Failed,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct Attachment {
    pub id: async_graphql::ID,
    pub status: AttachmentStatusEnum,
    pub filename: String,
    pub content_type: String,
    pub size_bytes: u64,
}

impl From<rim_services::message_attachment::dto::AttachmentDto> for Attachment {
    fn from(value: rim_services::message_attachment::dto::AttachmentDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            status: value.status.into(),
            filename: value.filename,
            content_type: value.content_type,
            size_bytes: value.size_bytes,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct UploadTicket {
    pub attachment_id: async_graphql::ID,
    pub upload_token: String,
    pub max_size_bytes: Option<u64>,
    pub expires_at: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::message_attachment::dto::UploadTicketDto> for UploadTicket {
    fn from(value: rim_services::message_attachment::dto::UploadTicketDto) -> Self {
        Self {
            attachment_id: async_graphql::ID(value.attachment_id.to_string()),
            upload_token: value.upload_token,
            max_size_bytes: value.max_size_bytes,
            expires_at: value.expires_at,
        }
    }
}
