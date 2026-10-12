use crate::model_id;
use chrono::{DateTime, Utc};

model_id!(UserId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub username: String,
    /// Argon2 hash of the password; `None` for a key-only account, which has never had one.
    pub password_hash: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    /// Lowercased, trimmed email address; `None` for a key-only account. Anything that emails
    /// users skips an account without one.
    pub email: Option<String>,
    /// IANA timezone name (e.g. `"America/Chicago"`), validated on write.
    pub timezone: Option<String>,
    /// Key-based principal id (`rimp1_...`) once the account enrolled a key; `None` before.
    /// An account with a principal id signs in with a device key, not its password.
    pub principal_id: Option<String>,
    /// When the account's pending deletion runs; `None` while no deletion is pending.
    pub deletion_scheduled_for: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub username: String,
    /// Argon2 hash of the password; `None` for a key-only account, which must then carry
    /// `key_identity` (the database refuses a row with neither).
    pub password: Option<String>,
    /// Lowercased, trimmed email address. The repository stores this directly;
    /// callers must normalise before constructing this struct. `None` for a key-only account.
    pub email: Option<String>,
    /// Optional display name. When `None`, the service layer falls back to `username`.
    pub display_name: Option<String>,
    /// The key identity the account is created with (key-only signup); `None` for a password
    /// account, which may enroll one later.
    pub key_identity: Option<NewUserKeyIdentity>,
}

/// The key identity a key-only account is created with. Its key log is stored separately,
/// through `KeyIdentityRepository::insert_key_log`, in the same transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUserKeyIdentity {
    /// `rimp1_...`, derived from the log's inception.
    pub principal_id: String,
    /// The current identity key (the log head's) in tagged form.
    pub identity_key: String,
}

#[derive(Debug, Clone, Default)]
pub struct UpdateProfilePatch {
    pub display_name: Option<Option<String>>,
    pub avatar_url: Option<Option<String>>,
    pub status_text: Option<Option<String>>,
    pub bio: Option<Option<String>>,
}
