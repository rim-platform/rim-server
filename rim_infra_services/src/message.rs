use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::repository::error::RepoErrorKind;
use rim_core::message::model::NewMessage;
use rim_core::repository::pagination::SortOrder;
use rim_services::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessagesQuery, SendMessageCmd,
};
use rim_services::message::dto::{MessageDto, MessagePageCursor};
use rim_services::message::service::MessageService;
use rim_services::{ServiceError, ServiceResult};

use crate::MessageRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[derive(Clone)]
pub struct DefaultMessageService {
    repos: MessageRepos,
}

impl DefaultMessageService {
    pub fn new(repos: MessageRepos) -> Self {
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

    async fn ensure_thread_in_conversation(
        &self,
        thread_id: rim_core::message_thread::model::MessageThreadId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<rim_core::message_thread::model::MessageThread> {
        let thread = self
            .repos
            .message_thread_repo
            .get_by_id(thread_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if thread.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        Ok(thread)
    }

    async fn to_dto(
        &self,
        message: rim_core::message::model::Message,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<MessageDto> {
        Ok(MessageDto {
            id: message.id,
            server_id,
            conversation_id,
            thread_id: message.message_thread_id,
            author_id: message.sent_by,
            text: message.text,
            has_attachments: message.has_attachments,
            parent_message_id: message.parent_message_id,
            is_deleted: message.is_deleted,
            sent_at: message.sent_at,
        })
    }
}

#[async_trait]
impl MessageService for DefaultMessageService {
    #[instrument(
        name = "message.send_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            conversation_id = %cmd.conversation_id,
            parent_message_id = ?cmd.parent_message_id,
            text_len = cmd.text.as_ref().map(|t| t.len()).unwrap_or(0)
        )
    )]
    async fn send_message(&self, cmd: SendMessageCmd) -> ServiceResult<MessageDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Validate conversation exists and belongs to the server.
        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        self.ensure_conversation_member(cmd.conversation_id, cmd.actor.user_id)
            .await?;

        if !cmd.attachments.is_empty() {
            return Err(ServiceError::Validation(
                "attachments are not supported in v0".to_string(),
            ));
        }

        let text = cmd.text.as_deref().map(str::trim).map(ToString::to_string);
        let text = match text {
            Some(t) if !t.is_empty() => Some(t),
            _ => {
                return Err(ServiceError::Validation(
                    "message text must be non-empty".to_string(),
                ));
            }
        };

        // v0 semantics: conversation has a dedicated "main thread" for the default timeline.
        // Root messages and replies (quotes) are both persisted into this thread.
        let main_thread_id = conv.main_thread_id;

        let parent_message_id = match cmd.parent_message_id {
            None => None,
            Some(parent_id) => {
                let parent = self
                    .repos
                    .message_repo
                    .get_by_id(parent_id)
                    .await
                    .map_err(rim_services::map_repo_error)?
                    .ok_or(ServiceError::NotFound)?;

                if parent.message_thread_id != main_thread_id {
                    // For now, only allow replying to messages in the main timeline.
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        let thread_id = main_thread_id;

        let new_message = NewMessage {
            text,
            sent_by: cmd.actor.user_id,
            message_thread_id: thread_id,
            parent_message_id,
            attachments: vec![],
        };

        let message = self
            .repos
            .message_repo
            .create_message(&new_message)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(message_id = %message.id, thread_id = %thread_id, is_reply = parent_message_id.is_some(), "created message");

        // Ensure main thread has a first_message_id set.
        // (After the backfill migration it should already be set, but keep this idempotent.)
        if parent_message_id.is_none() {
            if let Some(thread) = self
                .repos
                .message_thread_repo
                .get_by_id(thread_id)
                .await
                .map_err(rim_services::map_repo_error)?
            {
                if thread.first_message_id.is_none() {
                    let res = self
                        .repos
                        .message_thread_repo
                        .set_first_message(thread_id, message.id)
                        .await;
                    match res {
                        Ok(()) => {
                            info!(thread_id = %thread_id, root_message_id = %message.id, "bound main thread first message");
                        }
                        Err(e) if matches!(e.kind(), RepoErrorKind::Conflict) => {
                            // Another request likely raced and set it first; safe to ignore.
                        }
                        Err(e) => return Err(rim_services::map_repo_error(e)),
                    }
                }
            }
        }

        // Update activity.
        self.repos
            .message_thread_repo
            .touch_last_message_at(thread_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(thread_id = %thread_id, "updated thread activity");

        self.repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %cmd.conversation_id,
            "updated conversation activity"
        );

        self.to_dto(message, cmd.server_id, cmd.conversation_id)
            .await
    }

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto> {
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

        let msg = self
            .repos
            .message_repo
            .get_by_id(query.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Ensure message belongs to the conversation.
        let _thread = self
            .ensure_thread_in_conversation(msg.message_thread_id, query.conversation_id)
            .await?;

        self.to_dto(msg, query.server_id, query.conversation_id)
            .await
    }

    async fn list_messages(
        &self,
        query: ListMessagesQuery,
    ) -> ServiceResult<rim_core::repository::pagination::Page<MessageDto, MessagePageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        self.ensure_conversation_member(query.conversation_id, query.actor.user_id)
            .await?;

        let _thread = self
            .ensure_thread_in_conversation(conv.main_thread_id, query.conversation_id)
            .await?;

        let page = self
            .repos
            .message_repo
            .list_messages(
                conv.main_thread_id,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
                query.include_deleted,
                SortOrder::Desc,
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for m in page.items {
            items.push(
                self.to_dto(m, query.server_id, query.conversation_id)
                    .await?,
            );
        }

        Ok(rim_core::repository::pagination::Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto> {
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

        let msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        // Ensure message belongs to conversation.
        let _thread = self
            .ensure_thread_in_conversation(msg.message_thread_id, cmd.conversation_id)
            .await?;

        if msg.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        let new_text = cmd.text.trim();
        if new_text.is_empty() {
            return Err(ServiceError::Validation(
                "message text must be non-empty".to_string(),
            ));
        }

        let updated = self
            .repos
            .message_repo
            .update_message_text(cmd.message_id, new_text.to_string())
            .await
            .map_err(rim_services::map_repo_error)?;

        self.to_dto(updated, cmd.server_id, cmd.conversation_id)
            .await
    }

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()> {
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

        if cmd.hard {
            return Err(ServiceError::Validation(
                "hard delete is not supported in v0".to_string(),
            ));
        }

        // v0: only author can delete.
        let msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        let _thread = self
            .ensure_thread_in_conversation(msg.message_thread_id, cmd.conversation_id)
            .await?;

        if msg.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos
            .message_repo
            .soft_delete_message(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(())
    }
}
