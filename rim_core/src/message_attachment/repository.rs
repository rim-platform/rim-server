use crate::message::model::MessageId;
use crate::message_attachment::model::{MessageAttachment, MessageAttachmentId};
use crate::repository::error::RepoResult;
use async_trait::async_trait;

#[async_trait]
pub trait MessageAttachmentRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageAttachmentId) -> RepoResult<Option<MessageAttachment>>;

    async fn add_attachment(
        &self,
        message_id: MessageId,
        storage_url: &str,
        file_name: &str,
        file_size: i64,
        mime_type: &str,
        metadata: Option<serde_json::Value>,
    ) -> RepoResult<MessageAttachment>;

    async fn get_attachments_for_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Vec<MessageAttachment>>;
}
