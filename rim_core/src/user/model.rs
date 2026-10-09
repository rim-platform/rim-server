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
    /// Lowercased, trimmed email address. NOT NULL at the DB layer.
    pub email: String,
    /// IANA timezone name (e.g. `"America/Chicago"`), validated on write. Display/defaulting
    /// only — see `docs/oncall.md`: rotation handoff math lives on the on-call **schedule's**
    /// timezone, never the user's.
    pub timezone: Option<String>,
    /// Key-based principal id (`rimp1_...`) once the account enrolled a key; `None` before.
    /// An account with a principal id signs in with a device key, not its password.
    pub principal_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub username: String,
    pub password: String,
    /// Lowercased, trimmed email address. The repository stores this directly;
    /// callers must normalise before constructing this struct.
    pub email: String,
    /// Optional display name. When `None`, the service layer falls back to `username`.
    pub display_name: Option<String>,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProfilePatch {
    pub display_name: Option<Option<String>>,
    pub avatar_url: Option<Option<String>>,
    pub status_text: Option<Option<String>>,
    pub bio: Option<Option<String>>,
}
