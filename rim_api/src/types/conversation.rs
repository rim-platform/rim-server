use async_graphql::{ComplexObject, Enum, InputObject, SimpleObject};

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum ConversationTypeEnum {
    Direct,
    Group,
    Channel,
}

impl From<rim_core::conversation::model::ConversationType> for ConversationTypeEnum {
    fn from(value: rim_core::conversation::model::ConversationType) -> Self {
        use rim_core::conversation::model::ConversationType as D;
        match value {
            D::Direct => Self::Direct,
            D::Group => Self::Group,
            D::Channel => Self::Channel,
        }
    }
}

/// Who may post in a channel.
#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
#[graphql(name = "PostingPolicy")]
pub enum PostingPolicyGql {
    /// Every member who can write in the channel.
    Everyone,
    /// The workspace's Owner and Moderators only.
    Moderators,
}

impl From<rim_core::conversation::model::PostingPolicy> for PostingPolicyGql {
    fn from(value: rim_core::conversation::model::PostingPolicy) -> Self {
        use rim_core::conversation::model::PostingPolicy as D;
        match value {
            D::Everyone => Self::Everyone,
            D::Moderators => Self::Moderators,
        }
    }
}

impl From<PostingPolicyGql> for rim_core::conversation::model::PostingPolicy {
    fn from(value: PostingPolicyGql) -> Self {
        match value {
            PostingPolicyGql::Everyone => Self::Everyone,
            PostingPolicyGql::Moderators => Self::Moderators,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct Conversation {
    pub id: async_graphql::ID,
    pub server_id: async_graphql::ID,
    pub conversation_type: ConversationTypeEnum,
    pub title: Option<String>,
    pub topic: Option<String>,
    pub member_ids: Vec<async_graphql::ID>,
    /// Channels only: visible to its members alone. Fixed at creation.
    pub is_private: bool,
    pub is_archived: bool,
    pub archived_at: Option<chrono::DateTime<chrono::Utc>>,
    pub member_count: i32,
    pub created_by: Option<async_graphql::ID>,
    /// Slow mode: minimum seconds between two messages of the same non-moderator
    /// member; `0` when off.
    pub slow_mode_seconds: i32,
    /// When the conversation became end-to-end encrypted; null for plaintext. Messages from
    /// then on are encrypted.
    pub encrypted_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Members invited to an encrypted conversation who have no device set up for encryption
    /// yet. They cannot read or send; they join with `acceptConversationInvite` once a device is
    /// ready, without the history before that, or `declineConversationInvite`. Not in
    /// `memberIds` or `memberCount`.
    pub pending_member_ids: Vec<async_graphql::ID>,
    /// Who may post in the channel: `EVERYONE` or `MODERATORS` (the workspace's Owner and
    /// Moderators). Always `EVERYONE` for DMs and groups.
    pub posting_policy: PostingPolicyGql,
    /// Under `MODERATORS`, whether everyone else may still add reactions and reply in threads.
    /// Ignored under `EVERYONE`.
    pub posting_allows_threads: bool,
}

#[ComplexObject]
impl Conversation {
    async fn is_muted(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<bool> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx.data::<std::sync::Arc<
            dyn rim_services::notification_preference::service::NotificationPreferenceService,
        >>()?;
        let conversation_id =
            crate::ids::parse_uuid_id::<rim_core::conversation::model::ConversationId>(&self.id)?;

        svc.is_muted(
            rim_services::notification_preference::command::IsMutedQuery {
                actor: rim_services::Actor::new(actor),
                conversation_id,
            },
        )
        .await
        .map_err(crate::errors::map_service_error)
    }

    /// Whether the viewer is a member (for channels: has joined it).
    async fn is_member(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<bool> {
        let actor = crate::auth_context::require_actor(ctx)?;
        let actor = async_graphql::ID(actor.to_string());
        Ok(self.member_ids.contains(&actor))
    }

    /// The viewer's own notification level for this conversation; null inherits
    /// the workspace default.
    async fn notification_level(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<super::NotificationLevel>> {
        Ok(self
            .viewer_notification_level(ctx)
            .await?
            .level
            .map(Into::into))
    }

    /// The level that applies to the viewer now: `NONE` while muted, else `notificationLevel`,
    /// else the workspace default.
    async fn effective_notification_level(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<super::NotificationLevel> {
        Ok(self
            .viewer_notification_level(ctx)
            .await?
            .effective_level
            .into())
    }

    /// The conversation's current MLS group; null for plaintext, before the first Commit, and
    /// for a pending member.
    #[graphql(complexity = 5)]
    async fn mls_group(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<super::MlsGroup>> {
        if self.encrypted_at.is_none() {
            return Ok(None);
        }
        let actor = crate::auth_context::require_actor(ctx)?;
        // A pending member is not in the group yet.
        if self
            .pending_member_ids
            .contains(&async_graphql::ID(actor.to_string()))
        {
            return Ok(None);
        }
        let svc = ctx.data::<std::sync::Arc<dyn rim_services::e2ee::service::E2eeService>>()?;
        let group = svc
            .group(rim_services::e2ee::command::GetMlsGroupQuery {
                actor: rim_services::Actor::new(actor),
                server_id: crate::ids::parse_uuid_id(&self.server_id)?,
                conversation_id: crate::ids::parse_uuid_id(&self.id)?,
            })
            .await
            .map_err(crate::errors::map_service_error)?;
        Ok(group.map(Into::into))
    }

    /// The conversation this one succeeds, on the host that held it; null for any other.
    async fn predecessor(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<super::ConversationPredecessor>> {
        let Ok(federation) = ctx.data::<crate::federation::FederationContext>() else {
            return Ok(None);
        };
        let predecessor = federation
            .host
            .predecessor(crate::ids::parse_uuid_id(&self.id)?)
            .await
            .map_err(crate::errors::map_service_error)?;
        Ok(predecessor.map(Into::into))
    }

    /// For a DM: whether the other member's DM request for it is still pending, so the caller
    /// sees their messages wait for acceptance; false once it is accepted or ignored, when
    /// there is none, and for another member whose relay is another server. Null for groups
    /// and channels.
    async fn dm_request_pending(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<bool>> {
        if self.conversation_type != ConversationTypeEnum::Direct {
            return Ok(None);
        }
        let actor = crate::auth_context::require_actor(ctx)?;
        let Ok(federation) = ctx.data::<crate::federation::FederationContext>() else {
            return Ok(Some(false));
        };
        let me = async_graphql::ID(actor.to_string());
        let Some(other) = self
            .member_ids
            .iter()
            .chain(&self.pending_member_ids)
            .find(|id| **id != me)
        else {
            return Ok(Some(false));
        };
        let conversation_id =
            crate::ids::parse_uuid_id::<rim_core::conversation::model::ConversationId>(&self.id)?;
        let pending = federation
            .relay
            .request_pending(crate::ids::parse_uuid_id(other)?, conversation_id.into())
            .await
            .map_err(crate::errors::map_service_error)?;
        Ok(Some(pending))
    }
}

impl Conversation {
    async fn viewer_notification_level(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<rim_services::notification::dto::ConversationNotificationLevelDto>
    {
        let actor = crate::auth_context::require_actor(ctx)?;
        let svc = ctx
            .data::<std::sync::Arc<dyn rim_services::notification::service::NotificationService>>(
            )?;
        svc.get_conversation_level(
            rim_services::notification::command::GetConversationNotificationLevelQuery {
                actor: rim_services::Actor::new(actor),
                server_id: crate::ids::parse_uuid_id(&self.server_id)?,
                conversation_id: crate::ids::parse_uuid_id(&self.id)?,
            },
        )
        .await
        .map_err(crate::errors::map_service_error)
    }
}

impl From<rim_services::conversation::dto::ConversationDto> for Conversation {
    fn from(value: rim_services::conversation::dto::ConversationDto) -> Self {
        let is_archived = value.is_archived();
        let member_count = i32::try_from(value.member_count()).unwrap_or(i32::MAX);
        Self {
            id: async_graphql::ID(value.id.to_string()),
            server_id: async_graphql::ID(value.server_id.to_string()),
            conversation_type: value.conversation_type.into(),
            title: value.title,
            topic: value.topic,
            member_ids: value
                .member_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
            is_private: value.is_private,
            is_archived,
            archived_at: value.archived_at,
            member_count,
            created_by: value.created_by.map(|id| async_graphql::ID(id.to_string())),
            slow_mode_seconds: value.slow_mode_seconds,
            encrypted_at: value.encrypted_at,
            pending_member_ids: value
                .pending_member_ids
                .into_iter()
                .map(|id| async_graphql::ID(id.to_string()))
                .collect(),
            posting_policy: value.posting_policy.into(),
            posting_allows_threads: value.posting_allows_threads,
        }
    }
}

/// Narrows the `channels` query. By default it lists every channel the viewer can see (public
/// channels plus private channels they belong to) that is not archived.
#[derive(InputObject, Clone, Copy, Debug, Default)]
#[graphql(name = "ChannelListFilter")]
pub struct ChannelListFilterInput {
    /// Only channels the viewer has joined (the sidebar list).
    #[graphql(default)]
    pub joined_only: bool,
    /// Include archived channels.
    #[graphql(default)]
    pub include_archived: bool,
}

impl From<ChannelListFilterInput> for rim_core::conversation::repository::ChannelListFilter {
    fn from(value: ChannelListFilterInput) -> Self {
        Self {
            joined_only: value.joined_only,
            include_archived: value.include_archived,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct UnreadCount {
    pub conversation_id: async_graphql::ID,
    pub count: i64,
}

impl From<rim_services::read_state::dto::UnreadCountDto> for UnreadCount {
    fn from(value: rim_services::read_state::dto::UnreadCountDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            count: value.count,
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
pub struct MutedConversation {
    pub conversation_id: async_graphql::ID,
    pub muted_until: Option<chrono::DateTime<chrono::Utc>>,
}

impl From<rim_services::notification_preference::dto::MutedConversationDto> for MutedConversation {
    fn from(value: rim_services::notification_preference::dto::MutedConversationDto) -> Self {
        Self {
            conversation_id: async_graphql::ID(value.conversation_id.to_string()),
            muted_until: value.muted_until,
        }
    }
}
