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
