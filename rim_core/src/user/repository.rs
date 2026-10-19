use crate::repository::error::RepoResult;
use crate::user::model::{CreatedInvite, InviteChain, NewUser, UpdateProfilePatch, User, UserId};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use uuid::Uuid;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait UserRepository: Send + Sync {
    /// The user with `id`, a purged account's tombstone included, so what it left behind still
    /// names its author.
    async fn get_by_id(&self, id: UserId) -> RepoResult<Option<User>>;

    async fn get_by_ids(&self, ids: &[UserId]) -> RepoResult<Vec<User>>;

    /// Insert a user. With `key_identity` set, the row also carries its `principal_id` and
    /// `identity_key` (key-only signup); the key log goes through
    /// `KeyIdentityRepository::insert_key_log`.
    ///
    /// # Errors
    /// `Conflict` when the username, the email or the principal id is taken. A row with neither
    /// a password nor a principal id is refused by the database (an `Internal` error: no caller
    /// builds one on purpose).
    async fn create_user(&self, user_data: NewUser) -> RepoResult<User>;
    /// The account signing in or named by `username`, compared without regard to case; a purged
    /// account is nobody.
    async fn find_by_username(&self, username: &str) -> RepoResult<Option<User>>;

    async fn update_profile(&self, id: UserId, patch: UpdateProfilePatch) -> RepoResult<User>;

    async fn update_password(&self, id: UserId, password_hash: String) -> RepoResult<()>;

    /// Find a user by email address using a case-insensitive comparison.
    ///
    /// The lookup uses `LOWER(email) = LOWER($1)` so that `"Foo@Bar.COM"` and
    /// `"foo@bar.com"` resolve to the same row. Returns `None` when no row
    /// matches, or only a purged account's tombstone does.
    ///
    /// # Errors
    /// Propagates any underlying storage error as [`RepoError`].
    async fn find_by_email(&self, email: &str) -> RepoResult<Option<User>>;

    /// Record that the user accepted the instance's terms at `url` at `accepted_at`.
    ///
    /// # Errors
    /// `NotFound` when no user has `id`.
    async fn record_terms_acceptance(
        &self,
        id: UserId,
        url: String,
        accepted_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Record the invite the account `id` signed up with and that invite's creator.
    ///
    /// # Errors
    /// `NotFound` when no user has `id`.
    async fn record_invite_chain(
        &self,
        id: UserId,
        invite_id: Uuid,
        invited_by: Option<UserId>,
    ) -> RepoResult<()>;

    /// The invite the account `id` signed up with; `None` when it used none.
    async fn invite_chain(&self, id: UserId) -> RepoResult<Option<InviteChain>>;

    /// Accounts created with an invite `id` created, oldest first.
    async fn invited_users(&self, id: UserId) -> RepoResult<Vec<UserId>>;

    /// The instance and workspace invites `id` created, newest first.
    async fn invites_created_by(&self, id: UserId) -> RepoResult<Vec<CreatedInvite>>;

    /// Revoke every invite `id` created that is not revoked and has uses left, of both kinds.
    /// Returns how many.
    async fn revoke_invites_created_by(&self, id: UserId) -> RepoResult<u64>;

    /// Refuse `email` (lowercased) at signup and password login, for `reason`, or replace the
    /// reason of its block. Returns whether the block is new.
    async fn block_email(&self, email: &str, reason: &str) -> RepoResult<bool>;

    /// Returns whether a block was lifted.
    async fn unblock_email(&self, email: &str) -> RepoResult<bool>;

    /// Whether `email` is blocked, compared case-insensitively.
    async fn is_email_blocked(&self, email: &str) -> RepoResult<bool>;

    /// Whether the account `id` is an instance operator; a purged account never is.
    async fn is_instance_operator(&self, id: UserId) -> RepoResult<bool>;

    /// Make the account `id` an instance operator or not. Returns whether that changed it.
    async fn set_instance_operator(&self, id: UserId, operator: bool) -> RepoResult<bool>;

    /// The instance operators, by username.
    async fn instance_operators(&self) -> RepoResult<Vec<User>>;
}
