use crate::model_id;
use chrono::{DateTime, Utc};
use uuid::Uuid;

model_id!(UserId);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct User {
    pub id: UserId,
    pub username: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub last_login_at: Option<DateTime<Utc>>,
    pub display_name: Option<String>,
    pub avatar_url: Option<String>,
    pub status_text: Option<String>,
    pub bio: Option<String>,
    /// Lowercased, trimmed email address; `None` unless an email-bound invite gave the account
    /// one. Anything that emails users skips an account without one.
    pub email: Option<String>,
    /// IANA timezone name (e.g. `"America/Chicago"`), validated on write.
    pub timezone: Option<String>,
    /// Key-based principal id (`rimp1_...`); `None` only for a purged account that never had
    /// one.
    pub principal_id: Option<String>,
    /// When the account's pending deletion runs; `None` while no deletion is pending.
    pub deletion_scheduled_for: Option<DateTime<Utc>>,
    /// Until when the instance's operators locked the account's display name, picture, status
    /// and bio; `None` while no lock is set.
    pub profile_locked_until: Option<DateTime<Utc>>,
    /// What the account's handle is built from.
    pub handle: UserHandle,
}

/// What an account's handle (`@<username>:<domain>`) is built from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UserHandle {
    /// A local account: its username at this server's handle domain.
    Local,
    /// A person from another server: their username at their home relay and that relay's
    /// handle domain.
    Home { username: String, domain: String },
    /// No handle: a person from another server whose home handle is not known, or a purged
    /// account.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NewUser {
    pub username: String,
    /// Lowercased, trimmed email address. The repository stores this directly;
    /// callers must normalise before constructing this struct. `None` unless an email-bound
    /// invite gave one.
    pub email: Option<String>,
    /// Optional display name. When `None`, the service layer falls back to `username`.
    pub display_name: Option<String>,
    /// The key identity the account is created with. The database refuses a live account
    /// without one.
    pub key_identity: Option<NewUserKeyIdentity>,
}

/// The key identity an account is created with. Its key log is stored separately,
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

/// How an account was created with an invite: the invite and its creator.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InviteChain {
    /// The instance or workspace invite the account signed up with.
    pub invite_id: Uuid,
    /// The invite's creator; `None` for an operator's invite or a deleted creator.
    pub invited_by_user_id: Option<UserId>,
}

/// Which table an invite lives in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InviteKind {
    /// An instance invite: a signup to this server.
    Instance,
    /// A workspace invite: a signup or join into one workspace.
    Workspace,
}

/// An invite as its creator's record lists it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CreatedInvite {
    pub id: Uuid,
    pub kind: InviteKind,
    pub created_at: DateTime<Utc>,
    pub expires_at: Option<DateTime<Utc>>,
    pub max_uses: i32,
    pub use_count: i32,
    pub revoked_at: Option<DateTime<Utc>>,
}
