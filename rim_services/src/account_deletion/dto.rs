use chrono::{DateTime, Utc};

use rim_core::auth::model::SessionId;
use rim_core::user::model::UserId;

use crate::Actor;

/// Days an account waits between asking to be deleted and its purge, by default.
pub const DEFAULT_GRACE_DAYS: u32 = 14;

/// Longest grace period an operator may set, in days.
pub const MAX_GRACE_DAYS: u32 = 30;

/// Ask for the caller's account to be deleted once the grace period ends.
#[derive(Debug, Clone)]
pub struct RequestAccountDeletionCmd {
    pub actor: Actor,
    /// The session the request comes from; every other session of the account ends.
    pub session_id: SessionId,
    /// The caller's username, typed again to confirm.
    pub confirm_username: String,
}

/// A pending deletion: when the account will be purged.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AccountDeletionScheduleDto {
    pub scheduled_for: DateTime<Utc>,
}

/// What the purge did to one account, or would do in a dry run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPurgeDto {
    /// The purged account, now a tombstone with the same id.
    pub user_id: UserId,
    /// Rows removed or changed, by kind, in the order the purge ran.
    pub counts: Vec<(&'static str, u64)>,
    /// Stored files deleted (attachments, avatar, custom emoji, plugin bundles); in a dry run,
    /// the files that would be.
    pub files: u64,
}

/// An account the purge skipped because its transaction failed; the next run tries again.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountPurgeFailureDto {
    pub user_id: UserId,
    pub error: String,
}

/// One run of the purge over every account whose grace period ended.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PurgeRunDto {
    /// Whether the run changed nothing (every transaction rolled back).
    pub dry_run: bool,
    pub purged: Vec<AccountPurgeDto>,
    pub failed: Vec<AccountPurgeFailureDto>,
}
