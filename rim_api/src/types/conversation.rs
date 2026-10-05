use async_graphql::{ComplexObject, Enum, SimpleObject};

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
}

impl From<rim_services::conversation::dto::ConversationDto> for Conversation {
    fn from(value: rim_services::conversation::dto::ConversationDto) -> Self {
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
