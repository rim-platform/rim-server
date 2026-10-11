use async_trait::async_trait;

use rim_services::message_thread::command::{GetThreadByRootMessageQuery, GetThreadQuery};
use rim_services::message_thread::dto::ThreadDto;
use rim_services::message_thread::service::ThreadService;
use rim_services::{ServiceError, ServiceResult};

use crate::ThreadRepos;
use crate::util::{ConversationAccessRepos, ensure_conversation_member_access};

#[derive(Clone)]
pub struct DefaultThreadService {
    repos: ThreadRepos,
}

impl DefaultThreadService {
    pub fn new(repos: ThreadRepos) -> Self {
        Self { repos }
    }

    async fn to_dto(
        &self,
        thread: rim_core::message_thread::model::MessageThread,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<ThreadDto> {
        let mut root_author_id = None;
        let mut root_text = None;

        if let Some(root_message_id) = thread.first_message_id
            && let Some(root) = self.repos.message_repo.get_by_id(root_message_id).await?
        {
            root_author_id = Some(root.sent_by);
            // A deleted root is a tombstone and shows no text.
            root_text = root.text.filter(|_| !root.is_deleted);
        }

        Ok(ThreadDto {
            id: thread.id,
            server_id,
            conversation_id: thread.conversation_id,
            root_message_id: thread.first_message_id,
            root_author_id,
            root_text,
            created_at: thread.created_at,
            last_message_at: thread.last_message_at,
        })
    }
}

#[async_trait]
impl ThreadService for DefaultThreadService {
    async fn get_thread_by_root_message(
        &self,
        query: GetThreadByRootMessageQuery,
    ) -> ServiceResult<Option<ThreadDto>> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let root_message = self
            .repos
            .message_repo
            .get_by_id(query.root_message_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if root_message.conversation_id != query.conversation_id || root_message.thread_id.is_some()
        {
            return Err(ServiceError::NotFound);
        }

        let Some(thread) = self
            .repos
            .message_thread_repo
            .get_by_root_message(query.root_message_id)
            .await?
        else {
            return Ok(None);
        };

        Ok(Some(self.to_dto(thread, query.server_id).await?))
    }

    async fn get_thread(&self, query: GetThreadQuery) -> ServiceResult<ThreadDto> {
        ensure_conversation_member_access(
            ConversationAccessRepos::from_arcs(
                &self.repos.server_member_repo,
                &self.repos.conversation_repo,
                &self.repos.conversation_member_repo,
            ),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        let thread = self
            .repos
            .message_thread_repo
            .get_by_id(query.thread_id)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if thread.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }

        self.to_dto(thread, query.server_id).await
    }
}
