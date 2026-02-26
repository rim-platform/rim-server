use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
};
use crate::message_attachment::dto::{AttachmentDto, UploadTicketDto};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait MessageAttachmentService: Send + Sync {
    async fn create_attachment(&self, cmd: CreateAttachmentCmd) -> ServiceResult<UploadTicketDto>;

    async fn finalize_attachment(&self, cmd: FinalizeAttachmentCmd)
    -> ServiceResult<AttachmentDto>;

    async fn get_attachment(&self, query: GetAttachmentQuery) -> ServiceResult<AttachmentDto>;

    async fn delete_attachment(&self, cmd: DeleteAttachmentCmd) -> ServiceResult<()>;
}
