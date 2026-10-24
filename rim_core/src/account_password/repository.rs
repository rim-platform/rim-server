use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::account_password::model::{
    AccountPassword, AttemptSchedule, AttemptStart, PasswordLogin, PasswordLoginId, PasswordReset,
    PasswordResetId,
};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// Account passwords, the OPAQUE login states between start and finish, the rewraps a change
/// leaves for other devices, and resets in progress.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AccountPasswordRepository: Send + Sync {
    /// The account's password; `None` when it has none.
    async fn get(&self, user_id: UserId) -> RepoResult<Option<AccountPassword>>;

    /// Store the account's first password with its sealed server share, at `now`.
    ///
    /// # Errors
    /// `Conflict` when the account already has one.
    async fn insert(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        share_sealed: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Replace the account's registration record, keeping its server share, and clear its
    /// tries and lock. Returns whether the account had a password.
    async fn replace_registration(
        &self,
        user_id: UserId,
        registration: Vec<u8>,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// How many accounts have a password.
    async fn count_passwords(&self) -> RepoResult<u64>;

    /// Count a login start against the account's password at `now`, in one statement: when the
    /// password is neither locked nor waiting, add one to its tries and set the wait and the
    /// lock `schedule` gives the new count.
    async fn start_attempt(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
        schedule: &AttemptSchedule,
    ) -> RepoResult<AttemptStart>;

    /// Clear the account's tries, wait and lock after a successful login.
    async fn clear_attempts(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<()>;

    /// Store a login state, after deleting the states that expired before `now`.
    async fn insert_login(&self, login: PasswordLogin, now: DateTime<Utc>) -> RepoResult<()>;

    /// Delete the login state `id` and return it, expired or not; `None` when there is none.
    async fn take_login(&self, id: PasswordLoginId) -> RepoResult<Option<PasswordLogin>>;

    /// Store a ticket, by the SHA-256 hex of its token, that lets `user_id` finish a key sign-in
    /// until `expires_at`, after deleting the tickets that expired before `now`.
    async fn insert_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        expires_at: DateTime<Utc>,
        now: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Delete the ticket whose token hashes to `token_hash` when it belongs to `user_id` and has
    /// not expired at `now`. Returns whether it did.
    async fn consume_ticket(
        &self,
        token_hash: &str,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Store `rewrap` for the account at `now` and keep only its newest `keep` rewraps.
    async fn insert_rewrap(
        &self,
        user_id: UserId,
        rewrap: Vec<u8>,
        now: DateTime<Utc>,
        keep: u32,
    ) -> RepoResult<()>;

    /// The account's rewraps, newest first.
    async fn list_rewraps(&self, user_id: UserId) -> RepoResult<Vec<Vec<u8>>>;

    /// Delete the account's rewraps. Returns how many there were.
    async fn delete_rewraps(&self, user_id: UserId) -> RepoResult<u64>;

    /// Store a reset in progress, after deleting the resets that expired before `now`.
    async fn insert_reset(&self, reset: PasswordReset, now: DateTime<Utc>) -> RepoResult<()>;

    /// Delete the reset `id` and return it, expired or not; `None` when there is none.
    async fn take_reset(&self, id: PasswordResetId) -> RepoResult<Option<PasswordReset>>;
}
