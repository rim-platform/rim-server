use async_trait::async_trait;

use rim_services::message_attachment::command::{
    CreateAttachmentCmd, DeleteAttachmentCmd, FinalizeAttachmentCmd, GetAttachmentQuery,
};
use rim_services::message_attachment::dto::{AttachmentDto, AttachmentStatus, UploadTicketDto};
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::{ServiceError, ServiceResult};

use crate::AttachmentRepos;
use crate::util::ensure_server_member;

#[cfg(test)]
use crate::test_support::attachment::{
    AttachmentReposPorts, MessageAttachmentRepoPortAdapter, ServerMemberRepoPortAdapter,
};

#[derive(Clone)]
pub struct DefaultMessageAttachmentService {
    repos: AttachmentRepos,
}

impl DefaultMessageAttachmentService {
    pub fn new(repos: AttachmentRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: AttachmentReposPorts) -> Self {
        Self::new(AttachmentRepos {
            message_attachment_repo: std::sync::Arc::new(MessageAttachmentRepoPortAdapter(
                repos.message_attachment_repo,
            )),
            server_member_repo: std::sync::Arc::new(ServerMemberRepoPortAdapter(
                repos.server_member_repo,
            )),
        })
    }
}

#[async_trait]
impl MessageAttachmentService for DefaultMessageAttachmentService {
    async fn create_attachment(&self, _cmd: CreateAttachmentCmd) -> ServiceResult<UploadTicketDto> {
        Err(ServiceError::Validation(
            "attachments are not supported in v0".to_string(),
        ))
    }

    async fn finalize_attachment(
        &self,
        _cmd: FinalizeAttachmentCmd,
    ) -> ServiceResult<AttachmentDto> {
        Err(ServiceError::Validation(
            "attachments are not supported in v0".to_string(),
        ))
    }

    async fn get_attachment(&self, query: GetAttachmentQuery) -> ServiceResult<AttachmentDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let att = self
            .repos
            .message_attachment_repo
            .get_by_id(query.attachment_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        Ok(AttachmentDto {
            id: att.id,
            status: AttachmentStatus::Ready,
            filename: att.file_name,
            content_type: att.mime_type,
            size_bytes: att.file_size.max(0) as u64,
        })
    }

    async fn delete_attachment(&self, _cmd: DeleteAttachmentCmd) -> ServiceResult<()> {
        Err(ServiceError::Validation(
            "attachments are not supported in v0".to_string(),
        ))
    }
}
