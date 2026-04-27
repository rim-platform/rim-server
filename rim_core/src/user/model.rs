use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(UserId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub password_hash: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub username: String,
    pub password: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProfilePatch {
    pub display_name: Option<Option<String>>,
    pub avatar_url: Option<Option<String>>,
    pub status_text: Option<Option<String>>,
    pub bio: Option<Option<String>>,
}
