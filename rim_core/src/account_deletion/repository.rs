use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::account_deletion::model::{AccountStanding, OwnedSharedWorkspace, PurgeStep};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// Deletion requests and the purge of accounts whose grace period ended. The purge steps run in
/// the caller's transaction, one account at a time, in the order the service calls them.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait AccountDeletionRepository: Send + Sync {
    /// The account's deletion standing; `None` when no user has `user_id`.
    async fn standing(&self, user_id: UserId) -> RepoResult<Option<AccountStanding>>;

    /// The workspaces the account is the only owner of that have other members, by name.
    async fn owned_shared_workspaces(
        &self,
        user_id: UserId,
    ) -> RepoResult<Vec<OwnedSharedWorkspace>>;

    /// Record a deletion request made at `requested_at` that runs at `scheduled_for`, unless one
    /// is already pending or the account is purged. Whether it was recorded.
    async fn record_request(
        &self,
        user_id: UserId,
        requested_at: DateTime<Utc>,
        scheduled_for: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Clear the account's pending deletion request; whether one was pending.
    async fn clear_request(&self, user_id: UserId) -> RepoResult<bool>;

    /// Accounts whose deletion was scheduled for `now` or earlier and that are not purged yet,
    /// the earliest first, at most `limit`.
    async fn due(&self, now: DateTime<Utc>, limit: u64) -> RepoResult<Vec<UserId>>;

    /// Lock the account's row for the purge if its deletion is still due at `now`; `false` when
    /// it was cancelled or purged meanwhile.
    async fn claim_due(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<bool>;

    /// Delete the workspaces the account is the only member of, with everything in them, then
    /// remove the account from every other workspace and conversation (memberships, bans,
    /// timeouts, delegations) and revoke the invites it issued at `now`.
    async fn purge_workspaces(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<PurgeStep>;

    /// Hard-delete the plaintext messages the account sent, keeping, as deleted with no text, the
    /// ones another kept message replies to or whose thread holds kept replies; their
    /// attachments, reactions, pins and mentions go too. Removes the account's own reactions,
    /// mentions of it and its read state. Encrypted messages stay as they are.
    async fn purge_messages(&self, user_id: UserId) -> RepoResult<PurgeStep>;

    /// End the account's sessions and delete its password reset tokens, device links, key
    /// packages, key log, profile card and MLS requests; revoke its devices at `now`, so
    /// encrypted groups remove them.
    async fn purge_credentials(&self, user_id: UserId, now: DateTime<Utc>)
    -> RepoResult<PurgeStep>;

    /// Delete the account's notification rules and notifications, keybinds, commands, channel
    /// lenses, workspace state entries and custom emoji, and the macros no kept pipeline runs.
    async fn purge_personal_data(&self, user_id: UserId) -> RepoResult<PurgeStep>;

    /// Delete the account's relay queue, DM requests, contacts, blocks and subscriptions.
    async fn purge_relay_state(&self, user_id: UserId) -> RepoResult<PurgeStep>;

    /// Turn the user row into a tombstone at `now`: the first free tombstone username, the
    /// tombstone display name, no email, password, avatar, terms acceptance, profile fields or
    /// identity key. The avatar files are not among the step's files: they are found in the
    /// account's avatar directory.
    async fn tombstone(&self, user_id: UserId, now: DateTime<Utc>) -> RepoResult<PurgeStep>;
}
