use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::conversation::model::ConversationType as CoreConversationType;
use rim_services::conversation::command::{
    AddConversationMemberCmd, CreateConversationCmd, GetConversationQuery, ListChannelsQuery,
    ListInboxConversationsQuery, RemoveConversationMemberCmd,
};
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::ConversationService;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::{ServiceError, ServiceResult};

use rim_core::repository::pagination::Page;
use rim_services::conversation::dto::{ChannelPageCursor, InboxPageCursor};

use crate::ConversationRepos;
use crate::util::{ensure_conversation_in_server, ensure_server_member};

#[derive(Clone)]
pub struct DefaultConversationService {
    repos: ConversationRepos,
}

impl DefaultConversationService {
    pub fn new(repos: ConversationRepos) -> Self {
        Self { repos }
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
            title: None,
            member_ids,
        })
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
        ensure_server_member(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;

        let (conv_type, member_ids): (CoreConversationType, Vec<rim_core::user::model::UserId>) =
            match cmd.participants {
                ConversationParticipants::DirectMessage { other_user_id } => {
                    if other_user_id == cmd.actor.user_id {
                        return Err(ServiceError::Validation(
                            "cannot create DM with yourself".to_string(),
                        ));
                    }

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
                    (CoreConversationType::Group, unique)
                }
                ConversationParticipants::Channel => {
                    // v0: channels have no extra metadata here; just create and add creator.
                    (CoreConversationType::Channel, vec![cmd.actor.user_id])
                }
            };

        let conversation = self
            .repos
            .conversation_repo
            .create_conversation(conv_type, cmd.server_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(
            conversation_id = %conversation.id,
            conversation_type = ?conversation.conversation_type,
            "created conversation"
        );

        let member_count = member_ids.len();

        for user_id in member_ids {
            let _ = self
                .repos
                .conversation_member_repo
                .add_member(conversation.id, user_id)
                .await
                .map_err(rim_services::map_repo_error)?;
        }

        info!(
            conversation_id = %conversation.id,
            member_count,
            "initialized conversation members"
        );

        self.to_dto(conversation).await
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

        let mut items = Vec::with_capacity(page.items.len());
        for conv in page.items {
            items.push(self.to_dto(conv).await?);
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

        let mut items = Vec::with_capacity(page.items.len());
        for conv in page.items {
            items.push(self.to_dto(conv).await?);
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

        self.repos
            .conversation_member_repo
            .add_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.to_dto(conversation).await
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

        let is_member = self
            .repos
            .conversation_member_repo
            .is_member(cmd.conversation_id, cmd.actor.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;
        if !is_member {
            return Err(ServiceError::PermissionDenied);
        }

        self.repos
            .conversation_member_repo
            .remove_member(cmd.conversation_id, cmd.user_id)
            .await
            .map_err(rim_services::map_repo_error)?;

        self.to_dto(conversation).await
    }
}
