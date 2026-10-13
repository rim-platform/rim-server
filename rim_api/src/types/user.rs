use async_graphql::{Enum, SimpleObject};

#[derive(SimpleObject, Clone, Debug)]
pub struct User {
    pub id: async_graphql::ID,
    pub username: String,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
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
}

impl From<rim_services::user::dto::UserDto> for User {
    fn from(value: rim_services::user::dto::UserDto) -> Self {
        Self {
            id: async_graphql::ID(value.id.to_string()),
            username: value.username,
            display_name: value.display_name,
            avatar_url: value.avatar_url,
            status_text: value.status_text,
            bio: value.bio,
            principal_id: value.principal_id,
            probation_ends_at: None,
            deletion_scheduled_for: value.deletion_scheduled_for,
            instance_operator: false,
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
