use std::borrow::Cow;
use std::sync::Arc;

use async_graphql::dataloader::DataLoader;
use async_graphql::parser::types::Field;
use async_graphql::{
    ContextSelectionSet, Enum, OutputType, Positioned, ServerResult, SimpleObject, Value, registry,
};
use rim_core::server_feature::model::ServerFeature;
use rim_services::feature::service::FeatureGate;

use crate::loaders::{AuthorBanKey, AuthorBanLoader};

/// A stored profile picture's URL as the instance shows it: the URL while the `avatars` feature
/// is enabled, null while it is not. The schema declares it as a nullable `String`.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AvatarUrl(pub Option<String>);

impl OutputType for AvatarUrl {
    fn type_name() -> Cow<'static, str> {
        <Option<String> as OutputType>::type_name()
    }

    fn qualified_type_name() -> String {
        <Option<String> as OutputType>::qualified_type_name()
    }

    fn create_type_info(registry: &mut registry::Registry) -> String {
        <Option<String> as OutputType>::create_type_info(registry)
    }

    async fn resolve(
        &self,
        ctx: &ContextSelectionSet<'_>,
        field: &Positioned<Field>,
    ) -> ServerResult<Value> {
        let gate = ctx
            .data::<Arc<dyn FeatureGate>>()
            .map_err(|e| e.into_server_error(field.pos))?;
        if gate.state(ServerFeature::Avatars).is_enabled() {
            OutputType::resolve(&self.0, ctx, field).await
        } else {
            Ok(Value::Null)
        }
    }
}

#[derive(SimpleObject, Clone, Debug)]
#[graphql(complex)]
pub struct User {
    #[graphql(skip)]
    pub(crate) ban_workspace_id: Option<rim_core::server::model::ServerId>,
    #[graphql(skip)]
    ban_status: Arc<tokio::sync::OnceCell<Option<rim_services::moderation::dto::ServerBanDto>>>,
    pub id: async_graphql::ID,
    pub username: String,
    pub display_name: Option<String>,
    /// The profile picture's URL; null while the instance has the `avatars` feature off.
    pub avatar_url: AvatarUrl,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    /// Key-based principal id (`rimp1_...`) once the account enrolled a key; null before. Plugins
    /// see this id, and the host maps a provisional `rimp0_` id to it once.
    pub principal_id: Option<String>,
    /// When the account leaves probation, while it is in it. Read on `me` only; null for
    /// everyone else and once probation is over.
    pub probation_ends_at: Option<chrono::DateTime<chrono::Utc>>,
    /// When the account's pending deletion runs, after its grace period (`requestAccountDeletion`);
    /// null while none is pending, and always null except on `me`. Once it runs, the account is
    /// purged and only a tombstone named "Deleted account" stays as the author of its encrypted
    /// messages.
    pub deletion_scheduled_for: Option<chrono::DateTime<chrono::Utc>>,
    /// Whether the account is an instance operator, who may run the instance moderation queue
    /// (`instanceReports` and the other `instance…` roots). Read on `me` only; false for
    /// everyone else.
    pub instance_operator: bool,
    /// The person's handle, `@<username>:<domain>`: a local account's username at this server's
    /// handle domain, or a person from another server's username at their home server and that
    /// server's handle domain. Null when it is not known (a person from another server whose
    /// home server did not say, or whose handle domain does not point back at it) and for a
    /// deleted account.
    pub handle: Option<String>,
}

impl From<rim_services::user::dto::UserDto> for User {
    fn from(value: rim_services::user::dto::UserDto) -> Self {
        Self {
            ban_workspace_id: None,
            ban_status: Arc::new(tokio::sync::OnceCell::new()),
            id: async_graphql::ID(value.id.to_string()),
            username: value.username,
            display_name: value.display_name,
            avatar_url: AvatarUrl(value.avatar_url),
            status_text: value.status_text,
            bio: value.bio,
            principal_id: value.principal_id,
            probation_ends_at: None,
            deletion_scheduled_for: value.deletion_scheduled_for,
            instance_operator: false,
            handle: value.handle,
        }
    }
}

// ── Presence ──────────────────────────────────────────────────────────────

#[derive(Enum, Copy, Clone, Eq, PartialEq, Debug)]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

impl From<PresenceStatus> for rim_services::presence::PresenceStatus {
    fn from(value: PresenceStatus) -> Self {
        match value {
            PresenceStatus::Online => Self::Online,
            PresenceStatus::Away => Self::Away,
            PresenceStatus::Offline => Self::Offline,
        }
    }
}

impl From<rim_services::presence::PresenceStatus> for PresenceStatus {
    fn from(value: rim_services::presence::PresenceStatus) -> Self {
        match value {
            rim_services::presence::PresenceStatus::Online => Self::Online,
            rim_services::presence::PresenceStatus::Away => Self::Away,
            rim_services::presence::PresenceStatus::Offline => Self::Offline,
        }
    }
}

#[derive(SimpleObject)]
pub struct UserPresenceGql {
    pub user_id: async_graphql::ID,
    pub status: PresenceStatus,
    pub last_seen_at: chrono::DateTime<chrono::Utc>,
}

impl From<rim_services::presence::UserPresence> for UserPresenceGql {
    fn from(value: rim_services::presence::UserPresence) -> Self {
        Self {
            user_id: async_graphql::ID(value.user_id.to_string()),
            status: value.status.into(),
            last_seen_at: value.last_seen_at,
        }
    }
}

impl User {
    pub(crate) fn in_workspace(mut self, workspace: rim_core::server::model::ServerId) -> Self {
        self.ban_workspace_id = Some(workspace);
        self.ban_status = Arc::new(tokio::sync::OnceCell::new());
        self
    }

    async fn ban(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<rim_services::moderation::dto::ServerBanDto>> {
        let Some(workspace) = self.ban_workspace_id else {
            return Ok(None);
        };
        let actor = crate::auth_context::require_actor(ctx)?;
        let user_id = crate::ids::parse_uuid_id(&self.id)?;
        self.ban_status
            .get_or_try_init(|| async {
                if let Some(loader) = ctx.data_opt::<DataLoader<AuthorBanLoader>>() {
                    return loader
                        .load_one(AuthorBanKey {
                            actor_id: actor,
                            workspace_id: workspace,
                            user_id,
                        })
                        .await
                        .map(|ban| ban.flatten())
                        .map_err(|error| (*error).clone());
                }
                ctx.data::<Arc<dyn rim_services::moderation::service::ModerationService>>()?
                    .member_ban(rim_services::Actor::new(actor), workspace, user_id)
                    .await
                    .map_err(crate::errors::map_service_error)
            })
            .await
            .cloned()
    }
}

#[async_graphql::ComplexObject]
impl User {
    /// Whether this message's author is banned in its workspace; false outside message authors.
    async fn banned(&self, ctx: &async_graphql::Context<'_>) -> async_graphql::Result<bool> {
        Ok(self.ban(ctx).await?.is_some())
    }

    /// End of the active workspace ban; null for a permanent ban or no ban.
    async fn banned_until(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<chrono::DateTime<chrono::Utc>>> {
        Ok(self.ban(ctx).await?.and_then(|ban| ban.expires_at))
    }

    /// The active ban's reason, readable only by workspace moderators.
    async fn ban_reason(
        &self,
        ctx: &async_graphql::Context<'_>,
    ) -> async_graphql::Result<Option<String>> {
        Ok(self.ban(ctx).await?.and_then(|ban| ban.reason))
    }
}
