use chrono::{DateTime, Utc};

use rim_core::user::model::UserId;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresenceStatus {
    Online,
    Away,
    Offline,
}

impl PresenceStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Online => "Online",
            Self::Away => "Away",
            Self::Offline => "Offline",
        }
    }
}

#[derive(Debug, Clone)]
pub struct UserPresence {
    pub user_id: UserId,
    pub status: PresenceStatus,
    pub last_seen_at: DateTime<Utc>,
}
