use crate::message::model::MessageId;
use crate::message_attachment::model::{
    AttachmentUsage, MessageAttachment, MessageAttachmentId, NewMessageAttachment,
};
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use std::collections::HashMap;

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
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

    /// Atomically completes a pending upload: sets `storage_url`, `file_size` and `metadata`
    /// only while the row's `storage_url` still equals `expected_storage_url` and the
    /// attachment was created at or after `reserved_since`.
    ///
    /// Returns `None` when another request completed (or removed) the attachment first, or the
    /// reservation has expired.
    async fn try_complete_upload(
        &self,
        id: MessageAttachmentId,
        expected_storage_url: String,
        reserved_since: chrono::DateTime<chrono::Utc>,
        storage_url: String,
        file_size: i64,
        metadata: Option<serde_json::Value>,
    ) -> RepoResult<Option<MessageAttachment>>;

    async fn delete_attachment(&self, id: MessageAttachmentId) -> RepoResult<()>;

    /// Sum of `file_size` and count over the attachments in `server_id`'s conversations that
    /// hold storage: every stored file, plus pending uploads created at or after
    /// `pending_since` (their declared size is reserved). Each size is clamped to
    /// `0..=max_file_size`, so rows written before sizes were capped can't overflow the sum.
    async fn usage_for_server(
        &self,
        server_id: ServerId,
        pending_since: chrono::DateTime<chrono::Utc>,
        max_file_size: i64,
    ) -> RepoResult<AttachmentUsage>;

    /// Find all attachment IDs with the given metadata status strings
    /// whose UUID v7 id encodes a timestamp older than `before`.
    /// Used by the stale-attachment cleanup job.
    async fn find_ids_with_status_before(
        &self,
        statuses: Vec<String>,
        before: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<Vec<MessageAttachmentId>>;
}
