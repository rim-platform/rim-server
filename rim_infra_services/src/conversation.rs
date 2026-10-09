use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::conversation::model::{
    Conversation, ConversationType as CoreConversationType, NewConversation,
};
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_services::conversation::command::{
    AddConversationMembersCmd, ArchiveConversationCmd, CreateConversationCmd, GetConversationQuery,
    JoinChannelCmd, LeaveConversationCmd, ListChannelsQuery, ListInboxConversationsQuery,
    RemoveConversationMemberCmd, UpdateConversationTopicCmd,
};
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::ConversationService;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::{ServiceError, ServiceResult};

use rim_core::repository::pagination::Page;
use rim_services::conversation::dto::{ChannelPageCursor, InboxPageCursor};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::ConversationRepos;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_access, ensure_conversation_access_as,
    ensure_conversation_in_server, ensure_conversation_member_access, ensure_conversation_writable,
    ensure_not_archived, ensure_not_timed_out, ensure_server_member,
    ensure_server_owner_or_moderator, ensure_workspace_writable, member_standing,
};

/// The refusal of a member who cannot read an encrypted conversation.
fn not_ready() -> ServiceError {
    ServiceError::Validation(
        "every member of an encrypted conversation needs a device set up for encryption"
            .to_string(),
    )
}

#[derive(Clone)]
pub struct DefaultConversationService {
    repos: ConversationRepos,
    observers: Observers,
    /// Readiness of members for encrypted conversations. `None` in unit tests that are not about
    /// them; the composition root wires it.
    e2ee_repo: Option<Arc<dyn E2eeRepository>>,
    /// The relay's direct space, where everyone is a guest, guests start DMs and groups, and
    /// there are no channels.
    direct_space_id: Option<rim_core::server::model::ServerId>,
}

impl DefaultConversationService {
    pub fn new(repos: ConversationRepos, observers: Observers) -> Self {
        Self {
            repos,
            observers,
            e2ee_repo: None,
            direct_space_id: None,
        }
    }

    pub fn with_direct_space(mut self, direct_space_id: rim_core::server::model::ServerId) -> Self {
        self.direct_space_id = Some(direct_space_id);
        self
    }

    fn is_direct_space(&self, server_id: rim_core::server::model::ServerId) -> bool {
        self.direct_space_id == Some(server_id)
    }

    /// Make users members of the direct space, as guests, when they are not yet.
    async fn join_direct_space(
        &self,
        server_id: rim_core::server::model::ServerId,
        user_ids: &[UserId],
    ) -> ServiceResult<()> {
        for &user_id in user_ids {
            if self
                .repos
                .server_member_repo
                .is_member(server_id, user_id)
                .await?
            {
                continue;
            }
            match self
                .repos
                .server_member_repo
                .add_member(server_id, user_id, ServerMemberRole::Guest)
                .await
            {
                Ok(_) => {}
                Err(err) if matches!(err.kind(), RepoErrorKind::Conflict) => {}
                Err(err) if matches!(err.kind(), RepoErrorKind::NotFound) => {
                    return Err(ServiceError::NotFound);
                }
                Err(err) => return Err(err.into()),
            }
        }
        Ok(())
    }

    pub fn with_e2ee_repo(mut self, e2ee_repo: Arc<dyn E2eeRepository>) -> Self {
        self.e2ee_repo = Some(e2ee_repo);
        self
    }

    /// `Validation` unless every one of `user_ids` is ready for encryption: a principal and a
    /// device that uploaded key packages within the activity window.
    async fn ensure_ready(&self, user_ids: &[UserId]) -> ServiceResult<()> {
        let e2ee_repo = self.e2ee_repo.as_ref().ok_or_else(|| {
            ServiceError::internal(anyhow::anyhow!("encryption is not configured"))
        })?;
        let ready: HashSet<UserId> = e2ee_repo
            .ready_users(user_ids, crate::e2ee::active_since(chrono::Utc::now()))
            .await?
            .into_iter()
            .collect();
        if user_ids.iter().all(|id| ready.contains(id)) {
            Ok(())
        } else {
            Err(not_ready())
        }
    }

    /// Tell an encrypted conversation's members that its group should add or remove devices.
    fn emit_mls_membership_changed(&self, conversation: &Conversation, member_ids: Vec<UserId>) {
        if conversation.is_encrypted() {
            self.observers.emit(WorkspaceEvent::MlsMembershipChanged {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                member_user_ids: member_ids,
            });
        }
    }

    async fn to_dto(&self, conversation: Conversation) -> ServiceResult<ConversationDto> {
        let member_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await?;

        Ok(self.to_dto_with_members(conversation, member_ids))
    }

    fn to_dto_with_members(
        &self,
        conversation: Conversation,
        member_ids: Vec<UserId>,
    ) -> ConversationDto {
        ConversationDto {
            id: conversation.id,
            server_id: conversation.server_id,
            conversation_type: conversation.conversation_type,
            title: conversation.title,
            topic: conversation.topic,
            member_ids,
            is_private: conversation.is_private,
            archived_at: conversation.archived_at,
            created_by: conversation.created_by,
            slow_mode_seconds: conversation.slow_mode_seconds,
            encrypted_at: conversation.encrypted_at,
        }
    }

    /// Gate for joining and inviting (Track 1.2): role first, so a guest learns nothing about
    /// which conversation ids exist; then the timeout; then the usual read gate.
    async fn ensure_may_add_members(
        &self,
        actor: UserId,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<Conversation> {
        let standing =
            member_standing(self.repos.server_member_repo.as_ref(), server_id, actor).await?;
        if standing.role.is_guest() && !self.is_direct_space(server_id) {
            return Err(ServiceError::PermissionDenied);
        }
        ensure_not_timed_out(&standing)?;
        let conversation = ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            conversation_id,
            server_id,
        )
        .await?;
        ensure_conversation_access_as(
            self.repos.conversation_member_repo.as_ref(),
            &conversation,
            actor,
            standing.role,
        )
        .await?;
        Ok(conversation)
    }

    fn access_repos(&self) -> ConversationAccessRepos<'_> {
        ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        )
    }

    async fn is_owner_or_moderator(
        &self,
        server_id: rim_core::server::model::ServerId,
        user_id: UserId,
    ) -> ServiceResult<bool> {
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, user_id)
            .await?;
        Ok(member.is_some_and(|m| {
            matches!(
                m.member_role,
                ServerMemberRole::Owner | ServerMemberRole::Moderator
            )
        }))
    }

    /// Adds `user_id` unless already a member. Returns whether a row was inserted; a concurrent
    /// insert that loses the unique-index race counts as "already a member".
    async fn add_member_if_absent(
        &self,
        conversation_id: rim_core::conversation::model::ConversationId,
        user_id: UserId,
    ) -> ServiceResult<bool> {
        if self
            .repos
            .conversation_member_repo
            .is_member(conversation_id, user_id)
            .await?
        {
            return Ok(false);
        }
        match self
            .repos
            .conversation_member_repo
            .add_member(conversation_id, user_id)
            .await
        {
            Ok(_) => Ok(true),
            Err(err) if matches!(err.kind(), RepoErrorKind::Conflict) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Removes `user_id` from the conversation and tells the remaining members and the removed
    /// user (whose client must drop the conversation).
    async fn remove_member_and_notify(
        &self,
        conversation: &Conversation,
        user_id: UserId,
    ) -> ServiceResult<()> {
        self.repos
            .conversation_member_repo
            .remove_member(conversation.id, user_id)
            .await?;

        let mut notify_user_ids = self
            .repos
            .conversation_member_repo
            .list_member_ids(conversation.id)
            .await?;
        if !notify_user_ids.contains(&user_id) {
            notify_user_ids.push(user_id);
        }
        self.observers
            .emit(WorkspaceEvent::MemberRemovedFromConversation {
                workspace_id: conversation.server_id,
                conversation_id: conversation.id,
                user_id,
                member_user_ids: notify_user_ids.clone(),
            });
        let remaining = notify_user_ids
            .into_iter()
            .filter(|id| *id != user_id)
            .collect();
        self.emit_mls_membership_changed(conversation, remaining);
        Ok(())
    }

    fn emit_conversation_updated(&self, dto: &ConversationDto) {
        self.observers.emit(WorkspaceEvent::ConversationUpdated {
            workspace_id: dto.server_id,
            conversation_id: dto.id,
            conversation_type: dto.conversation_type.as_str().to_string(),
            is_private: dto.is_private,
            member_user_ids: dto.member_ids.clone(),
        });
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

        if cmd.is_private && cmd.participants != ConversationParticipants::Channel {
            return Err(ServiceError::Validation(
                "only channels can be private".to_string(),
            ));
        }
        if cmd.encrypted && cmd.participants == ConversationParticipants::Channel {
            return Err(ServiceError::Validation(
                "only direct messages and groups are encrypted".to_string(),
            ));
        }

        let direct = self.is_direct_space(cmd.server_id);
        if direct {
            if cmd.participants == ConversationParticipants::Channel {
                return Err(ServiceError::Validation(
                    "the direct space has no channels".to_string(),
                ));
            }
            let mut joining = vec![cmd.actor.user_id];
            match &cmd.participants {
                ConversationParticipants::DirectMessage { other_user_id } => {
                    joining.push(*other_user_id);
                }
                ConversationParticipants::Group { member_ids } => {
                    joining.extend(member_ids.iter().copied());
                }
                ConversationParticipants::Channel => {}
            }
            self.join_direct_space(cmd.server_id, &joining).await?;
        }

        // Member, not timed out; guests may only open DMs (Track 1.2), except in the direct
        // space, where everyone is a guest.
        let standing = ensure_workspace_writable(
            self.repos.server_member_repo.as_ref(),
            cmd.server_id,
            cmd.actor.user_id,
        )
        .await?;
        if standing.role.is_guest()
            && !direct
            && !matches!(
                cmd.participants,
                ConversationParticipants::DirectMessage { .. }
            )
        {
            return Err(ServiceError::PermissionDenied);
        }

        let (conv_type, member_ids): (CoreConversationType, Vec<UserId>) = match cmd.participants {
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
                    .await?;

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
                    .collect::<HashSet<_>>()
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
                        .await?;

                    if exists {
                        return Err(ServiceError::Conflict(
                            "A channel with this name already exists".to_string(),
                        ));
                    }
                }

                // The creator is always a member of the channel, public or private.
                (CoreConversationType::Channel, vec![cmd.actor.user_id])
            }
        };

        let member_count = member_ids.len();
        if cmd.encrypted {
            if member_count > rim_services::e2ee::MAX_ENCRYPTED_MEMBERS {
                return Err(ServiceError::Validation(format!(
                    "an encrypted conversation has at most {} members",
                    rim_services::e2ee::MAX_ENCRYPTED_MEMBERS
                )));
            }
            self.ensure_ready(&member_ids).await?;
        }

        let conversation = self
            .repos
            .conversation_repo
            .create_conversation_with_members(
                NewConversation {
                    server_id: cmd.server_id,
                    conversation_type: conv_type,
                    title,
                    is_private: cmd.is_private,
                    created_by: Some(cmd.actor.user_id),
                    encrypted: cmd.encrypted,
                },
                &member_ids,
            )
            .await?;

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

        self.observers.emit(WorkspaceEvent::ConversationCreated {
            workspace_id: dto.server_id,
            conversation_id: dto.id,
            title: dto.title.clone(),
            conversation_type: dto.conversation_type.as_str().to_string(),
            member_user_ids: dto.member_ids.clone(),
            is_private: dto.is_private,
        });

        Ok(dto)
    }

    async fn get_conversation(
        &self,
        query: GetConversationQuery,
    ) -> ServiceResult<ConversationDto> {
        let conversation = ensure_conversation_member_access(
            self.access_repos(),
            query.actor.user_id,
            query.server_id,
            query.conversation_id,
        )
        .await?;

        self.to_dto(conversation).await
    }

    async fn get_conversations_by_ids(
        &self,
        ids: &[rim_core::conversation::model::ConversationId],
    ) -> ServiceResult<Vec<ConversationDto>> {
        let conversations = self.repos.conversation_repo.get_by_ids(ids).await?;

        let conv_ids: Vec<_> = conversations.iter().map(|c| c.id).collect();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conv_ids)
            .await?;

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
            .await?;

        let conversation_ids = page.items.iter().map(|c| c.id).collect::<Vec<_>>();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conversation_ids)
            .await?;

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
        let standing = member_standing(
            self.repos.server_member_repo.as_ref(),
            query.server_id,
            query.actor.user_id,
        )
        .await?;
        // Guests cannot browse: they only ever list the channels they were added to.
        let mut filter = query.filter;
        if standing.role.is_guest() {
            filter.joined_only = true;
        }

        let page = self
            .repos
            .conversation_repo
            .list_channels_for_user(
                query.actor.user_id,
                query.server_id,
                filter,
                query.limit,
                query.dir,
                query.cursor.map(Into::into),
            )
            .await?;

        let conversation_ids = page.items.iter().map(|c| c.id).collect::<Vec<_>>();
        let member_ids_by_conversation = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&conversation_ids)
            .await?;

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

    #[instrument(
        name = "conversation.add_conversation_members",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn add_conversation_members(
        &self,
        cmd: AddConversationMembersCmd,
    ) -> ServiceResult<ConversationDto> {
        if cmd.user_ids.is_empty() {
            return Err(ServiceError::Validation(
                "at least one user id is required".to_string(),
            ));
        }

        // Guests cannot invite and timed-out members cannot write (Track 1.2). Private
        // channels and groups: the gate already requires membership (NotFound /
        // PermissionDenied otherwise).
        let conversation = self
            .ensure_may_add_members(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        if conversation.conversation_type == CoreConversationType::Direct {
            return Err(ServiceError::Validation(
                "cannot add members to a direct message".to_string(),
            ));
        }
        ensure_not_archived(&conversation)?;

        // Public channels: anyone can read them, but only members invite others (join first).
        if !self
            .repos
            .conversation_member_repo
            .is_member(conversation.id, cmd.actor.user_id)
            .await?
        {
            return Err(ServiceError::PermissionDenied);
        }

        let mut seen = HashSet::new();
        let targets: Vec<UserId> = cmd
            .user_ids
            .into_iter()
            .filter(|id| seen.insert(*id))
            .collect();
        if self.is_direct_space(cmd.server_id) {
            self.join_direct_space(cmd.server_id, &targets).await?;
        }
        for &user_id in &targets {
            if !self
                .repos
                .server_member_repo
                .is_member(cmd.server_id, user_id)
                .await?
            {
                return Err(ServiceError::Validation(format!(
                    "user {user_id} is not a member of this workspace"
                )));
            }
        }
        if conversation.is_encrypted() {
            let current = self
                .repos
                .conversation_member_repo
                .list_member_ids(conversation.id)
                .await?;
            let joining: Vec<UserId> = targets
                .iter()
                .copied()
                .filter(|id| !current.contains(id))
                .collect();
            if current.len() + joining.len() > rim_services::e2ee::MAX_ENCRYPTED_MEMBERS {
                return Err(ServiceError::Validation(format!(
                    "an encrypted conversation has at most {} members",
                    rim_services::e2ee::MAX_ENCRYPTED_MEMBERS
                )));
            }
            self.ensure_ready(&joining).await?;
        }

        let mut added = Vec::new();
        for user_id in targets {
            if self.add_member_if_absent(conversation.id, user_id).await? {
                added.push(user_id);
            }
        }

        info!(added = added.len(), "added conversation members");

        let encrypted = conversation.clone();
        let dto = self.to_dto(conversation).await?;
        if !added.is_empty() {
            self.emit_mls_membership_changed(&encrypted, dto.member_ids.clone());
        }
        for user_id in added {
            self.observers
                .emit(WorkspaceEvent::MemberAddedToConversation {
                    workspace_id: dto.server_id,
                    conversation_id: dto.id,
                    user_id,
                    member_user_ids: dto.member_ids.clone(),
                });
        }

        Ok(dto)
    }

    #[instrument(
        name = "conversation.remove_conversation_member",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, user_id = %cmd.user_id)
    )]
    async fn remove_conversation_member(
        &self,
        cmd: RemoveConversationMemberCmd,
    ) -> ServiceResult<()> {
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

        // Owner/Moderator may remove anyone, even from a private channel they are not in (the
        // same rule as archiving an orphaned private channel by id). Everyone else must be able
        // to see the conversation and may only remove themselves.
        if !self
            .is_owner_or_moderator(cmd.server_id, cmd.actor.user_id)
            .await?
        {
            ensure_conversation_access(
                self.repos.conversation_member_repo.as_ref(),
                &conversation,
                cmd.actor.user_id,
            )
            .await?;
            if cmd.user_id != cmd.actor.user_id {
                return Err(ServiceError::PermissionDenied);
            }
        }

        if conversation.conversation_type == CoreConversationType::Direct {
            return Err(ServiceError::Validation(
                "cannot remove members from a direct message".to_string(),
            ));
        }

        if !self
            .repos
            .conversation_member_repo
            .is_member(conversation.id, cmd.user_id)
            .await?
        {
            return Err(ServiceError::NotFound);
        }

        self.remove_member_and_notify(&conversation, cmd.user_id)
            .await?;
        info!("removed conversation member");
        Ok(())
    }

    #[instrument(
        name = "conversation.join_channel",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn join_channel(&self, cmd: JoinChannelCmd) -> ServiceResult<ConversationDto> {
        // Guests cannot join (Track 1.2); a timed-out member cannot either. A private channel
        // the actor is not in is NotFound here, so it cannot be joined.
        let conversation = self
            .ensure_may_add_members(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        if conversation.conversation_type != CoreConversationType::Channel {
            return Err(ServiceError::Validation(
                "only channels can be joined".to_string(),
            ));
        }
        ensure_not_archived(&conversation)?;

        let joined = self
            .add_member_if_absent(conversation.id, cmd.actor.user_id)
            .await?;

        let dto = self.to_dto(conversation).await?;
        if joined {
            info!("joined channel");
            self.observers
                .emit(WorkspaceEvent::MemberAddedToConversation {
                    workspace_id: dto.server_id,
                    conversation_id: dto.id,
                    user_id: cmd.actor.user_id,
                    member_user_ids: dto.member_ids.clone(),
                });
        }

        Ok(dto)
    }

    #[instrument(
        name = "conversation.leave_conversation",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn leave_conversation(&self, cmd: LeaveConversationCmd) -> ServiceResult<()> {
        let conversation = ensure_conversation_member_access(
            self.access_repos(),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;

        if conversation.conversation_type == CoreConversationType::Direct {
            return Err(ServiceError::Validation(
                "cannot leave a direct message".to_string(),
            ));
        }

        // Leaving a public channel you never joined is a no-op, mirroring `join_channel`.
        if !self
            .repos
            .conversation_member_repo
            .is_member(conversation.id, cmd.actor.user_id)
            .await?
        {
            return Ok(());
        }

        // Leaving a private channel as its last member is allowed; it then stays invisible to
        // everyone, and only an Owner/Moderator can archive it, by id.
        self.remove_member_and_notify(&conversation, cmd.actor.user_id)
            .await?;
        info!("left conversation");
        Ok(())
    }

    #[instrument(
        name = "conversation.set_conversation_archived",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id, archived = cmd.archived)
    )]
    async fn set_conversation_archived(
        &self,
        cmd: ArchiveConversationCmd,
    ) -> ServiceResult<ConversationDto> {
        // Role first: a non-moderator learns nothing about which ids exist.
        ensure_server_owner_or_moderator(
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

        if conversation.conversation_type != CoreConversationType::Channel {
            return Err(ServiceError::Validation(
                "only channels can be archived".to_string(),
            ));
        }

        if conversation.is_archived() == cmd.archived {
            return self.to_dto(conversation).await;
        }

        let updated = self
            .repos
            .conversation_repo
            .set_archived(conversation.id, cmd.archived.then(chrono::Utc::now))
            .await?;
        info!("changed conversation archive state");

        let dto = self.to_dto(updated).await?;
        self.emit_conversation_updated(&dto);
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
        ensure_conversation_writable(
            self.access_repos(),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
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
            .await?;

        info!(
            conversation_id = %updated.id,
            "updated conversation topic"
        );

        let dto = self.to_dto(updated).await?;
        self.emit_conversation_updated(&dto);
        Ok(dto)
    }
}
