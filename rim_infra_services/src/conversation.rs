use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::conversation::model::ConversationType as CoreConversationType;
use rim_services::conversation::command::{
    AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery, ListChannelsQuery,
    ListInboxConversationsQuery, RemoveConversationMemberCmd, UpdateConversationTopicCmd,
};
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::ConversationService;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::{ServiceError, ServiceResult};

use rim_core::repository::pagination::Page;
use rim_services::conversation::dto::{ChannelPageCursor, InboxPageCursor};

use rim_services::events::Observers;

use crate::ConversationRepos;
use crate::util::{ensure_conversation_access, ensure_conversation_in_server, ensure_server_member};

#[derive(Clone)]
pub struct DefaultConversationService {
    repos: ConversationRepos,
    observers: Observers,
}

impl DefaultConversationService {
    pub fn new(repos: ConversationRepos, observers: Observers) -> Self {
        Self { repos, observers }
    }

    async fn to_dto(
        &self,
        conversation: rim_core::conversation::model::Conversation,
    ) -> ServiceResult<ConversationDto> {
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(ConversationDto {
            id: conversation.id,
            server_id: conversation.server_id,
            conversation_type: conversation.conversation_type,
            title: conversation.title,
            topic: conversation.topic,
            member_ids,
        })
    }

    fn to_dto_with_members(
        &self,
        conversation: rim_core::conversation::model::Conversation,
        member_ids: Vec<rim_core::user::model::UserId>,
    ) -> ConversationDto {
        ConversationDto {
            id: conversation.id,
            server_id: conversation.server_id,
            conversation_type: conversation.conversation_type,
            title: conversation.title,
            topic: conversation.topic,
            member_ids,
        }
    }

    fn normalize_title(title: Option<String>) -> ServiceResult<Option<String>> {
        match title {
            Some(title) => {
                let trimmed = title.trim();
                if trimmed.is_empty() {
                    Ok(None)
                } else if trimmed.chars().count() > 100 {
                    Err(ServiceError::Validation(
                        "title must be at most 100 characters".to_string(),
                    ))
                } else {
                    Ok(Some(trimmed.to_string()))
                }
            }
            None => Ok(None),
        }
    }
}

#[async_trait]
impl ConversationService for DefaultConversationService {
    #[instrument(
        name = "conversation.create_conversation",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id)
    )]
    async fn create_conversation(
        &self,
        cmd: CreateConversationCmd,
    ) -> ServiceResult<ConversationDto> {
        let title = Self::normalize_title(cmd.title)?;

        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let (conv_type, member_ids): (CoreConversationType, Vec<rim_core::user::model::UserId>) =
            match cmd.participants {
                ConversationParticipants::DirectMessage { other_user_id } => {
                    if title.is_some() {
                        return Err(ServiceError::Validation(
                            "direct conversations cannot have titles".to_string(),
                        ));
                    }

                    if other_user_id == cmd.actor.user_id {
                        return Err(ServiceError::Validation(
                            "cannot create DM with yourself".to_string(),
                        ));
                    }

                    // Verify the other user is a server member.
                    ensure_server_member(
                        self.repos.server_member_repo.as_ref(),
                        cmd.server_id,
                        other_user_id,
                    )
                    .await?;

                    let existing = self
                        .repos
                        .conversation_repo
                        .find_direct_between_users(cmd.server_id, cmd.actor.user_id, other_user_id)
                        .await
                        .map_err(rim_services::map_repo_error)?;

                    if let Some(existing) = existing {
                        return self.to_dto(existing).await;
                    }

                    (
                        CoreConversationType::Direct,
                        vec![cmd.actor.user_id, other_user_id],
                    )
                }
                ConversationParticipants::Group { mut member_ids } => {
                    if !member_ids.contains(&cmd.actor.user_id) {
                        member_ids.push(cmd.actor.user_id);
                    }
                    let unique = member_ids
                        .into_iter()
                        .collect::<std::collections::HashSet<_>>()
                        .into_iter()
                        .collect::<Vec<_>>();

                    // Verify all members are server members.
                    for &uid in &unique {
                        ensure_server_member(
                            self.repos.server_member_repo.as_ref(),
                            cmd.server_id,
                            uid,
                        )
                        .await?;
                    }

                    if unique.len() < 2 {
                        return Err(ServiceError::Validation(
                            "group conversations require at least 2 members".to_string(),
                        ));
                    }

                    (CoreConversationType::Group, unique)
                }
                ConversationParticipants::Channel => {
                    if let Some(ref channel_title) = title {
                        let exists = self
                            .repos
                            .conversation_repo
                            .exists_channel_by_title(cmd.server_id, channel_title)
                            .await
                            .map_err(rim_services::map_repo_error)?;

                        if exists {
                            return Err(ServiceError::Conflict(
                                "A channel with this name already exists".to_string(),
                            ));
                        }
                    }

                    (CoreConversationType::Channel, vec![cmd.actor.user_id])
                }
            };

        let member_count = member_ids.len();

        let conversation = self
            .repos
            .conversation_repo
            .create_conversation_with_members(conv_type, cmd.server_id, title, &member_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %conversation.id,
            conversation_type = ?conversation.conversation_type,
            "created conversation"
        );

        info!(
            conversation_id = %conversation.id,
            member_count,
            "initialized conversation members"
        );

        let dto = self.to_dto(conversation).await?;

        self.observers.workspace.on_conversation_created(
            dto.server_id,
            dto.id,
            dto.title.as_deref(),
            dto.conversation_type.as_str(),
            dto.member_ids.clone(),
        );

        Ok(dto)
    }

    async fn get_conversation(
        &self,
        query: GetConversationQuery,
    ) -> ServiceResult<ConversationDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            query.conversation_id,
            query.server_id,
        )
        .await?;

        // Membership in the conversation.
        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(query.conversation_id, query.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }

        self.to_dto(conversation).await
    }

    async fn get_conversations_by_ids(
        &self,
        ids: &[rim_core::conversation::model::ConversationId],
    ) -> ServiceResult<Vec<ConversationDto>> {
        let conversations = self
            .repos
            .conversation_repo
            .get_by_ids(ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let conv_ids: Vec<_> = conversations.iter().map(|c| c.id).collect();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conv_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        Ok(conversations
            .into_iter()
            .map(|conv| {
                let member_ids = member_ids_by_conversation
                    .get(&conv.id)
                    .cloned()
                    .unwrap_or_default();
                self.to_dto_with_members(conv, member_ids)
            })
            .collect())
    }

    async fn list_inbox_conversations(
        &self,
        query: ListInboxConversationsQuery,
    ) -> ServiceResult<Page<ConversationDto, InboxPageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let page = self
            .repos
            .conversation_repo
            .list_inbox_conversations_for_user(
                query.actor.user_id,
                query.server_id,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        let conversation_ids = page.items.iter().map(|c| c.id).collect::<Vec<_>>();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conversation_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for conv in page.items {
            let member_ids = member_ids_by_conversation
                .get(&conv.id)
                .cloned()
                .unwrap_or_default();
            items.push(self.to_dto_with_members(conv, member_ids));
        }

        Ok(Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn list_channels(
        &self,
        query: ListChannelsQuery,
    ) -> ServiceResult<Page<ConversationDto, ChannelPageCursor>> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;

        let page = self
            .repos
            .conversation_repo
            .list_channels_for_user(
                query.actor.user_id,
                query.server_id,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
            )
            .await
            .map_err(rim_services::map_repo_error)?;

        let conversation_ids = page.items.iter().map(|c| c.id).collect::<Vec<_>>();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conversation_ids)
            .await
            .map_err(rim_services::map_repo_error)?;

        let mut items = Vec::with_capacity(page.items.len());
        for conv in page.items {
            let member_ids = member_ids_by_conversation
                .get(&conv.id)
                .cloned()
                .unwrap_or_default();
            items.push(self.to_dto_with_members(conv, member_ids));
        }

        Ok(Page::new(
            items,
            page.next.map(Into::into),
            page.prev.map(Into::into),
        ))
    }

    async fn add_member(&self, cmd: AddConversationMemberCmd) -> ServiceResult<ConversationDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        // v0: allow any member to add members only for Group conversations.
        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        if conversation.conversation_type != CoreConversationType::Group {
            return Err(ServiceError::Validation(
                "can only add members to group conversations".to_string(),
            ));
        }

        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(cmd.conversation_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }

        // Check that the target user is a server member.
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.user_id,
        )
        .await?;

        // Check that the target user is not already a conversation member.
        let already_member = self
            .repos
            .conversation_member_repo
            .is_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if already_member {
            return Err(ServiceError::Conflict(
                "user is already a member of this conversation".to_string(),
            ));
        }

        self.repos
            .conversation_member_repo
            .add_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let dto = self.to_dto(conversation).await?;

        self.observers.workspace.on_member_added_to_conversation(
            cmd.server_id,
            cmd.conversation_id,
            cmd.user_id,
            dto.member_ids.clone(),
        );

        Ok(dto)
    }

    async fn remove_member(
        &self,
        cmd: RemoveConversationMemberCmd,
    ) -> ServiceResult<ConversationDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        if conversation.conversation_type == CoreConversationType::Direct {
            return Err(ServiceError::Validation(
                "cannot remove members from a direct message".to_string(),
            ));
        }

        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(cmd.conversation_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }

        // Check that the target user is actually a member of the conversation.
        let target_is_member = self
            .repos
            .conversation_member_repo
            .is_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !target_is_member {
            return Err(ServiceError::NotFound);
        }

        self.repos
            .conversation_member_repo
            .remove_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        let dto = self.to_dto(conversation).await?;

        // Include both remaining members and the removed user so that the removed
        // user's client also receives the event.
        let mut notify_user_ids = dto.member_ids.clone();
        if !notify_user_ids.contains(&cmd.user_id) {
            notify_user_ids.push(cmd.user_id);
        }
        self.observers
            .workspace
            .on_member_removed_from_conversation(
                cmd.server_id,
                cmd.conversation_id,
                cmd.user_id,
                notify_user_ids,
            );

        Ok(dto)
    }

    #[instrument(
        name = "conversation.update_topic",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn update_topic(
        &self,
        cmd: UpdateConversationTopicCmd,
    ) -> ServiceResult<ConversationDto> {
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            cmd.conversation_id,
            cmd.server_id,
        )
        .await?;

        ensure_conversation_access(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            cmd.actor.user_id,
        )
        .await?;

        // Validate topic
        let topic = match cmd.topic {
            Some(t) => {
                let trimmed: String = t
                    .chars()
                    .filter(|c| !c.is_control() || *c == '\n')
                    .collect::<String>()
                    .trim()
                    .to_string();
                if trimmed.is_empty() {
                    None
                } else if trimmed.chars().count() > 512 {
                    return Err(ServiceError::Validation(
                        "topic must be at most 512 characters".to_string(),
                    ));
                } else {
                    Some(trimmed)
                }
            }
            None => None,
        };

        let updated = self
            .repos
            .conversation_repo
            .update_topic(cmd.conversation_id, topic)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %updated.id,
            "updated conversation topic"
        );

        let dto = self.to_dto(updated).await?;

        self.observers
            .workspace
            .on_conversation_updated(dto.server_id, dto.id);

        Ok(dto)
    }
}
