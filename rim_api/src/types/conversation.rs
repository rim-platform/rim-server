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
    /// Slow mode (Track 1.2): minimum seconds between two messages of the same non-moderator
    /// member; `0` when off.
    pub slow_mode_seconds: i32,
    /// When the conversation became end-to-end encrypted; null for plaintext. Messages from
    /// then on are encrypted.
    pub encrypted_at: Option<chrono::DateTime<chrono::Utc>>,
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

    /// The viewer's own notification level for this conversation (Track 1.3); null inherits
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

    /// The conversation's current MLS group; null for plaintext and before the first Commit.
    #[graphql(complexity = 5)]
    async fn mls_group(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<super::MlsGroup>> {
        if self.encrypted_at.is_none() {
            return Ok(None);
        }
        let actor = crate::auth_context::require_actor(ctx)?;
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
