use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd,
};
use crate::message_attachment::dto::{AttachmentDto, UploadTicketDto};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageAttachmentService: Send + Sync {
    async fn create_attachment(&self, cmd: CreateAttachmentCmd) -> ServiceResult<UploadTicketDto>;

    async fn finalize_attachment(&self, cmd: FinalizeAttachmentCmd)
    -> ServiceResult<AttachmentDto>;

    async fn delete_attachment(&self, cmd: DeleteAttachmentCmd) -> ServiceResult<()>;

    /// Validate an upload token, verify the attachment is pending, and mark it
    /// as uploaded with the final storage key. Called by the file upload handler
    /// after the file has been written to storage.
    async fn complete_upload(
        &self,
        cmd: super::command::CompleteUploadCmd,
    ) -> ServiceResult<AttachmentDto>;

    /// Authorize a download request: verify the user can access the attachment's
    /// conversation, then return the attachment DTO (which includes the storage key).
    async fn authorize_download(
        &self,
        query: super::command::AuthorizeDownloadQuery,
    ) -> ServiceResult<AttachmentDto>;

    /// Mark all stale `PendingUpload`/`Uploaded` attachments older than
    /// `older_than_minutes` as failed. Returns the number of attachments updated.
    async fn cleanup_stale_attachments(&self, older_than_minutes: i64) -> ServiceResult<u64>;
}
