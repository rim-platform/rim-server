use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::s2s::dto::{
    PeerBlockChange, PeerDto, PeerLookup, S2sClientError, S2sRequest, S2sResponse,
};

/// Peers this server talks to: pinning a peer's key log from its discovery document, keeping
/// the pin current, and refusing peers whose log forked.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PeerService: Send + Sync {
    /// The keys `server_id` signs with. A peer not pinned yet is pinned from `origin` when one
    /// is given (its inception must hash to `server_id`). A pinned peer is refetched from its
    /// pinned origin when `key_seq` is above its head or the pin is over 6 hours old, at most
    /// once a minute per peer and 60 times a minute in all; a failed refetch keeps the pin.
    async fn resolve(
        &self,
        server_id: &str,
        origin: Option<&str>,
        key_seq: Option<u32>,
    ) -> ServiceResult<PeerLookup>;

    /// Refetch a pinned peer now, within the same limits, as after a `401` from it.
    async fn refresh(&self, server_id: &str) -> ServiceResult<PeerLookup>;

    /// Pin the server at `origin`, or refresh its pin when it is pinned already.
    ///
    /// # Errors
    /// `Validation` when the origin is not acceptable, the document does not verify, its id is
    /// not `expected_server_id`, or its descriptor has no server-to-server endpoint at the
    /// origin; `Unavailable` when it cannot be fetched; `Forbidden` when the pin forked.
    async fn pin(&self, origin: &str, expected_server_id: Option<&str>) -> ServiceResult<PeerDto>;

    async fn get(&self, server_id: &str) -> ServiceResult<Option<PeerDto>>;

    /// A pinned peer's discovery document as JSON text, rebuilt from its pinned key log and
    /// descriptor.
    async fn discovery_document(&self, server_id: &str) -> ServiceResult<Option<String>>;

    /// Forget a peer's pin after an out-of-band check, so its next contact pins it afresh.
    /// Returns whether it was pinned.
    async fn reset(&self, server_id: &str) -> ServiceResult<bool>;

    /// Block a pinned peer (`blocked`: refused both ways at once) or unblock it (`active`). A
    /// forked peer stays forked.
    async fn set_blocked(&self, server_id: &str, blocked: bool) -> ServiceResult<PeerBlockChange>;
}

/// Replay protection for incoming server-to-server requests.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait S2sNonceService: Send + Sync {
    /// Accept a request nonce from a peer, remembered until `expires_at`. Returns `false` for a
    /// nonce the peer used already. Also records the peer's latest request.
    async fn accept(
        &self,
        peer_server_id: &str,
        nonce: &str,
        expires_at: DateTime<Utc>,
    ) -> ServiceResult<bool>;

    /// Delete nonces past their expiry. Returns how many were.
    async fn purge_expired(&self) -> ServiceResult<u64>;
}

/// Sends signed requests to peers and verifies their signed answers.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait S2sClient: Send + Sync {
    async fn send(&self, request: S2sRequest) -> Result<S2sResponse, S2sClientError>;
}
