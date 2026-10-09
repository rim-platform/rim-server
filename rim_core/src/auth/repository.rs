use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::auth::model::{
    BootstrapToken, BootstrapTokenId, InstanceInvite, InstanceInviteId, Invite, InviteId,
    NewBootstrapToken, NewInstanceInvite, NewInvite, NewSession, Session, SessionId,
};
use crate::key_identity::model::DeviceKeyId;
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait SessionRepository: Send + Sync {
    async fn get_by_id(&self, id: SessionId) -> RepoResult<Option<Session>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Session>>;
    async fn create_session(&self, session: NewSession) -> RepoResult<Session>;
    async fn touch_last_seen_at(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_session(&self, id: SessionId) -> RepoResult<()>;
    async fn revoke_all_for_user(&self, user_id: UserId) -> RepoResult<u64>;
    /// Record that a key login session belongs to `device_key_id`, so revoking the device ends
    /// the session.
    async fn bind_device(&self, id: SessionId, device_key_id: DeviceKeyId) -> RepoResult<()>;
    /// Revoke every active session of a device. Returns how many were revoked.
    async fn revoke_all_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64>;
}

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait InviteRepository: Send + Sync {
    async fn get_by_id(&self, id: InviteId) -> RepoResult<Option<Invite>>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<Invite>>;
    async fn create_invite(&self, invite: NewInvite) -> RepoResult<Invite>;
    /// Atomically consumes one invite use.
    ///
    /// Returns `true` if the invite was consumed (use_count incremented), `false` if it was
    /// already exhausted or missing.
    async fn try_consume(&self, id: InviteId) -> RepoResult<bool>;
    async fn revoke_invite(&self, id: InviteId) -> RepoResult<()>;
    async fn list_active_by_server(
        &self,
        server_id: crate::server::model::ServerId,
    ) -> RepoResult<Vec<Invite>>;
}

/// Repository for bootstrap tokens — one-shot CLI-issued tokens for cold-start signup.
#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait BootstrapTokenRepository: Send + Sync {
    /// Create a new bootstrap token row with the given fields.
    ///
    /// # Errors
    /// Returns [`RepoErrorKind::Conflict`] if `token_hash` already exists.
    async fn create(&self, token: NewBootstrapToken) -> RepoResult<BootstrapToken>;

    /// Look up a bootstrap token by its primary key.
    ///
    /// Returns `None` if no row with the given id exists.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn get_by_id(&self, id: BootstrapTokenId) -> RepoResult<Option<BootstrapToken>>;

    /// Look up a token by its SHA-256 hash.
    ///
    /// Returns `None` if no row with the given hash exists.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<BootstrapToken>>;

    /// Atomically mark a bootstrap token as used.
    ///
    /// Sets `used_at = now()` and `consumed_by_user_id` in a single
    /// `UPDATE … WHERE id = $1 AND used_at IS NULL AND revoked_at IS NULL`
    /// statement, eliminating the check-then-update race present in a
    /// two-step read + write.
    ///
    /// Returns `true` if the row was updated (token was still unused and
    /// not revoked), `false` if another concurrent consumer already claimed
    /// it or if the row does not exist.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn try_mark_used(
        &self,
        id: BootstrapTokenId,
        consumed_by_user_id: UserId,
    ) -> RepoResult<bool>;

    /// Revoke a token by setting `revoked_at = now()`.
    ///
    /// No-op if the row does not exist.
    ///
    /// # Errors
    /// Returns a repository error on DB failure.
    async fn revoke(&self, id: BootstrapTokenId) -> RepoResult<()>;
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
