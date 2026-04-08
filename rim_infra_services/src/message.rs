use async_trait::async_trait;
use rim_infra_db::TransactionScope;
use sea_orm::DatabaseConnection;
use tracing::{info, instrument};

/// Maximum number of items any paginated query can return in a single page.
const MAX_PAGE_LIMIT: u32 = 200;

use rim_core::message::model::NewMessage;
use rim_core::repository::pagination::SortOrder;
use rim_infra_db::begin_transaction;
use rim_services::events::{MessageEventData, Observers};
use rim_services::message::command::{
    DeleteMessageCmd, EditMessageCmd, ForwardMessageCmd, GetMessageQuery, GetMessageReferenceQuery,
    ListMessagesQuery, ListThreadMessagesQuery, ReplyToMessageCmd, SearchMessagesQuery,
    SendMessageCmd,
};
use rim_services::message::dto::{MessageDto, MessagePageCursor, MessageReferenceDto};
use rim_services::message::service::MessageService;
use rim_services::message_attachment::dto::AttachmentDto;
use rim_services::{ServiceError, ServiceResult};

/// Maximum allowed length (in characters) for message text.
const MAX_MESSAGE_TEXT_LENGTH: usize = 10_000;

use crate::MessageRepos;
use crate::mention::parse_mention_ids;
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

    /// Begin a transaction if a database connection is available.
    ///
    /// Returns `(repos, optional_scope)`. The caller must commit the scope on
    /// success — if dropped without committing it rolls back automatically.
    async fn begin_write(&self) -> ServiceResult<(MessageRepos, Option<TransactionScope>)> {
        if let Some(db) = &self.db {
            let scope = begin_transaction(db)
                .await
                .map_err(ServiceError::internal)?;
            let repos = AppRepos::with_handle(&scope.handle()).for_message();
            Ok((repos, Some(scope)))
        } else {
            Ok((self.repos.clone(), None))
        }
    }

    /// Commit the transaction scope if present.
    async fn commit_if_present(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    /// Parse mentions from message text, validate them against workspace members,
    /// store in DB, and emit UserMentioned event.
    async fn process_mentions(
        &self,
        text: Option<&str>,
        message_id: rim_core::message::model::MessageId,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
        author_id: rim_core::user::model::UserId,
    ) {
        let Some(text) = text else { return };
        let mention_ids = parse_mention_ids(text);
        if mention_ids.is_empty() {
            return;
        }

        // Validate: each mentioned user must be a workspace member.
        let mut valid_ids = Vec::new();
        for uid in &mention_ids {
            if *uid == author_id {
                continue; // Don't mention yourself
            }
            if let Ok(true) = self
                .repos
                .server_member_repo
                .is_member(server_id, *uid)
                .await
            {
                valid_ids.push(*uid);
            }
        }

        if valid_ids.is_empty() {
            return;
        }

        if let Err(e) = self
            .repos
            .message_mention_repo
            .create_mentions(message_id, &valid_ids)
            .await
        {
            tracing::warn!(error = %e, "failed to store mentions");
            return;
        }

        self.observers.mention.on_user_mentioned(
            server_id,
            conversation_id,
            message_id,
            valid_ids,
            author_id,
        );
    }

    /// Collect conversation member IDs for event broadcasting.
    async fn list_member_user_ids(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<Vec<rim_core::user::model::UserId>> {
        self.repos
            .conversation_member_repo
            .list_member_ids(conversation_id)
            .await
            .map_err(rim_services::map_repo_error)
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
            .map(crate::message_attachment::DefaultMessageAttachmentService::attachment_dto)
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
            forwarded_from_message_id: message.forwarded_from_message_id,
            thread_root_message_id: None,
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
        thread_root_message_id: Option<rim_core::message::model::MessageId>,
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
            forwarded_from_message_id: message.forwarded_from_message_id,
            thread_root_message_id,
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

        // Batch-load threads for messages that are thread replies, so we can
        // resolve thread_root_message_id (= thread.first_message_id).
        let thread_ids_from_messages: Vec<rim_core::message_thread::model::MessageThreadId> =
            messages
                .iter()
                .filter_map(|m| m.thread_id)
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
        let thread_root_message_map: std::collections::HashMap<
            rim_core::message_thread::model::MessageThreadId,
            rim_core::message::model::MessageId,
        > = if !thread_ids_from_messages.is_empty() {
            let threads = self
                .repos
                .message_thread_repo
                .get_by_ids(&thread_ids_from_messages)
                .await
                .map_err(rim_services::map_repo_error)?;
            threads
                .into_iter()
                .filter_map(|t| t.first_message_id.map(|mid| (t.id, mid)))
                .collect()
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
            let thread_root_message_id = message
                .thread_id
                .and_then(|tid| thread_root_message_map.get(&tid).copied());
            let attachments = attachments_by_message
                .get(&message.id)
                .cloned()
                .unwrap_or_default()
                .into_iter()
                .map(crate::message_attachment::DefaultMessageAttachmentService::attachment_dto)
                .collect();
            items.push(self.to_dto_with_root_thread_id(
                message,
                server_id,
                root_thread_id,
                attachments,
                thread_root_message_id,
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
            _ => None, // Allow empty text (e.g. attachment-only messages)
        };

        if let Some(ref t) = text {
            if t.chars().count() > MAX_MESSAGE_TEXT_LENGTH {
                return Err(ServiceError::Validation(format!(
                    "message text too long (max {} characters)",
                    MAX_MESSAGE_TEXT_LENGTH
                )));
            }
        }

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
            forwarded_from_message_id: None,
            attachments: vec![],
        };

        // Run create + activity touch inside a transaction when DB is available.
        let (repos, scope) = self.begin_write().await?;

        let message = repos
            .message_repo
            .create_message(&new_message)
            .await
            .map_err(rim_services::map_repo_error)?;

        repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        Self::commit_if_present(scope).await?;

        info!(message_id = %message.id, is_reply = parent_message_id.is_some(), "created timeline message");
        info!(
            conversation_id = %cmd.conversation_id,
            "updated conversation activity"
        );

        let dto = self.to_dto(message, cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.message.on_message_sent(
            cmd.server_id,
            dto.conversation_id,
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData::from(&dto),
        );

        self.process_mentions(
            dto.text.as_deref(),
            dto.id,
            cmd.server_id,
            cmd.conversation_id,
            cmd.actor.user_id,
        )
        .await;

        Ok(dto)
    }

    #[instrument(
        name = "message.reply_to_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            conversation_id = %cmd.conversation_id,
            root_message_id = %cmd.root_message_id,
            parent_message_id = ?cmd.parent_message_id,
            text_len = cmd.text.len()
        )
    )]
    async fn reply_to_message(&self, cmd: ReplyToMessageCmd) -> ServiceResult<MessageDto> {
        // 1. Validate access
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

        // 2. Validate root message exists and belongs to conversation
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

        // 3. Validate text and attachments
        if !cmd.attachments.is_empty() {
            return Err(ServiceError::Validation(
                "attachments are not supported in v0".to_string(),
            ));
        }

        let text = cmd.text.trim().to_string();
        if text.is_empty() {
            return Err(ServiceError::Validation(
                "message text must be non-empty".to_string(),
            ));
        }

        if text.chars().count() > MAX_MESSAGE_TEXT_LENGTH {
            return Err(ServiceError::Validation(format!(
                "message text too long (max {} characters)",
                MAX_MESSAGE_TEXT_LENGTH
            )));
        }

        // 4. Begin transaction
        let (repos, scope) = self.begin_write().await?;

        // 5. Find or create thread
        let mut thread_was_created = false;
        let thread = match repos
            .message_thread_repo
            .get_by_root_message(cmd.root_message_id)
            .await
            .map_err(rim_services::map_repo_error)?
        {
            Some(existing) => existing,
            None => {
                match repos
                    .message_thread_repo
                    .create_thread_for_root(cmd.conversation_id, cmd.root_message_id)
                    .await
                {
                    Ok(thread) => {
                        thread_was_created = true;
                        thread
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            rim_core::repository::error::RepoErrorKind::Conflict
                        ) =>
                    {
                        // Race condition: another request created the thread concurrently.
                        repos
                            .message_thread_repo
                            .get_by_root_message(cmd.root_message_id)
                            .await
                            .map_err(rim_services::map_repo_error)?
                            .ok_or(ServiceError::Conflict(
                                "thread already exists for message".to_string(),
                            ))?
                    }
                    Err(err) => return Err(rim_services::map_repo_error(err)),
                }
            }
        };

        // Validate parent_message_id if provided
        let parent_message_id = match cmd.parent_message_id {
            None => None,
            Some(parent_id) => {
                let parent = repos
                    .message_repo
                    .get_by_id(parent_id)
                    .await
                    .map_err(rim_services::map_repo_error)?
                    .ok_or(ServiceError::NotFound)?;

                let is_root_parent =
                    thread.first_message_id == Some(parent.id) && parent.thread_id.is_none();
                let is_thread_parent = parent.thread_id == Some(thread.id);
                let valid_parent = is_root_parent || is_thread_parent;

                if parent.conversation_id != cmd.conversation_id || !valid_parent {
                    return Err(ServiceError::NotFound);
                }
                Some(parent_id)
            }
        };

        // 6. Create message with thread_id
        let new_msg = NewMessage {
            conversation_id: cmd.conversation_id,
            text: Some(text),
            sent_by: cmd.actor.user_id,
            thread_id: Some(thread.id),
            parent_message_id,
            forwarded_from_message_id: None,
            attachments: vec![],
        };

        let message = repos
            .message_repo
            .create_message(&new_msg)
            .await
            .map_err(rim_services::map_repo_error)?;

        // 7. Touch thread last_message_at
        repos
            .message_thread_repo
            .touch_last_message_at(thread.id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        // 8. Touch conversation last_activity_at
        repos
            .conversation_repo
            .touch_last_activity_at(cmd.conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        // 9. Commit transaction
        Self::commit_if_present(scope).await?;

        info!(
            message_id = %message.id,
            thread_id = %thread.id,
            thread_was_created,
            "created reply message"
        );

        let dto = self.to_dto(message, cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        // 10. Emit events
        if thread_was_created {
            self.observers.thread.on_thread_created(
                cmd.server_id,
                cmd.conversation_id,
                thread.id,
                cmd.root_message_id,
                conv.conversation_type.as_str(),
                member_user_ids.clone(),
            );
        }

        self.observers.message.on_message_sent(
            cmd.server_id,
            dto.conversation_id,
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData::from(&dto),
        );

        self.process_mentions(
            dto.text.as_deref(),
            dto.id,
            cmd.server_id,
            cmd.conversation_id,
            cmd.actor.user_id,
        )
        .await;

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

        let mut page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: None,
                limit: query.limit.min(MAX_PAGE_LIMIT).max(1),
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        // Reverse items to return in display order (oldest-first)
        page.items.reverse();

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

        let mut page = self
            .repos
            .message_repo
            .list_messages(rim_core::message::repository::ListMessagesParams {
                conversation_id: query.conversation_id,
                thread_id: Some(query.thread_id),
                limit: query.limit.min(MAX_PAGE_LIMIT).max(1),
                dir: query.dir,
                cursor: query.cursor.map(Into::into),
                include_deleted: query.include_deleted,
                order: SortOrder::Desc,
            })
            .await
            .map_err(rim_services::map_repo_error)?;

        // Reverse items to return in display order (oldest-first)
        page.items.reverse();

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

        let q = query.query.trim();
        if q.is_empty() {
            return Err(ServiceError::Validation(
                "search query must be non-empty".to_string(),
            ));
        }

        let messages = match query.conversation_id {
            Some(conversation_id) => {
                // Scoped search: single conversation
                let conv = ensure_conversation_in_server(
                    self.repos.conversation_repo.as_ref(),
                    conversation_id,
                    query.server_id,
                )
                .await?;

                ensure_conversation_access(
                    self.repos.conversation_member_repo.as_ref(),
                    &conv,
                    query.actor.user_id,
                )
                .await?;

                // Security: always exclude deleted messages from search results.
                let clamped_limit = query.limit.min(MAX_PAGE_LIMIT).max(1);
                self.repos
                    .message_repo
                    .search_messages(conversation_id, q, clamped_limit, false)
                    .await
                    .map_err(rim_services::map_repo_error)?
            }
            None => {
                // Workspace-wide search: all accessible conversations
                let accessible_ids = self
                    .repos
                    .conversation_repo
                    .list_accessible_conversation_ids(query.server_id, query.actor.user_id)
                    .await
                    .map_err(rim_services::map_repo_error)?;

                let clamped_limit = query.limit.min(MAX_PAGE_LIMIT).max(1);
                self.repos
                    .message_repo
                    .search_messages_across_conversations(&accessible_ids, q, clamped_limit)
                    .await
                    .map_err(rim_services::map_repo_error)?
            }
        };

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

        let new_text = cmd.text.trim();
        if new_text.is_empty() {
            return Err(ServiceError::Validation(
                "message text must be non-empty".to_string(),
            ));
        }

        if new_text.chars().count() > MAX_MESSAGE_TEXT_LENGTH {
            return Err(ServiceError::Validation(format!(
                "message text too long (max {} characters)",
                MAX_MESSAGE_TEXT_LENGTH
            )));
        }

        // Run validation + update inside a transaction.
        let (repos, scope) = self.begin_write().await?;

        let msg = repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if msg.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        if msg.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        if msg.is_deleted {
            return Err(ServiceError::Validation(
                "cannot edit a deleted message".to_string(),
            ));
        }

        let updated = repos
            .message_repo
            .update_message_text(cmd.message_id, new_text.to_string())
            .await
            .map_err(rim_services::map_repo_error)?;

        Self::commit_if_present(scope).await?;

        let dto = self.to_dto(updated, cmd.server_id).await?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.message.on_message_updated(
            cmd.server_id,
            dto.conversation_id,
            conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData::from(&dto),
        );

        // Re-process mentions: delete old, create new.
        let _ = self
            .repos
            .message_mention_repo
            .delete_mentions_for_message(cmd.message_id)
            .await;
        self.process_mentions(
            dto.text.as_deref(),
            dto.id,
            cmd.server_id,
            cmd.conversation_id,
            cmd.actor.user_id,
        )
        .await;

        Ok(dto)
    }

    #[instrument(
        name = "message.forward_message",
        skip_all,
        fields(
            actor_id = %cmd.actor.user_id,
            server_id = %cmd.server_id,
            source_conversation_id = %cmd.source_conversation_id,
            target_conversation_id = %cmd.target_conversation_id,
            message_id = %cmd.message_id
        )
    )]
    async fn forward_message(&self, cmd: ForwardMessageCmd) -> ServiceResult<MessageDto> {
        // Verify membership in workspace
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // Verify access to source conversation
        let source_conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.source_conversation_id,
            cmd.server_id,
        )
        .await?;
        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &source_conv,
            cmd.actor.user_id,
        )
        .await?;

        // Verify access to target conversation
        let target_conv = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.target_conversation_id,
            cmd.server_id,
        )
        .await?;
        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &target_conv,
            cmd.actor.user_id,
        )
        .await?;

        // Get the source message
        let source_msg = self
            .repos
            .message_repo
            .get_by_id(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?
            .ok_or(ServiceError::NotFound)?;

        if source_msg.conversation_id != cmd.source_conversation_id {
            return Err(ServiceError::NotFound);
        }

        if source_msg.is_deleted {
            return Err(ServiceError::Validation(
                "cannot forward a deleted message".to_string(),
            ));
        }

        // Create the forwarded message in the target conversation
        let new_message = NewMessage {
            conversation_id: cmd.target_conversation_id,
            text: source_msg.text.clone(),
            sent_by: cmd.actor.user_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: Some(cmd.message_id),
            attachments: vec![],
        };

        let (repos, scope) = self.begin_write().await?;

        let message = repos
            .message_repo
            .create_message(&new_message)
            .await
            .map_err(rim_services::map_repo_error)?;

        repos
            .conversation_repo
            .touch_last_activity_at(cmd.target_conversation_id, message.sent_at)
            .await
            .map_err(rim_services::map_repo_error)?;

        Self::commit_if_present(scope).await?;

        info!(
            message_id = %message.id,
            forwarded_from = %cmd.message_id,
            "forwarded message"
        );

        let dto = self.to_dto(message, cmd.server_id).await?;

        let member_user_ids = self
            .list_member_user_ids(cmd.target_conversation_id)
            .await?;

        self.observers.message.on_message_sent(
            cmd.server_id,
            dto.conversation_id,
            target_conv.conversation_type.as_str(),
            member_user_ids,
            MessageEventData::from(&dto),
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

        if msg.conversation_id != cmd.conversation_id {
            return Err(ServiceError::NotFound);
        }

        if msg.sent_by != cmd.actor.user_id {
            return Err(ServiceError::PermissionDenied);
        }

        let thread_id = msg.thread_id;

        self.repos
            .message_repo
            .soft_delete_message(cmd.message_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let member_user_ids = self.list_member_user_ids(cmd.conversation_id).await?;

        self.observers.message.on_message_deleted(
            cmd.server_id,
            cmd.conversation_id,
            conv.conversation_type.as_str(),
            member_user_ids,
            cmd.message_id,
            thread_id,
        );

        Ok(())
    }

    async fn get_messages_by_ids(
        &self,
        ids: &[rim_core::message::model::MessageId],
    ) -> ServiceResult<Vec<MessageDto>> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }

        let messages_map = self
            .repos
            .message_repo
            .get_by_ids(ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let messages: Vec<_> = messages_map.into_values().collect();

        // Batch-fetch all conversations to avoid N+1
        let conv_ids: Vec<_> = messages
            .iter()
            .map(|m| m.conversation_id)
            .collect::<std::collections::HashSet<_>>()
            .into_iter()
            .collect();
        let convs = self
            .repos
            .conversation_repo
            .get_by_ids(&conv_ids)
            .await
            .map_err(rim_services::map_repo_error)?;
        let conv_map: std::collections::HashMap<_, _> =
            convs.into_iter().map(|c| (c.id, c)).collect();

        let mut result = Vec::with_capacity(messages.len());
        for msg in messages {
            let server_id = conv_map
                .get(&msg.conversation_id)
                .map(|c| c.server_id)
                .ok_or_else(|| {
                    tracing::warn!(
                        message_id = %msg.id,
                        conversation_id = %msg.conversation_id,
                        "message references non-existent conversation"
                    );
                    ServiceError::NotFound
                })?;

            let dto = self.to_dto(msg, server_id).await?;
            result.push(dto);
        }

        Ok(result)
    }
}
