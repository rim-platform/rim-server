use crate::message::model::MessageId;
use crate::message_attachment::model::{
    MessageAttachment, MessageAttachmentId, NewMessageAttachment,
};
use crate::repository::error::RepoResult;
use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;
use std::collections::HashMap;

#[async_trait]
#[cfg_attr(test, automock)]
pub trait MessageAttachmentRepository: Send + Sync {
    async fn get_by_id(&self, id: MessageAttachmentId) -> RepoResult<Option<MessageAttachment>>;

    async fn add_attachment(
        &self,
        attachment: NewMessageAttachment,
    ) -> RepoResult<MessageAttachment>;

    async fn get_attachments_for_message(
        &self,
        message_id: MessageId,
    ) -> RepoResult<Vec<MessageAttachment>>;

    async fn get_attachments_for_messages(
        &self,
        message_ids: &[MessageId],
    ) -> RepoResult<HashMap<MessageId, Vec<MessageAttachment>>>;

    async fn update_attachment(
        &self,
        id: MessageAttachmentId,
        storage_url: String,
        metadata: Option<serde_json::Value>,
    ) -> RepoResult<MessageAttachment>;

    async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;
}
