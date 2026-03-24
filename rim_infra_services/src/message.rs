use async_trait::async_trait;
use sea_orm::DatabaseConnection;
use tracing::{info, instrument};

use rim_core::message::model::NewMessage;
use rim_core::repository::pagination::SortOrder;
use rim_infra_db::begin_transaction;
use rim_services::events::{MessageEventData, Observers};
use rim_services::message::command::{
    DeleteMessageCmd, EditMessageCmd, GetMessageQuery, GetMessageReferenceQuery, ListMessagesQuery,
    ListThreadMessagesQuery, SearchMessagesQuery, SendMessageCmd, SendThreadMessageCmd,
};
use rim_services::message::dto::{MessageDto, MessagePageCursor, MessageReferenceDto};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::dto::AttachmentDto;
use rim_services::{ServiceError, ServiceResult};

use crate::MessageRepos;
use crate::repos::AppRepos;
use crate::util::{
    ensure_conversation_access, ensure_conversation_in_server, ensure_server_member,
};

#[derive(Clone)]
pub struct DefaultMessageService {
    repos: MessageRepos,
    db: Option<DatabaseConnection>,
    observers: Observers,
}

impl DefaultMessageService {
    pub fn new(repos: MessageRepos, db: Option<DatabaseConnection>, observers: Observers) -> Self {
        Self {
            repos,
            db,
            observers,
        }
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

    async fn get_message_model(
        &self,
        actor: rim_services::Actor,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        message_id: rim_core::message::model::MessageId,
    ) -> ServiceResult<rim_core::message::model::Message> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            server_id,
            actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            conversation_id,
            server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            actor.user_id,
        )
        .await?;

        let msg = self
            .repos
            .message_repo
            .get_by_id(message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if msg.conversation_id != conversation_id {
            return Err(ServiceError::NotFound);
        }

        Ok(msg)
    }

    async fn map_messages_to_dtos(
        &self,
        server_id: rim_core::server::model::ServerId,
        messages: Vec<rim_core::message::model::Message>,
        include_root_thread_ids: bool,
    ) -> ServiceResult<Vec<MessageDto>> {
        let message_ids = messages
            .iter()
            .map(|message| message.id)
            .collect::<Vec<_>>();
        let attachments_by_message = self
            .repos
            .message_attachment_repo
            .get_attachments_for_messages(&message_ids)
            .await
            .map_err(rim_services::map_repo_error)?;
        let root_thread_ids = if include_root_thread_ids {
            self.repos
                .message_thread_repo
                .get_thread_ids_by_root_messages(&message_ids)
                .await
                .map_err(rim_services::map_repo_error)?
        } else {
            std::collections::HashMap::new()
        };

        let mut items = Vec::with_capacity(messages.len());
        for message in messages {
            let root_thread_id = if include_root_thread_ids && message.thread_id.is_none() {
                root_thread_ids.get(&message.id).copied()
            } else {
                None
            };
            let attachments = attachments_by_message
                .get(&message.id)
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
                message,
                server_id,
                root_thread_id,
                attachments,
            ));
        }

        Ok(items)
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

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
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

        // Run create + activity touch inside a transaction when DB is available.
        let message = if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let txn_repos = AppRepos::with_handle(&scope.handle());

            let message = txn_repos
                .message_repo
                .create_message(&new_message)
                .await
                .map_err(rim_services::map_repo_error)?;

            txn_repos
                .conversation_repo
                .touch_last_activity_at(cmd.conversation_id, message.sent_at)
                .await
                .map_err(rim_services::map_repo_error)?;

            scope.commit().await.map_err(ServiceError::internal)?;
            message
        } else {
            let message = self
                .repos
                .message_repo
                .create_message(&new_message)
                .await
                .map_err(rim_services::map_repo_error)?;

            self.repos
                .conversation_repo
                .touch_last_activity_at(cmd.conversation_id, message.sent_at)
                .await
                .map_err(rim_services::map_repo_error)?;

            message
        };

        info!(message_id = %message.id, is_reply = parent_message_id.is_some(), "created timeline message");
        info!(
            conversation_id = %cmd.conversation_id,
            "updated conversation activity"
        );

        let dto = self.to_dto(message, cmd.server_id).await?;

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        self.observers.message.on_message_sent(
            &cmd.server_id.to_string(),
            &dto.conversation_id.to_string(),
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData {
                id: dto.id.to_string(),
                server_id: dto.server_id.to_string(),
                conversation_id: dto.conversation_id.to_string(),
                thread_id: dto.thread_id.map(|id| id.to_string()),
                root_thread_id: dto.root_thread_id.map(|id| id.to_string()),
                author_id: dto.author_id.to_string(),
                text: dto.text.clone(),
                has_attachments: dto.has_attachments,
                parent_message_id: dto.parent_message_id.map(|id| id.to_string()),
                is_deleted: dto.is_deleted,
                sent_at: dto.sent_at.to_rfc3339(),
                reactions: vec![],
            },
        );

        Ok(dto)
    }

    async fn send_thread_message(&self, cmd: SendThreadMessageCmd) -> ServiceResult<MessageDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
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

        let new_msg = NewMessage {
            conversation_id: cmd.conversation_id,
            text,
            sent_by: cmd.actor.user_id,
            thread_id: Some(cmd.thread_id),
            parent_message_id,
            attachments: vec![],
        };

        // Run create + touch inside a transaction when DB is available.
        let message = if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let txn_repos = AppRepos::with_handle(&scope.handle());

            let message = txn_repos
                .message_repo
                .create_message(&new_msg)
                .await
                .map_err(rim_services::map_repo_error)?;

            txn_repos
                .message_thread_repo
                .touch_last_message_at(cmd.thread_id, message.sent_at)
                .await
                .map_err(rim_services::map_repo_error)?;

            txn_repos
                .conversation_repo
                .touch_last_activity_at(cmd.conversation_id, message.sent_at)
                .await
                .map_err(rim_services::map_repo_error)?;

            scope.commit().await.map_err(ServiceError::internal)?;
            message
        } else {
            let message = self
                .repos
                .message_repo
                .create_message(&new_msg)
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

            message
        };

        info!(message_id = %message.id, thread_id = %cmd.thread_id, "created thread message");

        let dto = self.to_dto(message, cmd.server_id).await?;

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        self.observers.message.on_message_sent(
            &cmd.server_id.to_string(),
            &dto.conversation_id.to_string(),
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData {
                id: dto.id.to_string(),
                server_id: dto.server_id.to_string(),
                conversation_id: dto.conversation_id.to_string(),
                thread_id: dto.thread_id.map(|id| id.to_string()),
                root_thread_id: dto.root_thread_id.map(|id| id.to_string()),
                author_id: dto.author_id.to_string(),
                text: dto.text.clone(),
                has_attachments: dto.has_attachments,
                parent_message_id: dto.parent_message_id.map(|id| id.to_string()),
                is_deleted: dto.is_deleted,
                sent_at: dto.sent_at.to_rfc3339(),
                reactions: vec![],
            },
        );

        Ok(dto)
    }

    async fn get_message(&self, query: GetMessageQuery) -> ServiceResult<MessageDto> {
        let msg = self
            .get_message_model(
                query.actor,
                query.server_id,
                query.conversation_id,
                query.message_id,
            )
            .await?;

        if msg.thread_id.is_some() {
            return Err(ServiceError::NotFound);
        }

        self.to_dto(msg, query.server_id).await
    }

    async fn get_message_reference(
        &self,
        query: GetMessageReferenceQuery,
    ) -> ServiceResult<MessageReferenceDto> {
        let msg = self
            .get_message_model(
                query.actor,
                query.server_id,
                query.conversation_id,
                query.message_id,
            )
            .await?;

        let permalink_path = match msg.thread_id {
            Some(thread_id) => format!(
                "/workspaces/{}/conversations/{}/threads/{}/messages/{}",
                query.server_id, query.conversation_id, thread_id, msg.id
            ),
            None => format!(
                "/workspaces/{}/conversations/{}/messages/{}",
                query.server_id, query.conversation_id, msg.id
            ),
        };

        Ok(MessageReferenceDto {
            server_id: query.server_id,
            conversation_id: query.conversation_id,
            message_id: msg.id,
            thread_id: msg.thread_id,
            permalink_path,
            anchor: format!("message-{}", msg.id),
        })
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

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            query.actor.user_id,
        )
        .await?;

        let page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: None,
                limit: query.limit,
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        let items = self
            .map_messages_to_dtos(query.server_id, page.items, true)
            .await?;

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

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            query.actor.user_id,
        )
        .await?;
        self.ensure_thread_in_conversation(query.thread_id, query.conversation_id)
            .await?;

        let page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: Some(query.thread_id),
                limit: query.limit,
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await
            .map_err(rim_services::map_repo_error)?;
        let items = self
            .map_messages_to_dtos(query.server_id, page.items, false)
            .await?;

        Ok(rim_core::repository::pagination::Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn search_messages(&self, query: SearchMessagesQuery) -> ServiceResult<Vec<MessageDto>> {
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

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            query.actor.user_id,
        )
        .await?;

        let q = query.query.trim();
        if q.is_empty() {
            return Err(ServiceError::Validation(
                "search query must be non-empty".to_string(),
            ));
        }

        let messages = self
            .repos
            .message_repo
            .search_messages(query.conversation_id, q, query.limit, query.include_deleted)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.map_messages_to_dtos(query.server_id, messages, true)
            .await
    }

    async fn edit_message(&self, cmd: EditMessageCmd) -> ServiceResult<MessageDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
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

        let dto = self.to_dto(updated, cmd.server_id).await?;

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        self.observers.message.on_message_updated(
            &cmd.server_id.to_string(),
            &dto.conversation_id.to_string(),
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData {
                id: dto.id.to_string(),
                server_id: dto.server_id.to_string(),
                conversation_id: dto.conversation_id.to_string(),
                thread_id: dto.thread_id.map(|id| id.to_string()),
                root_thread_id: dto.root_thread_id.map(|id| id.to_string()),
                author_id: dto.author_id.to_string(),
                text: dto.text.clone(),
                has_attachments: dto.has_attachments,
                parent_message_id: dto.parent_message_id.map(|id| id.to_string()),
                is_deleted: dto.is_deleted,
                sent_at: dto.sent_at.to_rfc3339(),
                reactions: vec![],
            },
        );

        Ok(dto)
    }

    async fn delete_message(&self, cmd: DeleteMessageCmd) -> ServiceResult<()> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conv,
            cmd.actor.user_id,
        )
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

        let member_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(cmd.conversation_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .into_iter()
            .map(|id| id.to_string())
            .collect::<Vec<_>>();

        self.observers.message.on_message_deleted(
            &cmd.server_id.to_string(),
            &cmd.conversation_id.to_string(),
            conv.conversation_type.as_str(),
            member_user_ids,
            &cmd.message_id.to_string(),
        );

        Ok(())
    }
}
