use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::message::model::NewMessage;
use rim_core::repository::pagination::SortOrder;
use rim_services::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, ListMessagesQuery, ListThreadMessagesQuery,
    SendMessageCmd, SendThreadMessageCmd,
};
use rim_services::message::dto::{MessageDto, MessagePageCursor};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::dto::AttachmentDto;
use rim_services::{ServiceError, ServiceResult};

use crate::MessageRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[cfg(test)]
use crate::test_support::message::{
    ConversationMemberRepoPortAdapter, ConversationRepoPortAdapter,
    MessageAttachmentRepoPortAdapter, MessageRepoPortAdapter, MessageReposPorts,
    ServerMemberRepoPortAdapter, ThreadRepoPortAdapter,
};

#[derive(Clone)]
pub struct DefaultMessageService {
    repos: MessageRepos,
}

impl DefaultMessageService {
    pub fn new(repos: MessageRepos) -> Self {
        Self { repos }
    }

    #[cfg(test)]
    pub fn new_for_tests(repos: MessageReposPorts) -> Self {
        Self::new(MessageRepos {
            conversation_repo: std::sync::Arc::new(ConversationRepoPortAdapter(
                repos.conversation_repo,
            )),
            conversation_member_repo: std::sync::Arc::new(ConversationMemberRepoPortAdapter(
                repos.conversation_member_repo,
            )),
            message_repo: std::sync::Arc::new(MessageRepoPortAdapter(repos.message_repo)),
            message_attachment_repo: std::sync::Arc::new(MessageAttachmentRepoPortAdapter(
                repos.message_attachment_repo,
            )),
            message_thread_repo: std::sync::Arc::new(ThreadRepoPortAdapter(
                repos.message_thread_repo,
            )),
            server_member_repo: std::sync::Arc::new(ServerMemberRepoPortAdapter(
                repos.server_member_repo,
            )),
        })
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
        message: rim_core::message::model::Message,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<MessageDto> {
        let root_thread_id = if message.thread_id.is_none() {
            self.repos
                .message_thread_repo
                .get_by_root_message(message.id)
                .await
                .map_err(rim_services::map_repo_error)?
                .map(|thread| thread.id)
        } else {
            None
        };

        let attachments = self
            .repos
            .message_attachment_repo
            .get_attachments_for_message(message.id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|att| AttachmentDto {
                id: att.id,
                status: crate::message_attachment::DefaultMessageAttachmentService::metadata_status(
                    att.metadata.as_ref(),
                ),
                filename: att.file_name,
                content_type: att.mime_type,
                size_bytes: att.file_size.max(0) as u64,
            })
            .collect();

        Ok(MessageDto {
            id: message.id,
            server_id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            root_thread_id,
            author_id: message.sent_by,
            text: message.text,
            has_attachments: message.has_attachments,
            attachments,
            parent_message_id: message.parent_message_id,
            is_deleted: message.is_deleted,
            sent_at: message.sent_at,
        })
    }

    fn to_dto_with_root_thread_id(
        &self,
        message: rim_core::message::model::Message,
        server_id: rim_core::server::model::ServerId,
        root_thread_id: Option<rim_core::message_thread::model::MessageThreadId>,
        attachments: Vec<AttachmentDto>,
    ) -> MessageDto {
        MessageDto {
            id: message.id,
            server_id,
            conversation_id: message.conversation_id,
            thread_id: message.thread_id,
            root_thread_id,
            author_id: message.sent_by,
            text: message.text,
            has_attachments: message.has_attachments,
            attachments,
            parent_message_id: message.parent_message_id,
            is_deleted: message.is_deleted,
            sent_at: message.sent_at,
        }
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
        let _conv = ensure_conversation_in_server(
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

                if parent.conversation_id != cmd.conversation_id || parent.thread_id.is_some() {
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        let new_message = NewMessage {
            conversation_id: cmd.conversation_id,
            text,
            sent_by: cmd.actor.user_id,
            thread_id: None,
            parent_message_id,
            attachments: vec![],
        };

        let message = self
            .repos
            .message_repo
            .create_message(&new_message)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(message_id = %message.id, is_reply = parent_message_id.is_some(), "created timeline message");

        self.repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %cmd.conversation_id,
            "updated conversation activity"
        );

        self.to_dto(message, cmd.server_id).await
    }

    async fn send_thread_message(&self, cmd: SendThreadMessageCmd) -> ServiceResult<MessageDto> {
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

        if !cmd.attachments.is_empty() {
            return Err(ServiceError::Validation(
                "attachments are not supported in v0".to_string(),
            ));
        }

        let thread = self
            .ensure_thread_in_conversation(cmd.thread_id, cmd.conversation_id)
            .await?;

        let text = cmd.text.as_deref().map(str::trim).map(ToString::to_string);
        let text = match text {
            Some(t) if !t.is_empty() => Some(t),
            _ => {
                return Err(ServiceError::Validation(
                    "message text must be non-empty".to_string(),
                ));
            }
        };

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

                let is_root_parent =
                    thread.first_message_id == Some(parent.id) && parent.thread_id.is_none();
                let is_thread_parent = parent.thread_id == Some(cmd.thread_id);
                let valid_parent = is_root_parent || is_thread_parent;

                if parent.conversation_id != cmd.conversation_id || !valid_parent {
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        let message = self
            .repos
            .message_repo
            .create_message(&NewMessage {
                conversation_id: cmd.conversation_id,
                text,
                sent_by: cmd.actor.user_id,
                thread_id: Some(cmd.thread_id),
                parent_message_id,
                attachments: vec![],
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        self.repos
            .message_thread_repo
            .touch_last_message_at(cmd.thread_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(message_id = %message.id, thread_id = %cmd.thread_id, "created thread message");

        self.to_dto(message, cmd.server_id).await
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

        if msg.conversation_id != query.conversation_id || msg.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

        self.to_dto(msg, query.server_id).await
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
            .message_repo
            .list_messages(
                query.conversation_id,
                None,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
                query.include_deleted,
                SortOrder::Desc,
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        let root_message_ids = page
            .items
            .iter()
            .filter(|m| m.thread_id.is_none())
            .map(|m| m.id)
            .collect::<Vec<_>>();
        let root_thread_ids = self
            .repos
            .message_thread_repo
            .get_thread_ids_by_root_messages(&root_message_ids)
            .await
            .map_err(rim_services::map_repo_error)?;
        let attachments_by_message = self
            .repos
            .message_attachment_repo
            .get_attachments_for_messages(&page.items.iter().map(|m| m.id).collect::<Vec<_>>())
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for m in page.items {
            let root_thread_id = if m.thread_id.is_none() {
                root_thread_ids.get(&m.id).copied()
            } else {
                None
            };
            let attachments = attachments_by_message
                .get(&m.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|att| AttachmentDto {
                    id: att.id,
                    status:
                        crate::message_attachment::DefaultMessageAttachmentService::metadata_status(
                            att.metadata.as_ref(),
                        ),
                    filename: att.file_name,
                    content_type: att.mime_type,
                    size_bytes: att.file_size.max(0) as u64,
                })
                .collect();
            items.push(self.to_dto_with_root_thread_id(
                m,
                query.server_id,
                root_thread_id,
                attachments,
            ));
        }

        Ok(rim_core::repository::pagination::Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn list_thread_messages(
        &self,
        query: ListThreadMessagesQuery,
    ) -> ServiceResult<rim_core::repository::pagination::Page<MessageDto, MessagePageCursor>> {
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
        self.ensure_thread_in_conversation(query.thread_id, query.conversation_id)
            .await?;

        let page = self
            .repos
            .message_repo
            .list_messages(
                query.conversation_id,
                Some(query.thread_id),
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
                query.include_deleted,
                SortOrder::Desc,
            )
            .await
            .map_err(rim_services::map_repo_error)?;
        let attachments_by_message = self
            .repos
            .message_attachment_repo
            .get_attachments_for_messages(&page.items.iter().map(|m| m.id).collect::<Vec<_>>())
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for m in page.items {
            let attachments = attachments_by_message
                .get(&m.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(|att| AttachmentDto {
                    id: att.id,
                    status:
                        crate::message_attachment::DefaultMessageAttachmentService::metadata_status(
                            att.metadata.as_ref(),
                        ),
                    filename: att.file_name,
                    content_type: att.mime_type,
                    size_bytes: att.file_size.max(0) as u64,
                })
                .collect();
            items.push(self.to_dto_with_root_thread_id(m, query.server_id, None, attachments));
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

        if msg.conversation_id != cmd.conversation_id || msg.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

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

        self.to_dto(updated, cmd.server_id).await
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

        if msg.conversation_id != cmd.conversation_id || msg.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

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
