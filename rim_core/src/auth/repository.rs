use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::auth::model::{
    InstanceInvite, InstanceInviteId, Invite, InviteId, NewInstanceInvite, NewInvite, NewSession,
    Session, SessionId,
};
use crate::key_identity::model::DeviceKeyId;
use crate::repository::error::RepoResult;
use crate::repository::pagination::{Page, PageDir};
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// Cursor for [`InviteRepository::list_for_server`]: the standard `(created_at, id)` tuple,
/// newest first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InviteCursor {
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub invite_id: InviteId,
}

/// Parameters of [`InviteRepository::list_for_server`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ListInvitesParams {
    pub server_id: ServerId,
    pub limit: u32,
    pub dir: PageDir,
    pub cursor: Option<InviteCursor>,
}

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>>;
    /// The sessions among `ids` that exist, revoked and expired ones included.
    async fn get_many(&self, ids: &[SessionId]) -> RepoResult<Vec<Session>>;
    async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
    async fn touch_last_seen_at(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64>;
    /// Revoke every active session of `user_id` except `keep`. Returns how many were revoked.
    async fn revoke_all_for_user_except(&self, user_id: UserId, keep: SessionId)
    -> RepoResult<u64>;
    /// Record that a key login session belongs to `device_key_id`, so revoking the device ends
    /// the session.
    async fn bind_device(&self, id: SessionId, device_key_id: DeviceKeyId) -> RepoResult<()>;
    /// Revoke every active session of a device. Returns how many were revoked.
    async fn revoke_all_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64>;
    /// Delete the sessions that sign nobody in at `now`: revoked ones and those whose expiry
    /// is reached. Returns how many were deleted.
    async fn delete_ended(&self, now: chrono::DateTime<chrono::Utc>) -> RepoResult<u64>;
}

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait InviteRepository: Send + Sync {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>>;
    async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite>;
    /// Atomically consumes one invite use, in one update that also requires the invite to be
    /// unrevoked and unexpired (no expiry, or one after now).
    ///
    /// Returns `true` if the invite was consumed (use_count incremented), `false` if it was
    /// missing, used up, revoked or expired.
    async fn try_consume(&self, id: InviteId) -> RepoResult<bool>;
    /// Sets the invite's `revoked_at` to `at`. An invite already revoked keeps its first
    /// revocation time; an unknown id changes nothing.
    async fn revoke(&self, id: InviteId, at: chrono::DateTime<chrono::Utc>) -> RepoResult<()>;
    /// The invites of one workspace, revoked, expired and used-up ones included, ordered
    /// `created_at DESC, id DESC`, with the standard cursor semantics (DESC endpoint).
    async fn list_for_server(
        &self,
        params: ListInvitesParams,
    ) -> RepoResult<Page<Invite, InviteCursor>>;
    /// How many invites `user_id` created at or after `since`.
    async fn count_created_since(
        &self,
        user_id: UserId,
        since: chrono::DateTime<chrono::Utc>,
    ) -> RepoResult<u64>;
}

/// Repository for instance invites — admin-issued multi-use tokens for account creation.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait InstanceInviteRepository: Send + Sync {
    /// Create a new instance invite.
    ///
    /// # Errors
    /// Returns [`RepoErrorKind::Conflict`] if `token_hash` already exists.
    async fn create(&self, invite: NewInstanceInvite) -> RepoResult<InstanceInvite>;

    /// Look up an invite by its primary key.
    ///
    /// Returns `None` if no row with the given id exists.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn get_by_id(&self, id: InstanceInviteId) -> RepoResult<Option<InstanceInvite>>;

    /// Look up an invite by its SHA-256 hash.
    ///
    /// Returns `None` if no row exists with the given hash.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<InstanceInvite>>;

    /// Atomically consume one use slot.
    ///
    /// Increments `use_count` only when `use_count < max_uses`.
    /// Returns `true` on success, `false` when exhausted or missing.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn try_consume(&self, id: InstanceInviteId) -> RepoResult<bool>;

    /// Revoke an instance invite.
    ///
    /// No-op if the row does not exist.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn revoke(&self, id: InstanceInviteId) -> RepoResult<()>;

    /// List all non-revoked instance invites.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn list_active(&self) -> RepoResult<Vec<InstanceInvite>>;
}
