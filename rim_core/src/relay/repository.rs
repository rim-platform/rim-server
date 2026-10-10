use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use uuid::Uuid;

use crate::key_identity::model::DeviceKeyId;
use crate::relay::model::{
    CachedIdentity, Contact, DmRequest, DmRequestId, DmRequestStatus, DroppedSpace, NewDmRequest,
    NewRelayEnvelope, NewRelayItem, RelayItemRecord,
};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

/// What a relay keeps for its local users: queued host events with per-device cursors, the
/// request inbox, contacts, blocks, and identities fetched from other relays.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RelayRepository: Send + Sync {
    /// The highest outbox id received from a host; 0 before the first.
    async fn received_through(&self, peer_server_id: &str) -> RepoResult<i64>;

    /// Raise the highest outbox id received from a host; a lower value changes nothing.
    async fn advance_received_through(&self, peer_server_id: &str, through: i64) -> RepoResult<()>;

    /// Hosts this relay has received from.
    async fn inbound_peers(&self) -> RepoResult<Vec<String>>;

    /// Store an envelope, or return the stored one with the same host, space, stream and seq.
    async fn insert_envelope(&self, envelope: NewRelayEnvelope) -> RepoResult<i64>;

    async fn insert_item(&self, item: NewRelayItem) -> RepoResult<i64>;

    /// Bytes queued for a user, held items included.
    async fn queued_bytes(&self, user_id: UserId) -> RepoResult<i64>;

    /// Delete a user's oldest items that are not held until at least `bytes` are freed.
    /// Returns the spaces they belonged to.
    async fn drop_oldest(&self, user_id: UserId, bytes: i64) -> RepoResult<Vec<DroppedSpace>>;

    /// The items a device reads above `after`, oldest first: the user's items for every
    /// device and those for this device, never held ones.
    async fn list_items(
        &self,
        user_id: UserId,
        device_key_id: DeviceKeyId,
        after: i64,
        limit: i64,
    ) -> RepoResult<Vec<RelayItemRecord>>;

    /// Where a device's queue starts: its cursor, or, before it acked anything, the newest
    /// item queued before the device was added.
    async fn device_cursor(&self, device_key_id: DeviceKeyId) -> RepoResult<i64>;

    /// Move a device's cursor up to `up_to`; it never moves back.
    async fn ack(&self, device_key_id: DeviceKeyId, up_to: i64) -> RepoResult<()>;

    /// Delete items every non-revoked device of their user has passed. Returns how many.
    async fn delete_acked_items(&self) -> RepoResult<u64>;

    async fn delete_expired_items(&self, now: DateTime<Utc>) -> RepoResult<u64>;

    /// Delete envelopes no item refers to. Returns how many.
    async fn delete_orphan_envelopes(&self) -> RepoResult<u64>;

    async fn find_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        conversation_id: Uuid,
    ) -> RepoResult<Option<DmRequest>>;

    /// # Errors
    /// `Conflict` when the user has a request for that conversation already.
    async fn insert_request(&self, request: NewDmRequest) -> RepoResult<DmRequest>;

    async fn get_request(&self, user_id: UserId, id: DmRequestId) -> RepoResult<Option<DmRequest>>;

    /// A user's requests, newest first.
    async fn list_requests(
        &self,
        user_id: UserId,
        status: Option<DmRequestStatus>,
    ) -> RepoResult<Vec<DmRequest>>;

    async fn count_pending_requests(&self, user_id: UserId) -> RepoResult<i64>;

    /// Requests from a principal created since `since`, for every local user.
    async fn count_requests_from(
        &self,
        principal_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<i64>;

    /// Move a pending request to `status`, expiring at `expires_at`. Returns `None` when the
    /// request is not pending.
    async fn decide_request(
        &self,
        id: DmRequestId,
        status: DmRequestStatus,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<Option<DmRequest>>;

    /// Let a request's held items flow. Returns how many.
    async fn release_held(&self, request_id: DmRequestId) -> RepoResult<u64>;

    /// Delete a request's held items but its first, the invite, which stays with the request so
    /// the app can still verify the conversation's home. Returns how many.
    async fn delete_held(&self, request_id: DmRequestId) -> RepoResult<u64>;

    async fn count_held(&self, request_id: DmRequestId) -> RepoResult<i64>;

    async fn held_items(&self, request_id: DmRequestId) -> RepoResult<Vec<RelayItemRecord>>;

    /// Whether the user ever accepted a request from the principal.
    async fn accepted_from(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// Ignore every pending request from a principal, deleting their held items but each
    /// request's invite. Returns how many requests.
    async fn ignore_pending_from(
        &self,
        user_id: UserId,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    async fn delete_expired_requests(&self, now: DateTime<Utc>) -> RepoResult<u64>;

    /// Local users whose request for a conversation of this server's direct space is pending,
    /// with their principal ids.
    async fn pending_holders(&self, conversation_id: Uuid) -> RepoResult<Vec<(UserId, String)>>;

    /// Whether a conversation stays out of a local user's list: their request for it is pending
    /// or ignored, or they blocked the direct-space conversation's creator.
    async fn conversation_hidden(&self, user_id: UserId, conversation_id: Uuid)
    -> RepoResult<bool>;

    /// A user's contacts, oldest first.
    async fn list_contacts(&self, user_id: UserId) -> RepoResult<Vec<Contact>>;

    /// Add a contact, or replace its relay URLs.
    async fn upsert_contact(
        &self,
        user_id: UserId,
        principal_id: &str,
        relay_urls: Vec<String>,
    ) -> RepoResult<Contact>;

    async fn delete_contact(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn is_contact(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// Returns whether the block is new.
    async fn block(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn unblock(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn is_blocked(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// Hosts that queued items or sent requests for a user.
    async fn hosts_of_user(&self, user_id: UserId) -> RepoResult<Vec<String>>;

    /// Delete a user's queue, requests, contacts and blocks.
    async fn clear_user(&self, user_id: UserId) -> RepoResult<()>;

    async fn cached_identity(&self, principal_id: &str) -> RepoResult<Option<CachedIdentity>>;

    async fn store_identity(
        &self,
        principal_id: &str,
        served_by: &str,
        identity: &str,
    ) -> RepoResult<()>;
}
