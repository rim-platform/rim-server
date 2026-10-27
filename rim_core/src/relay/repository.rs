use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use uuid::Uuid;

use crate::key_identity::model::DeviceKeyId;
use crate::relay::model::{
    AbuseReport, AbuseReportFilter, AbuseReportId, BlockedPrincipal, CachedIdentity, Contact,
    DmRequest, DmRequestId, DmRequestStatus, DroppedSpace, DueFanout, FanoutStatus, NewAbuseReport,
    NewDmRequest, NewRelayEnvelope, NewRelayItem, PrincipalBlock, RelayItemRecord, RelayQueueStats,
    RelaySubscription, RevocationFanout, SubscriptionStatus,
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

    /// Bytes queued for a user's devices; held items are not counted.
    async fn queued_bytes(&self, user_id: UserId) -> RepoResult<i64>;

    /// Bytes of the items a user's requests hold, whatever their status: the invites kept with
    /// ignored requests included.
    async fn user_held_bytes(&self, user_id: UserId) -> RepoResult<i64>;

    /// Items and bytes queued for every user, split into waiting and held.
    async fn queue_stats(&self) -> RepoResult<RelayQueueStats>;

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

    /// Delete queued items past their expiry. Items a request holds go with the request.
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

    /// A user's pending requests, cleared ones not counted.
    async fn count_pending_requests(&self, user_id: UserId) -> RepoResult<i64>;

    /// Requests from a principal created since `since`, for every local user.
    async fn count_requests_from(
        &self,
        principal_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<i64>;

    /// When the oldest of the requests [`Self::count_requests_from`] counts was filed; `None`
    /// when there is none.
    async fn oldest_request_from(
        &self,
        principal_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<Option<DateTime<Utc>>>;

    /// Requests a host delivered since `since`, for every local user.
    async fn count_requests_via_host(
        &self,
        host_server_id: &str,
        since: DateTime<Utc>,
    ) -> RepoResult<i64>;

    /// Move a pending, unexpired request to `status`, expiring at `expires_at`. Returns `None`
    /// when the request is not pending or has expired.
    async fn decide_request(
        &self,
        id: DmRequestId,
        status: DmRequestStatus,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<Option<DmRequest>>;

    /// Release a request's held items to the queue, each kept at least until `expires_at`.
    /// Returns how many.
    async fn release_held(
        &self,
        request_id: DmRequestId,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Delete a request's held items but its first, the invite, which stays with the request so
    /// the app can still verify the conversation's home. Returns how many.
    async fn delete_held(&self, request_id: DmRequestId) -> RepoResult<u64>;

    async fn count_held(&self, request_id: DmRequestId) -> RepoResult<i64>;

    /// Bytes of the items a request holds.
    async fn request_held_bytes(&self, request_id: DmRequestId) -> RepoResult<i64>;

    async fn held_items(&self, request_id: DmRequestId) -> RepoResult<Vec<RelayItemRecord>>;

    /// Whether the user ever accepted a request from the principal.
    async fn accepted_from(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// Whether the user holds an accepted request for a conversation the host homes, so takes
    /// part in its direct space.
    async fn accepted_at_host(&self, user_id: UserId, host_server_id: &str) -> RepoResult<bool>;

    /// Ignore every pending request from a principal, deleting their held items but each
    /// request's invite. Returns how many requests.
    async fn ignore_pending_from(
        &self,
        user_id: UserId,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Clear every ignored request from a principal that still holds its invite: pending again,
    /// decided as it is filed so it is neither listed nor counted, expiring at `expires_at`. The
    /// next item of its conversation files it again ([`Self::refile_request`]). Returns how many.
    async fn clear_ignored_from(
        &self,
        user_id: UserId,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// File a cleared request again as a new pending request, expiring at `expires_at`. Returns
    /// `false` when the request is not cleared.
    async fn refile_request(&self, id: DmRequestId, expires_at: DateTime<Utc>) -> RepoResult<bool>;

    async fn delete_expired_requests(&self, now: DateTime<Utc>) -> RepoResult<u64>;

    /// Clear, instead of letting them expire, the requests not accepted of conversations
    /// `own_server_id` hosts that expired before `now` while their user is still a member: pending,
    /// decided as filed, holding nothing, expiring at `expires_at`. One from a principal the user
    /// or the operator blocked stays ignored, keeps what it holds and expires at `expires_at`.
    /// Returns how many.
    async fn rearm_expired_local_requests(
        &self,
        own_server_id: &str,
        now: DateTime<Utc>,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// How many of a user's conversations from a host are accepted.
    async fn count_accepted_requests(
        &self,
        user_id: UserId,
        host_server_id: &str,
    ) -> RepoResult<i64>;

    /// Move a request's expiry to `expires_at` when one is given, and set the id its host gives
    /// its user to `host_user_id` when one is given and none is set.
    async fn refresh_request(
        &self,
        id: DmRequestId,
        expires_at: Option<DateTime<Utc>>,
        host_user_id: Option<Uuid>,
    ) -> RepoResult<()>;

    /// Delete a user's accepted row for a conversation of a host. Returns how many.
    async fn delete_accepted_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        conversation_id: Uuid,
    ) -> RepoResult<u64>;

    /// Local users still members of a conversation of this server's direct space whose request
    /// for it is pending, with their principal ids.
    async fn pending_holders(&self, conversation_id: Uuid) -> RepoResult<Vec<(UserId, String)>>;

    /// Local users still members of a conversation whose cleared request for it holds nothing,
    /// with their principal ids: the conversation's next event sends them its invite again.
    async fn uninvited_holders(&self, conversation_id: Uuid) -> RepoResult<Vec<(UserId, String)>>;

    /// Accept a user's cleared request for a conversation as if accepted when filed, deleting
    /// what it holds, expiring at `expires_at`. Returns whether there was one.
    async fn accept_cleared(
        &self,
        user_id: UserId,
        conversation_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Whether a local user's request for a conversation is pending.
    async fn request_pending(&self, user_id: UserId, conversation_id: Uuid) -> RepoResult<bool>;

    /// Whether a conversation stays out of a local user's list: their request for it is pending
    /// or ignored, or they blocked the direct-space conversation's creator.
    async fn conversation_hidden(&self, user_id: UserId, conversation_id: Uuid)
    -> RepoResult<bool>;

    /// Whether `user_id` is in a direct conversation whose other member blocked them.
    async fn blocked_by_dm_partner(
        &self,
        user_id: UserId,
        conversation_id: Uuid,
    ) -> RepoResult<bool>;

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

    async fn count_contacts(&self, user_id: UserId) -> RepoResult<i64>;

    /// Whether a user and the holder of a principal know each other: each lists the other as
    /// a contact (only a user of this server has a list here), or one accepted a DM request
    /// from the other here. A contact listed by one side only does not count.
    async fn knows(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// Returns whether the block is new.
    async fn block(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn unblock(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn is_blocked(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    /// A user's blocks, newest first, each with the undeleted account here holding the principal.
    async fn list_blocks(&self, user_id: UserId) -> RepoResult<Vec<BlockedPrincipal>>;

    /// Hosts that queued items or sent requests for a user, or hold an active subscription of
    /// theirs.
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

    /// Delete the cached identities fetched before `fetched_before`, except those of principals
    /// held here as remote users. Returns how many were deleted.
    async fn purge_identities(&self, fetched_before: DateTime<Utc>) -> RepoResult<u64>;
}

/// A relay's side of workspaces hosted elsewhere: its users' subscriptions, and revocations of
/// its users' devices on their way to every host holding them.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait SubscriptionRepository: Send + Sync {
    /// Record a subscription's status and expiry, one row per user, host and workspace.
    async fn upsert_subscription(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        status: SubscriptionStatus,
        expires_at: Option<DateTime<Utc>>,
    ) -> RepoResult<RelaySubscription>;

    /// A user's subscriptions, oldest first.
    async fn list_subscriptions(&self, user_id: UserId) -> RepoResult<Vec<RelaySubscription>>;

    /// Whether a user's subscription to a host's workspace is active and has not expired.
    async fn is_subscribed(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Hosts where the user holds an active subscription that has not expired.
    async fn subscribed_hosts(
        &self,
        user_id: UserId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<String>>;

    /// End every subscription of a user. Returns how many were not ended already.
    async fn end_subscriptions(&self, user_id: UserId) -> RepoResult<u64>;

    /// End one subscription of a user, to a host's workspace. Returns whether it was not ended
    /// already.
    async fn end_subscription(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
    ) -> RepoResult<bool>;

    /// Mark active subscriptions past their expiry as lapsed. Returns how many.
    async fn lapse_expired(&self, now: DateTime<Utc>) -> RepoResult<u64>;

    /// Record a join request a user filed at a host's workspace, until `expires_at`; a request
    /// for the same workspace replaces it.
    async fn record_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        request_id: Uuid,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Whether a user holds a recorded join request at a host's workspace that has not expired.
    async fn has_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Forget a user's join request at a host's workspace. Returns whether one was recorded.
    async fn delete_join_request(
        &self,
        user_id: UserId,
        host_server_id: &str,
        workspace_id: Uuid,
    ) -> RepoResult<bool>;

    /// Forget every join request of a user. Returns how many.
    async fn delete_join_requests(&self, user_id: UserId) -> RepoResult<u64>;

    /// Forget join requests past their expiry. Returns how many.
    async fn delete_expired_join_requests(&self, now: DateTime<Utc>) -> RepoResult<u64>;

    /// Start sending a device's revocation to `hosts`, first at `next_attempt_at`; a host it is
    /// on its way to already keeps its row. Returns how many rows were added.
    async fn insert_fanouts(
        &self,
        device_key_id: DeviceKeyId,
        hosts: &[String],
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// Pending fan-outs whose next attempt is due, at most `limit`, oldest first.
    async fn due_fanouts(&self, now: DateTime<Utc>, limit: i64) -> RepoResult<Vec<DueFanout>>;

    /// Record an attempt that got no acknowledgement.
    async fn record_fanout_attempt(
        &self,
        device_key_id: DeviceKeyId,
        host_server_id: &str,
        attempts: i32,
        status: FanoutStatus,
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// Record a host's acknowledgement.
    async fn record_fanout_ack(
        &self,
        device_key_id: DeviceKeyId,
        host_server_id: &str,
        attempts: i32,
        ack_payload: &str,
        ack_signature: &str,
    ) -> RepoResult<()>;

    /// Where a device's revocation stands at each host, by host.
    async fn list_fanouts(&self, device_key_id: DeviceKeyId) -> RepoResult<Vec<RevocationFanout>>;
}

/// The operator's abuse tools: principals this server refuses, and abuse reports.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RelayAbuseRepository: Send + Sync {
    /// Block a principal, or replace the reason of its block. Returns whether the block is new.
    async fn block_principal(&self, principal_id: &str, reason: &str) -> RepoResult<bool>;

    /// Returns whether a block was lifted.
    async fn unblock_principal(&self, principal_id: &str) -> RepoResult<bool>;

    async fn is_principal_blocked(&self, principal_id: &str) -> RepoResult<bool>;

    /// Every block, newest first.
    async fn list_principal_blocks(&self) -> RepoResult<Vec<PrincipalBlock>>;

    /// Ignore every pending DM request from a principal, for every local user, deleting their
    /// held items but each request's invite. Returns how many requests.
    async fn ignore_requests_from(
        &self,
        principal_id: &str,
        expires_at: DateTime<Utc>,
    ) -> RepoResult<u64>;

    /// File a report. `Conflict` when the reporter already reported the same DM request.
    async fn insert_report(&self, report: NewAbuseReport) -> RepoResult<AbuseReport>;

    /// The reports `filter` selects, oldest first.
    async fn list_reports(&self, filter: AbuseReportFilter) -> RepoResult<Vec<AbuseReport>>;

    /// Mark a report resolved, keeping the time of an earlier resolution. `None` when there is
    /// no such report.
    async fn resolve_report(&self, id: AbuseReportId) -> RepoResult<Option<AbuseReport>>;
}
