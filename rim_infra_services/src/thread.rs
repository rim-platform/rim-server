use async_trait::async_trait;
use tracing::info;

use rim_services::message_thread::command::{
    CreateThreadCmd, GetThreadByRootMessageQuery, GetThreadQuery, ListThreadsQuery,
};
use rim_services::message_thread::dto::{ThreadDto, ThreadPageCursor};
use rim_services::message_thread::service::ThreadService;
use rim_services::{ServiceError, ServiceResult};

use rim_core::repository::pagination::Page;

use crate::ThreadRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[derive(Clone)]
pub struct DefaultThreadService {
    repos: ThreadRepos,
}

impl DefaultThreadService {
    pub fn new(repos: ThreadRepos) -> Self {
        Self { repos }
    }

    async fn ensure_conversation_member(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
        user_id: rim_core::user::model::UserId,
    ) -> ServiceResult<()> {
        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(conversation_id, user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }
        Ok(())
    }

    async fn to_dto(
        &self,
        thread: rim_core::message_thread::model::MessageThread,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<ThreadDto> {
        let mut root_author_id = None;
        let mut root_text = None;

        if let Some(root_message_id) = thread.first_message_id
            && let Some(root) = self
                .repos
                .message_repo
                .get_by_id(root_message_id)
                .await
                .map_err(rim_services::map_repo_error)?
        {
            root_author_id = Some(root.sent_by);
            root_text = root.text;
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

    fn to_dto_with_root(
        &self,
        thread: rim_core::message_thread::model::MessageThread,
        server_id: rim_core::server::model::ServerId,
        root: Option<&rim_core::message::model::Message>,
    ) -> ThreadDto {
        ThreadDto {
            id: thread.id,
            server_id,
            conversation_id: thread.conversation_id,
            root_message_id: thread.first_message_id,
            root_author_id: root.map(|m| m.sent_by),
            root_text: root.and_then(|m| m.text.clone()),
            created_at: thread.created_at,
            last_message_at: thread.last_message_at,
        }
    }
}

#[async_trait]
impl ThreadService for DefaultThreadService {
    async fn create_thread(&self, cmd: CreateThreadCmd) -> ServiceResult<ThreadDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let _conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        self.ensure_conversation_member(cmd.conversation_id, cmd.actor.user_id)
            .await?;

        let root_message = self
            .repos
            .message_repo
            .get_by_id(cmd.root_message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if root_message.conversation_id != cmd.conversation_id || root_message.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

        if let Some(existing) = self
            .repos
            .message_thread_repo
            .get_by_root_message(cmd.root_message_id)
            .await
            .map_err(rim_services::map_repo_error)?
        {
            return self.to_dto(existing, cmd.server_id).await;
        }

        let thread = match self
            .repos
            .message_thread_repo
            .create_thread_for_root(cmd.conversation_id, cmd.root_message_id)
            .await
        {
            Ok(thread) => thread,
            Err(err)
                if matches!(
                    err.kind(),
                    rim_core::repository::error::RepoErrorKind::Conflict
                ) =>
            {
                self.repos
                    .message_thread_repo
                    .get_by_root_message(cmd.root_message_id)
                    .await
                    .map_err(rim_services::map_repo_error)?
                    .ok_or(ServiceError::Conflict(
                        "thread already exists for message".to_string(),
                    ))?
            }
            Err(err) => return Err(rim_services::map_repo_error(err)),
        };

        info!(thread_id = %thread.id, root_message_id = %cmd.root_message_id, "created thread from message");

        self.to_dto(thread, cmd.server_id).await
    }

    async fn get_thread_by_root_message(
        &self,
        query: GetThreadByRootMessageQuery,
    ) -> ServiceResult<Option<ThreadDto>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let _conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        self.ensure_conversation_member(query.conversation_id, query.actor.user_id)
            .await?;

        let root_message = self
            .repos
            .message_repo
            .get_by_id(query.root_message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if root_message.conversation_id != query.conversation_id || root_message.thread_id.is_some()
        {
            return Err(ServiceError::NotFound);
        }

        let Some(thread) = self
            .repos
            .message_thread_repo
            .get_by_root_message(query.root_message_id)
            .await
            .map_err(rim_services::map_repo_error)?
        else {
            return Ok(None);
        };

        Ok(Some(self.to_dto(thread, query.server_id).await?))
    }

    async fn get_thread(&self, query: GetThreadQuery) -> ServiceResult<ThreadDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let _conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        self.ensure_conversation_member(query.conversation_id, query.actor.user_id)
            .await?;

        let thread = self
            .repos
            .message_thread_repo
            .get_by_id(query.thread_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if thread.conversation_id != query.conversation_id {
            return Err(ServiceError::NotFound);
        }

        self.to_dto(thread, query.server_id).await
    }

    async fn list_threads(
        &self,
        query: ListThreadsQuery,
    ) -> ServiceResult<Page<ThreadDto, ThreadPageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let _conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        self.ensure_conversation_member(query.conversation_id, query.actor.user_id)
            .await?;

        let page = self
            .repos
            .message_thread_repo
            .list_threads_for_conversation(
                query.conversation_id,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %query.conversation_id,
            thread_count = page.items.len(),
            "listed threads"
        );

        let root_ids = page
            .items
            .iter()
            .filter_map(|t| t.first_message_id)
            .collect::<Vec<_>>();
        let roots = self
            .repos
            .message_repo
            .get_by_ids(&root_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for thread in page.items {
            let root = thread.first_message_id.and_then(|id| roots.get(&id));
            items.push(self.to_dto_with_root(thread, query.server_id, root));
        }

        Ok(Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }
}
