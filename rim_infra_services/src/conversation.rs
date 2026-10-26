use std::collections::HashSet;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, instrument};

use rim_core::conversation::model::{
    Conversation, ConversationId, ConversationType as CoreConversationType, NewConversation,
};
use rim_core::conversation_member::model::Membership;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::user::model::UserId;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::conversation::command::{
    AddConversationMembersCmd, ArchiveConversationCmd, ConversationInviteCmd,
    CreateConversationCmd, GetConversationQuery, JoinChannelCmd, LeaveConversationCmd,
    ListChannelsQuery, ListInboxConversationsQuery, RemoveConversationMemberCmd,
    UpdateConversationTopicCmd,
};
use rim_services::conversation::dto::ConversationDto;
use rim_services::conversation::service::ConversationService;
use rim_services::conversation::types::ConversationParticipants;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::host::service::HostService;
use rim_services::limits::{InstanceLimits, ensure_below};
use rim_services::{ServiceError, ServiceResult};

use rim_core::repository::pagination::Page;
use rim_services::conversation::dto::{ChannelPageCursor, InboxPageCursor};

use rim_services::events::{EventObserver, Observers, WorkspaceEvent};

use crate::ConversationRepos;
use crate::quota::DailyCounter;
use crate::util::{
    ConversationAccessRepos, ensure_conversation_access_as, ensure_conversation_in_server,
    ensure_conversation_member_access, ensure_conversation_writable, ensure_not_archived,
    ensure_not_timed_out, ensure_server_member, ensure_server_owner_or_moderator,
    ensure_workspace_features, ensure_workspace_writable, member_standing,
};

/// Most conversations one user starts in the direct space per day.
pub(crate) const MAX_DIRECT_SPACE_CONVERSATIONS_PER_DAY: u64 = 50;

/// Users one call may name to start a group of the default workspace or to add to one of its
/// conversations.
pub(crate) const MAX_DEFAULT_WORKSPACE_IDS_PER_CALL: usize = 50;
/// Members a plaintext group of the default workspace holds; any other group holds
/// [`rim_services::e2ee::MAX_ENCRYPTED_MEMBERS`].
pub(crate) const MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS: usize = 50;

/// Whether a member of `role` may manage `conversation`'s shared surfaces (its topic, others'
/// pins): anyone in a DM or group; in a channel an Owner or Moderator, or the creator of a
/// private channel.
pub(crate) fn manages_channel(
    conversation: &Conversation,
    role: ServerMemberRole,
    user_id: UserId,
) -> bool {
    conversation.conversation_type != CoreConversationType::Channel
        || role.is_moderator()
        || (conversation.is_private && conversation.created_by == Some(user_id))
}

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
    /// The cap on channels per workspace.
    limits: InstanceLimits,
    /// Conversations each user started in the direct space today.
    direct_space_starts: Arc<DailyCounter>,
    /// Which workspace every new account joins, where only its Owner and Moderators create
    /// channels; `None` where none is wired.
    default_workspace: Option<Arc<dyn DefaultWorkspaceService>>,
    /// Checks starting a group of the default workspace, or adding people to one, like a DM
    /// with each of them; `None` where the server runs no request inbox.
    request_gate: Option<Arc<dyn HostService>>,
}

impl DefaultConversationService {
    pub fn new(repos: ConversationRepos, observers: Observers) -> Self {
        Self {
            repos,
            observers,
            e2ee_repo: None,
            direct_space_id: None,
            limits: InstanceLimits::default(),
            direct_space_starts: Arc::new(DailyCounter::per_day(
                MAX_DIRECT_SPACE_CONVERSATIONS_PER_DAY,
            )),
            default_workspace: None,
            request_gate: None,
        }
    }

    /// Count one conversation `user_id` starts in the direct space.
    ///
    /// # Errors
    ///
    /// `Validation` once they started their most conversations there today.
    fn count_direct_space_start(&self, user_id: UserId) -> ServiceResult<()> {
        match self
            .direct_space_starts
            .check_and_record(&user_id.to_string(), chrono::Utc::now())
        {
            RateLimitDecision::Allowed => Ok(()),
            RateLimitDecision::Limited {
                retry_after_seconds,
            } => {
                crate::metrics::rate_limited("direct_space_conversations");
                Err(ServiceError::Validation(format!(
                    "you can start at most {MAX_DIRECT_SPACE_CONVERSATIONS_PER_DAY} new \
                     conversations a day; try again in {} hours",
                    retry_after_seconds.div_ceil(3600)
                )))
            }
        }
    }

    pub fn with_request_gate(mut self, request_gate: Arc<dyn HostService>) -> Self {
        self.request_gate = Some(request_gate);
        self
    }

    /// `Ok` when `actor` may bring `user_ids` into a group of the default workspace `server_id`
    /// as the request inbox would let them start a DM with each: refused by a recipient's DM
    /// settings, past the request limits or in probation, it answers as that DM would.
    async fn ensure_may_request_group(
        &self,
        actor: UserId,
        server_id: rim_core::server::model::ServerId,
        user_ids: &[UserId],
    ) -> ServiceResult<()> {
        let Some(gate) = &self.request_gate else {
            return Ok(());
        };
        let others: Vec<UserId> = user_ids.iter().copied().filter(|id| *id != actor).collect();
        if others.is_empty() {
            return Ok(());
        }
        gate.ensure_may_request_group(rim_services::Actor::new(actor), server_id, others)
            .await
    }

    pub fn with_default_workspace(
        mut self,
        default_workspace: Arc<dyn DefaultWorkspaceService>,
    ) -> Self {
        self.default_workspace = Some(default_workspace);
        self
    }

    /// Whether `server_id` is the server's default workspace now.
    async fn is_default_workspace(
        &self,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<bool> {
        match &self.default_workspace {
            Some(default_workspace) => Ok(default_workspace.current().await? == Some(server_id)),
            None => Ok(false),
        }
    }

    /// `Validation` naming the cap when `server_id` already has as many unarchived channels as
    /// the instance allows.
    async fn ensure_channel_below_cap(
        &self,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<()> {
        if self.limits.channels_per_workspace.is_none() {
            return Ok(());
        }
        let channels = self
            .repos
            .conversation_repo
            .count_channels(server_id)
            .await?;
        ensure_below(
            self.limits.channels_per_workspace,
            channels,
            "channels per workspace",
        )
    }

    /// `Validation` when `server_id` already holds as many channels, archived ones included, as
    /// the instance allows; archiving does not free room toward this ceiling.
    async fn ensure_channels_below_ceiling(
        &self,
        server_id: rim_core::server::model::ServerId,
    ) -> ServiceResult<()> {
        let Some(ceiling) = self.limits.all_channels_per_workspace() else {
            return Ok(());
        };
        let channels = self
            .repos
            .conversation_repo
            .count_all_channels(server_id)
            .await?;
        if channels >= u64::from(ceiling) {
            return Err(ServiceError::Validation(format!(
                "this instance allows {ceiling} channels per workspace counting archived ones; \
                 archived channels cannot be deleted and still count toward it"
            )));
        }
        Ok(())
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    /// The relay's direct space, or `None` where none is wired.
    pub fn with_direct_space(
        mut self,
        direct_space_id: Option<rim_core::server::model::ServerId>,
    ) -> Self {
        self.direct_space_id = direct_space_id;
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

    /// Splits the users joining an encrypted conversation into those ready for encryption (a
    /// principal and a device that uploaded key packages within the activity window), who join
    /// active, and the rest, who join pending. `Validation` when one of the rest may not wait:
    /// a remote user, or anyone in the direct space.
    async fn split_by_readiness(
        &self,
        server_id: rim_core::server::model::ServerId,
        user_ids: &[UserId],
    ) -> ServiceResult<(Vec<UserId>, Vec<UserId>)> {
        let e2ee_repo = self.e2ee_repo()?;
        let ready = self.ready_users(user_ids).await?;
        let (active, pending): (Vec<UserId>, Vec<UserId>) =
            user_ids.iter().partition(|id| ready.contains(id));
        if pending.is_empty() {
            return Ok((active, pending));
        }
        if self.is_direct_space(server_id) || !e2ee_repo.remote_users(&pending).await?.is_empty() {
            return Err(not_ready());
        }
        self.ensure_invites_below_cap(&pending).await?;
        Ok((active, pending))
    }

    fn e2ee_repo(&self) -> ServiceResult<&Arc<dyn E2eeRepository>> {
        self.e2ee_repo
            .as_ref()
            .ok_or_else(|| ServiceError::internal(anyhow::anyhow!("encryption is not configured")))
    }

    /// The users among `user_ids` ready for encryption.
    async fn ready_users(&self, user_ids: &[UserId]) -> ServiceResult<HashSet<UserId>> {
        Ok(self
            .e2ee_repo()?
            .ready_users(user_ids, crate::e2ee::active_since(chrono::Utc::now()))
            .await?
            .into_iter()
            .collect())
    }

    /// `Validation` naming the cap when one of `user_ids` already holds as many pending
    /// invitations as the instance allows.
    async fn ensure_invites_below_cap(&self, user_ids: &[UserId]) -> ServiceResult<()> {
        let Some(cap) = self.limits.pending_invites_per_user else {
            return Ok(());
        };
        for &user_id in user_ids {
            let held = self
                .repos
                .conversation_member_repo
                .list_pending_conversation_ids(user_id)
                .await?
                .len();
            ensure_below(
                Some(cap),
                u64::try_from(held).unwrap_or(u64::MAX),
                "pending invitations per user",
            )?;
        }
        Ok(())
    }

    /// The conversation `user_id` holds a pending invitation to, as a workspace member.
    async fn invited_conversation(
        &self,
        server_id: rim_core::server::model::ServerId,
        conversation_id: ConversationId,
        user_id: UserId,
    ) -> ServiceResult<Conversation> {
        ensure_server_member(self.repos.server_member_repo.as_ref(), server_id, user_id).await?;
        ensure_conversation_in_server(
            self.repos.conversation_repo.as_ref(),
            conversation_id,
            server_id,
        )
        .await
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
        // Only encrypted conversations have pending members.
        let pending_member_ids = if conversation.is_encrypted() {
            self.repos
                .conversation_member_repo
                .list_pending_member_ids(conversation.id)
                .await?
        } else {
            Vec::new()
        };

        Ok(Self::to_dto_with_members(
            conversation,
            member_ids,
            pending_member_ids,
        ))
    }

    /// The DTOs of `conversations`, with members listed in two batched queries.
    async fn to_dtos(
        &self,
        conversations: Vec<Conversation>,
    ) -> ServiceResult<Vec<ConversationDto>> {
        let ids: Vec<ConversationId> = conversations.iter().map(|c| c.id).collect();
        let mut members = self
            .repos
            .conversation_member_repo
            .list_member_ids_for_conversations(&ids)
            .await?;
        let encrypted: Vec<ConversationId> = conversations
            .iter()
            .filter(|c| c.is_encrypted())
            .map(|c| c.id)
            .collect();
        let mut pending = if encrypted.is_empty() {
            Default::default()
        } else {
            self.repos
                .conversation_member_repo
                .list_pending_member_ids_for_conversations(&encrypted)
                .await?
        };

        Ok(conversations
            .into_iter()
            .map(|conv| {
                let member_ids = members.remove(&conv.id).unwrap_or_default();
                let pending_member_ids = pending.remove(&conv.id).unwrap_or_default();
                Self::to_dto_with_members(conv, member_ids, pending_member_ids)
            })
            .collect())
    }

    fn to_dto_with_members(
        conversation: Conversation,
        member_ids: Vec<UserId>,
        pending_member_ids: Vec<UserId>,
    ) -> ConversationDto {
        ConversationDto {
            id: conversation.id,
            server_id: conversation.server_id,
            conversation_type: conversation.conversation_type,
            title: conversation.title,
            topic: conversation.topic,
            member_ids,
            pending_member_ids,
            is_private: conversation.is_private,
            archived_at: conversation.archived_at,
            created_by: conversation.created_by,
            slow_mode_seconds: conversation.slow_mode_seconds,
            posting_policy: conversation.posting_policy,
            posting_allows_threads: conversation.posting_allows_threads,
            encrypted_at: conversation.encrypted_at,
        }
    }

    /// Gate for joining and inviting: role first, so a guest learns nothing about
    /// which conversation ids exist; then the timeout; then the usual read gate.
    async fn ensure_may_add_members(
        &self,
        actor: UserId,
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> ServiceResult<(Conversation, ServerMemberRole)> {
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
        Ok((conversation, standing.role))
    }

    /// The rules of adding `targets` to `conversation` of the default workspace, which every
    /// account joins: at most [`MAX_DEFAULT_WORKSPACE_IDS_PER_CALL`] at once; to a channel only
    /// as an Owner or Moderator, or as the creator of a private channel; to a group within
    /// [`MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS`] when it is plaintext, and as a DM with each newcomer
    /// would be started.
    ///
    /// # Errors
    /// `Validation` past a cap, `Forbidden` for a channel the actor does not manage, and what
    /// starting the DMs would answer.
    async fn ensure_may_add_in_default_workspace(
        &self,
        conversation: &Conversation,
        role: ServerMemberRole,
        actor: UserId,
        targets: &[UserId],
    ) -> ServiceResult<()> {
        if targets.len() > MAX_DEFAULT_WORKSPACE_IDS_PER_CALL {
            return Err(ServiceError::Validation(format!(
                "at most {MAX_DEFAULT_WORKSPACE_IDS_PER_CALL} people are added at once in this \
                 workspace"
            )));
        }
        match conversation.conversation_type {
            CoreConversationType::Channel => {
                if role.is_moderator()
                    || (conversation.is_private && conversation.created_by == Some(actor))
                {
                    Ok(())
                } else {
                    Err(ServiceError::Forbidden(
                        "in this workspace only the owner, moderators and the creator of a \
                         private channel add people to it"
                            .to_string(),
                    ))
                }
            }
            CoreConversationType::Group => {
                let members = &self.repos.conversation_member_repo;
                let mut current = members.list_member_ids(conversation.id).await?;
                current.extend(members.list_pending_member_ids(conversation.id).await?);
                let joining: Vec<UserId> = targets
                    .iter()
                    .copied()
                    .filter(|id| !current.contains(id))
                    .collect();
                if !conversation.is_encrypted()
                    && current.len() + joining.len() > MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS
                {
                    return Err(ServiceError::Validation(format!(
                        "a group of this workspace has at most \
                         {MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS} members"
                    )));
                }
                self.ensure_may_request_group(actor, conversation.server_id, &joining)
                    .await
            }
            CoreConversationType::Direct => Ok(()),
        }
    }

    /// `Validation` when adding `targets` would take the plaintext group `conversation` past
    /// [`rim_services::e2ee::MAX_ENCRYPTED_MEMBERS`] members, pending ones included.
    async fn ensure_group_has_room(
        &self,
        conversation: &Conversation,
        targets: &[UserId],
    ) -> ServiceResult<()> {
        let members = &self.repos.conversation_member_repo;
        let mut current = members.list_member_ids(conversation.id).await?;
        current.extend(members.list_pending_member_ids(conversation.id).await?);
        let joining = targets.iter().filter(|id| !current.contains(id)).count();
        if current.len() + joining > rim_services::e2ee::MAX_ENCRYPTED_MEMBERS {
            return Err(ServiceError::Validation(format!(
                "a group has at most {} members",
                rim_services::e2ee::MAX_ENCRYPTED_MEMBERS
            )));
        }
        Ok(())
    }

    fn access_repos(&self) -> ConversationAccessRepos<'_> {
        ConversationAccessRepos::from_arcs(
            &self.repos.server_member_repo,
            &self.repos.conversation_repo,
            &self.repos.conversation_member_repo,
        )
    }

    /// Adds `user_id`, active or pending as `membership` says, unless already a member.
    /// Returns whether a row was inserted; a concurrent insert that loses the unique-index race,
    /// or an existing pending row, counts as "already a member".
    async fn add_member_if_absent(
        &self,
        conversation_id: ConversationId,
        user_id: UserId,
        membership: Membership,
    ) -> ServiceResult<bool> {
        if self
            .repos
            .conversation_member_repo
            .is_member(conversation_id, user_id)
            .await?
        {
            return Ok(false);
        }
        let members = &self.repos.conversation_member_repo;
        let inserted = match membership {
            Membership::Active => members.add_member(conversation_id, user_id).await,
            Membership::Pending => members.add_pending_member(conversation_id, user_id).await,
        };
        match inserted {
            Ok(_) => Ok(true),
            Err(err) if matches!(err.kind(), RepoErrorKind::Conflict) => Ok(false),
            Err(err) => Err(err.into()),
        }
    }

    /// Removes `user_id`, whose membership was `membership`, from the conversation, deletes
    /// their notifications for it, and tells the remaining members and the removed user (whose
    /// client must drop the conversation).
    async fn remove_member_and_notify(
        &self,
        conversation: &Conversation,
        user_id: UserId,
        membership: Membership,
    ) -> ServiceResult<()> {
        self.repos
            .conversation_member_repo
            .remove_member(conversation.id, user_id)
            .await?;
        self.repos
            .notification_repo
            .delete_for_user_in_conversation(user_id, conversation.id)
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
        // A pending member has no devices in the group.
        if membership == Membership::Active {
            let remaining = notify_user_ids
                .into_iter()
                .filter(|id| *id != user_id)
                .collect();
            self.emit_mls_membership_changed(conversation, remaining);
        }
        Ok(())
    }

    /// Tells the members and `user_id`, active or pending, that `user_id` joined.
    /// Announce that `user_id` joined `dto`, added by `added_by` (`None` for an accepted
    /// invitation).
    fn emit_member_added(&self, dto: &ConversationDto, user_id: UserId, added_by: Option<UserId>) {
        let mut member_user_ids = dto.member_ids.clone();
        if !member_user_ids.contains(&user_id) {
            member_user_ids.push(user_id);
        }
        self.observers
            .emit(WorkspaceEvent::MemberAddedToConversation {
                workspace_id: dto.server_id,
                conversation_id: dto.id,
                user_id,
                member_user_ids,
                added_by,
            });
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

        if cmd.participants == ConversationParticipants::Channel {
            ensure_workspace_features(self.direct_space_id, cmd.server_id)?;
        }
        let direct = self.is_direct_space(cmd.server_id);
        if direct {
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

        // Member, not timed out; guests may only open DMs, except in the direct
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
                let in_default_workspace = self.is_default_workspace(cmd.server_id).await?;
                if in_default_workspace && member_ids.len() > MAX_DEFAULT_WORKSPACE_IDS_PER_CALL {
                    return Err(ServiceError::Validation(format!(
                        "a group of this workspace is started with at most \
                         {MAX_DEFAULT_WORKSPACE_IDS_PER_CALL} people at once"
                    )));
                }
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
                if !cmd.encrypted && unique.len() > rim_services::e2ee::MAX_ENCRYPTED_MEMBERS {
                    return Err(ServiceError::Validation(format!(
                        "a group has at most {} members",
                        rim_services::e2ee::MAX_ENCRYPTED_MEMBERS
                    )));
                }
                if in_default_workspace {
                    if !cmd.encrypted && unique.len() > MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS {
                        return Err(ServiceError::Validation(format!(
                            "a group of this workspace has at most \
                             {MAX_DEFAULT_WORKSPACE_GROUP_MEMBERS} members"
                        )));
                    }
                    self.ensure_may_request_group(cmd.actor.user_id, cmd.server_id, &unique)
                        .await?;
                }

                (CoreConversationType::Group, unique)
            }
            ConversationParticipants::Channel => {
                if !standing.role.is_moderator() && self.is_default_workspace(cmd.server_id).await?
                {
                    return Err(ServiceError::Forbidden(
                        "in this workspace only the owner and moderators create channels"
                            .to_string(),
                    ));
                }
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
                self.ensure_channel_below_cap(cmd.server_id).await?;
                self.ensure_channels_below_ceiling(cmd.server_id).await?;

                // The creator is always a member of the channel, public or private.
                (CoreConversationType::Channel, vec![cmd.actor.user_id])
            }
        };

        let member_count = member_ids.len();
        // Members of an encrypted conversation without a ready device join pending; the
        // creator must be ready.
        let (member_ids, pending_ids) = if cmd.encrypted {
            if member_count > rim_services::e2ee::MAX_ENCRYPTED_MEMBERS {
                return Err(ServiceError::Validation(format!(
                    "an encrypted conversation has at most {} members",
                    rim_services::e2ee::MAX_ENCRYPTED_MEMBERS
                )));
            }
            let (active, pending) = self.split_by_readiness(cmd.server_id, &member_ids).await?;
            if pending.contains(&cmd.actor.user_id) {
                return Err(not_ready());
            }
            (active, pending)
        } else {
            (member_ids, Vec::new())
        };
        if direct {
            self.count_direct_space_start(cmd.actor.user_id)?;
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
                &pending_ids,
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
            pending = pending_ids.len(),
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
        for &user_id in &pending_ids {
            self.emit_member_added(&dto, user_id, Some(cmd.actor.user_id));
        }

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

    async fn get_writable_conversation(
        &self,
        query: GetConversationQuery,
    ) -> ServiceResult<ConversationDto> {
        let conversation = ensure_conversation_writable(
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
        self.to_dtos(conversations).await
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

        let items = self.to_dtos(page.items).await?;

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
        ensure_workspace_features(self.direct_space_id, query.server_id)?;
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

        let items = self.to_dtos(page.items).await?;

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

        // Guests cannot invite and timed-out members cannot write. Private
        // channels and groups: the gate already requires membership (NotFound /
        // PermissionDenied otherwise).
        let (conversation, role) = self
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
        if conversation.conversation_type == CoreConversationType::Group
            && !conversation.is_encrypted()
        {
            self.ensure_group_has_room(&conversation, &targets).await?;
        }
        if self.is_default_workspace(cmd.server_id).await? {
            self.ensure_may_add_in_default_workspace(
                &conversation,
                role,
                cmd.actor.user_id,
                &targets,
            )
            .await?;
        }
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
        // Joiners of an encrypted conversation without a ready device join pending.
        let mut pending = HashSet::new();
        if conversation.is_encrypted() {
            let members = &self.repos.conversation_member_repo;
            let mut current = members.list_member_ids(conversation.id).await?;
            current.extend(members.list_pending_member_ids(conversation.id).await?);
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
            let (_, waiting) = self.split_by_readiness(cmd.server_id, &joining).await?;
            pending.extend(waiting);
        }

        let mut added = Vec::new();
        let mut added_active = false;
        for user_id in targets {
            let membership = if pending.contains(&user_id) {
                Membership::Pending
            } else {
                Membership::Active
            };
            if self
                .add_member_if_absent(conversation.id, user_id, membership)
                .await?
            {
                added.push(user_id);
                added_active |= membership == Membership::Active;
            }
        }

        info!(
            added = added.len(),
            pending = pending.len(),
            "added conversation members"
        );

        let encrypted = conversation.clone();
        let dto = self.to_dto(conversation).await?;
        if added_active {
            self.emit_mls_membership_changed(&encrypted, dto.member_ids.clone());
        }
        for user_id in added {
            self.emit_member_added(&dto, user_id, Some(cmd.actor.user_id));
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
        let standing = member_standing(
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
        if !standing.role.is_moderator() {
            ensure_conversation_access_as(
                self.repos.conversation_member_repo.as_ref(),
                &conversation,
                cmd.actor.user_id,
                standing.role,
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

        let Some(membership) = self
            .repos
            .conversation_member_repo
            .membership(conversation.id, cmd.user_id)
            .await?
        else {
            return Err(ServiceError::NotFound);
        };

        self.remove_member_and_notify(&conversation, cmd.user_id, membership)
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
        ensure_workspace_features(self.direct_space_id, cmd.server_id)?;
        // Guests cannot join; a timed-out member cannot either. A private channel
        // the actor is not in is NotFound here, so it cannot be joined.
        let (conversation, _) = self
            .ensure_may_add_members(cmd.actor.user_id, cmd.server_id, cmd.conversation_id)
            .await?;

        if conversation.conversation_type != CoreConversationType::Channel {
            return Err(ServiceError::Validation(
                "only channels can be joined".to_string(),
            ));
        }
        ensure_not_archived(&conversation)?;

        let joined = self
            .add_member_if_absent(conversation.id, cmd.actor.user_id, Membership::Active)
            .await?;

        let dto = self.to_dto(conversation).await?;
        if joined {
            info!("joined channel");
            self.emit_member_added(&dto, cmd.actor.user_id, Some(cmd.actor.user_id));
        }

        Ok(dto)
    }

    #[instrument(
        name = "conversation.leave_conversation",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn leave_conversation(&self, cmd: LeaveConversationCmd) -> ServiceResult<()> {
        let membership = self
            .repos
            .conversation_member_repo
            .membership(cmd.conversation_id, cmd.actor.user_id)
            .await?;
        // A pending member cannot read the conversation, but may leave it.
        let conversation = if membership == Some(Membership::Pending) {
            self.invited_conversation(cmd.server_id, cmd.conversation_id, cmd.actor.user_id)
                .await?
        } else {
            ensure_conversation_member_access(
                self.access_repos(),
                cmd.actor.user_id,
                cmd.server_id,
                cmd.conversation_id,
            )
            .await?
        };

        if conversation.conversation_type == CoreConversationType::Direct {
            return Err(ServiceError::Validation(
                "cannot leave a direct message".to_string(),
            ));
        }

        // Leaving a public channel you never joined is a no-op, mirroring `join_channel`.
        let Some(membership) = membership else {
            return Ok(());
        };

        // Leaving a private channel as its last member is allowed; it then stays invisible to
        // everyone, and only an Owner/Moderator can archive it, by id.
        self.remove_member_and_notify(&conversation, cmd.actor.user_id, membership)
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
        if !cmd.archived {
            self.ensure_channel_below_cap(cmd.server_id).await?;
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
        let (conversation, standing) = crate::util::ensure_conversation_writable_with_standing(
            self.access_repos(),
            cmd.actor.user_id,
            cmd.server_id,
            cmd.conversation_id,
        )
        .await?;
        if !manages_channel(&conversation, standing.role, cmd.actor.user_id) {
            return Err(ServiceError::Forbidden(
                "only the owner and moderators change a channel's topic".to_string(),
            ));
        }

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

    #[instrument(
        name = "conversation.accept_invite",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn accept_conversation_invite(
        &self,
        cmd: ConversationInviteCmd,
    ) -> ServiceResult<ConversationDto> {
        let user_id = cmd.actor.user_id;
        let conversation = self
            .invited_conversation(cmd.server_id, cmd.conversation_id, user_id)
            .await?;
        match self
            .repos
            .conversation_member_repo
            .membership(conversation.id, user_id)
            .await?
        {
            Some(Membership::Active) => return self.to_dto(conversation).await,
            Some(Membership::Pending) => {}
            None => return Err(ServiceError::NotFound),
        }
        if !self.ready_users(&[user_id]).await?.contains(&user_id) {
            return Err(not_ready());
        }
        // A concurrent accept may have flipped the row first; it emitted the events.
        if !self
            .repos
            .conversation_member_repo
            .activate_member(conversation.id, user_id)
            .await?
        {
            return self.to_dto(conversation).await;
        }
        info!("accepted conversation invitation");

        let encrypted = conversation.clone();
        let dto = self.to_dto(conversation).await?;
        self.emit_mls_membership_changed(&encrypted, dto.member_ids.clone());
        self.emit_member_added(&dto, user_id, None);
        Ok(dto)
    }

    #[instrument(
        name = "conversation.decline_invite",
        skip_all,
        fields(actor_id = %cmd.actor.user_id, server_id = %cmd.server_id, conversation_id = %cmd.conversation_id)
    )]
    async fn decline_conversation_invite(&self, cmd: ConversationInviteCmd) -> ServiceResult<()> {
        let user_id = cmd.actor.user_id;
        if self
            .repos
            .conversation_member_repo
            .membership(cmd.conversation_id, user_id)
            .await?
            != Some(Membership::Pending)
        {
            return Err(ServiceError::NotFound);
        }
        let conversation = self
            .invited_conversation(cmd.server_id, cmd.conversation_id, user_id)
            .await?;
        self.remove_member_and_notify(&conversation, user_id, Membership::Pending)
            .await?;
        info!("declined conversation invitation");
        Ok(())
    }
}
