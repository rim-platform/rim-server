use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::s2s::model::{
    ForgottenPeer, PeerPin, PeerServer, PeerStatus, ServerDescriptorRecord, ServerKeyLogRecord,
    SignedRecord,
};

/// This server's key log and published descriptor.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait ServerIdentityRepository: Send + Sync {
    /// The whole key log, inception first.
    async fn key_log(&self) -> RepoResult<Vec<ServerKeyLogRecord>>;

    /// Append an entry.
    ///
    /// # Errors
    /// `Conflict` when an entry with that seq exists, so two writers cannot both extend the log.
    async fn append_key_log_entry(&self, entry: ServerKeyLogRecord) -> RepoResult<()>;

    async fn descriptor(&self) -> RepoResult<Option<ServerDescriptorRecord>>;

    /// Store `descriptor` if no descriptor is stored or the stored one has a lower version.
    /// Returns whether it was stored.
    async fn store_descriptor(&self, descriptor: ServerDescriptorRecord) -> RepoResult<bool>;
}

/// Pinned peers and their key logs.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PeerServerRepository: Send + Sync {
    async fn find(&self, server_id: &str) -> RepoResult<Option<PeerServer>>;

    /// The peer's pinned key log, inception first.
    async fn key_log(&self, server_id: &str) -> RepoResult<Vec<SignedRecord>>;

    /// Pin a new peer: its row with `status`, and its whole log. Its forgotten head, if any, is
    /// deleted with it.
    ///
    /// # Errors
    /// `Conflict` when the peer is pinned already; `PreconditionFailed` when
    /// [`MAX_PEERS_PER_ORIGIN`](crate::s2s::model::MAX_PEERS_PER_ORIGIN) peers are pinned at its
    /// origin.
    async fn insert(&self, pin: PeerPin, status: PeerStatus) -> RepoResult<PeerServer>;

    /// The key-log head kept when the peer's idle pin was forgotten.
    async fn forgotten(&self, server_id: &str) -> RepoResult<Option<ForgottenPeer>>;

    /// Forget the `active` pins idle since `idle_before` that nothing here refers to: delete
    /// each with its log and keep its key-log head. A pin is idle when it was neither fetched
    /// nor sent a request since then. It is referred to when its id is in `kept`, is the home
    /// relay of a remote user, or is named by a delegation, a subscription, an outbox row, an
    /// inbound cursor, a queued envelope, a DM request, a revocation fan-out, a conversation's
    /// predecessor or a cached identity, or when a remote user's profile card names its
    /// origin. Returns the server ids forgotten.
    async fn forget_idle(
        &self,
        idle_before: DateTime<Utc>,
        kept: &[String],
    ) -> RepoResult<Vec<String>>;

    /// Move a pin forward: append the log entries above `expected_head_seq` and replace the
    /// head, previous key, descriptor and origin, if the stored head is still
    /// `expected_head_seq` and the peer is `active`. Returns `None` when it is not.
    async fn advance(&self, pin: PeerPin, expected_head_seq: u32)
    -> RepoResult<Option<PeerServer>>;

    /// Record a fetch that changed nothing in the log: `fetched_at` becomes now and the
    /// descriptor is replaced when `descriptor` is given.
    async fn mark_fetched(
        &self,
        server_id: &str,
        descriptor: Option<(u32, SignedRecord)>,
    ) -> RepoResult<()>;

    /// Set the status. Returns whether the peer exists.
    async fn set_status(&self, server_id: &str, status: PeerStatus) -> RepoResult<bool>;

    /// Set the status to `to` if it is `from`. Returns whether it changed.
    async fn replace_status(
        &self,
        server_id: &str,
        from: PeerStatus,
        to: PeerStatus,
    ) -> RepoResult<bool>;

    /// Forget a peer, its log and its forgotten head, so its next contact pins it afresh.
    /// Returns whether a pin or a forgotten head existed.
    async fn delete(&self, server_id: &str) -> RepoResult<bool>;

    /// Set `last_request_at` to now.
    async fn touch(&self, server_id: &str) -> RepoResult<()>;

    /// Record the peer's checked handle domain, or `None` when its descriptor declares none or
    /// the domain does not point back at it.
    async fn set_handle_domain(&self, server_id: &str, domain: Option<String>) -> RepoResult<()>;
}

/// Request nonces seen per peer.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait S2sNonceRepository: Send + Sync {
    /// Remember a nonce until `expires_at`. Returns `false` when the peer used it already, also
    /// when two inserts race: exactly one of them returns `true`.
    async fn insert(
        &self,
        peer_server_id: &str,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Delete nonces past their expiry. Returns how many were.
    async fn delete_expired(&self) -> RepoResult<u64>;
}
