use async_trait::async_trait;
use crate::message::model::MessageId;
use crate::message_attachment::model::{MessageAttachment, MessageAttachmentId};
use crate::repository::repository::Repository;

#[async_trait]
pub trait MessageAttachmentRepository<Model = MessageAttachment, Id = MessageAttachmentId>: Repository
{
    async fn add_attachment(
        &self,
        message_id: MessageId,
        storage_url: &str,
        file_name: &str,
        file_size: i64,
        mime_type: &str,
        metadata: Option<serde_json::Value>,
    ) -> anyhow::Result<Self::Model>;

    async fn get_attachments_for_message(
        &self,
        message_id: MessageId,
    ) -> anyhow::Result<Vec<Self::Model>>;
}
