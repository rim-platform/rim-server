use async_trait::async_trait;
use chrono::Duration;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::device_link::model::{DeviceLink, DeviceLinkId, NewDeviceLink, SealedBytes};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// Device links, addressed by the hash of their link id.
///
/// Every transition is one conditional update on `link_hash`, the state it leaves and
/// `expires_at > now()` (database clock), plus the user for calls made with a session. Two
/// racing calls cannot both succeed, and a call that arrives after expiry loses: a transition
/// returns `None` and the caller classifies the refusal with [`DeviceLinkRepository::find`].
/// `consumed` and `cancelled` set `ended_at` and clear the sealed bytes.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DeviceLinkRepository: Send + Sync {
    /// Lock the user's row until the surrounding transaction ends (`SELECT ... FOR UPDATE`).
    /// Returns `false` when the user does not exist.
    async fn lock_user(&self, user_id: UserId) -> RepoResult<bool>;

    /// Cancel every `open`, `offered` or `answered` link of the user, expired ones included,
    /// so the one-live-link index never blocks a new link. Returns the ids of the links
    /// cancelled.
    async fn cancel_live_links(&self, user_id: UserId) -> RepoResult<Vec<DeviceLinkId>>;

    /// Insert an `open` link.
    ///
    /// # Errors
    /// `Conflict` when the user still has a live-state link, or the hash is taken.
    async fn create(&self, link: NewDeviceLink) -> RepoResult<DeviceLink>;

    /// The link with this hash, in any state, expired or not.
    async fn find(&self, link_hash: &[u8]) -> RepoResult<Option<DeviceLink>>;

    /// `open` → `offered`, storing the offer.
    async fn offer(&self, link_hash: &[u8], offer: SealedBytes) -> RepoResult<Option<DeviceLink>>;

    /// `offered` → `answered` for the user's link, storing the answer; the link then expires
    /// `ttl` after now.
    async fn answer(
        &self,
        link_hash: &[u8],
        user_id: UserId,
        answer: SealedBytes,
        ttl: Duration,
    ) -> RepoResult<Option<DeviceLink>>;

    /// `answered` → `consumed` for the user's link.
    async fn complete(&self, link_hash: &[u8], user_id: UserId) -> RepoResult<Option<DeviceLink>>;

    /// `open`, `offered` or `answered` → `cancelled`; only the user's link when `user_id` is
    /// given.
    async fn cancel(
        &self,
        link_hash: &[u8],
        user_id: Option<UserId>,
    ) -> RepoResult<Option<DeviceLink>>;

    /// Delete links whose expiry passed more than `grace` ago. Returns how many were.
    async fn delete_expired(&self, grace: Duration) -> RepoResult<u64>;
}
