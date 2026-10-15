use async_trait::async_trait;
use chrono::{DateTime, Utc};
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::conversation::model::ConversationId;
use crate::host::model::{
    DelegationStored, NewDelegation, NewRemoteUser, OutboxRow, PeerDelivery, RemoteMember,
    SeqDecision, UserKind,
};
use crate::key_identity::model::DeviceKeyId;
use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;

/// What a host keeps for spaces with members from other servers: the direct space, remote
/// users, event streams, the outbox per relay and the `seq` window of forwarded requests.
///
/// [`HostRepository::next_stream_seq`] and [`HostRepository::insert_outbox`] run in the
/// caller's transaction, so a stream's `seq` is taken only when its outbox rows commit.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait HostRepository: Send + Sync {
    /// The direct space, created with `name` when missing.
    async fn ensure_direct_space(&self, name: &str) -> RepoResult<ServerId>;

    async fn direct_space(&self) -> RepoResult<Option<ServerId>>;

    /// Add a user to a space as a guest. Returns whether they were added.
    async fn join_as_guest(&self, server_id: ServerId, user_id: UserId) -> RepoResult<bool>;

    async fn user_kind(&self, user_id: UserId) -> RepoResult<Option<UserKind>>;

    /// The user holding a principal, local or remote; a purged account is nobody.
    async fn find_user(&self, principal_id: &str) -> RepoResult<Option<(UserId, UserKind)>>;

    /// # Errors
    /// `Conflict` when the principal or the username is taken.
    async fn create_remote_user(&self, user: NewRemoteUser) -> RepoResult<UserId>;

    /// Update a remote user's display name and home relay; `None` leaves a field unchanged.
    async fn update_remote_user(
        &self,
        user_id: UserId,
        display_name: Option<String>,
        home_relay: Option<String>,
    ) -> RepoResult<()>;

    /// Turn a remote user into a local one with a username and display name of their choice,
    /// and the normalised `email` their signup took, if any.
    async fn make_local(
        &self,
        user_id: UserId,
        username: &str,
        display_name: &str,
        email: Option<&str>,
    ) -> RepoResult<()>;

    async fn home_relay(&self, user_id: UserId) -> RepoResult<Option<String>>;

    /// The active pinned peer at an origin.
    async fn peer_at_origin(&self, origin: &str) -> RepoResult<Option<String>>;

    /// The remote members of a space.
    async fn remote_members(&self, server_id: ServerId) -> RepoResult<Vec<RemoteMember>>;

    /// Mark a remote user's device active for encryption (as if it had just uploaded key
    /// packages) or not.
    async fn set_device_mls_active(
        &self,
        device_key_id: DeviceKeyId,
        active: bool,
    ) -> RepoResult<()>;

    /// Whether a user shares a workspace (not the direct space) with the user holding a
    /// principal.
    async fn shares_workspace(&self, user_id: UserId, principal_id: &str) -> RepoResult<bool>;

    async fn set_conversation_predecessor(
        &self,
        conversation_id: ConversationId,
        host_server_id: &str,
        predecessor_id: uuid::Uuid,
    ) -> RepoResult<()>;

    /// The conversation a successor replaces: its host's server id and its id there.
    async fn conversation_predecessor(
        &self,
        conversation_id: ConversationId,
    ) -> RepoResult<Option<(String, uuid::Uuid)>>;

    /// Take the next `seq` of a stream: 1 for its first event.
    async fn next_stream_seq(&self, workspace_id: ServerId, stream: &str) -> RepoResult<i64>;

    async fn insert_outbox(
        &self,
        peer_server_id: &str,
        item: &str,
        size_bytes: i32,
    ) -> RepoResult<i64>;

    /// Relays with outbox rows above what they acknowledged, whose next attempt is due.
    async fn outbox_peers_due(&self, now: DateTime<Utc>) -> RepoResult<Vec<String>>;

    /// A relay's outbox rows above `after`, oldest first: at most `limit` rows and, past the
    /// first, at most `max_bytes` in all.
    async fn outbox_after(
        &self,
        peer_server_id: &str,
        after: i64,
        limit: i64,
        max_bytes: i64,
    ) -> RepoResult<Vec<OutboxRow>>;

    /// How far a relay has acknowledged; zero values before its first delivery.
    async fn delivery(&self, peer_server_id: &str) -> RepoResult<PeerDelivery>;

    /// Record an acknowledgement through `through` and reset the failure count.
    async fn record_delivered(&self, peer_server_id: &str, through: i64) -> RepoResult<()>;

    async fn record_failure(
        &self,
        peer_server_id: &str,
        failures: i32,
        next_attempt_at: DateTime<Utc>,
    ) -> RepoResult<()>;

    /// The current `seq` of each named stream that has one.
    async fn stream_heads(
        &self,
        workspace_id: ServerId,
        streams: &[String],
    ) -> RepoResult<Vec<(String, i64)>>;

    /// Delete outbox rows created before `before`. Returns how many.
    async fn delete_outbox_before(&self, before: DateTime<Utc>) -> RepoResult<u64>;

    /// Accept a forwarded request's `seq` from a device, or refuse it as a replay: refused when
    /// it was accepted before, or is at or below `highest - window`. An accepted `seq` raises
    /// the device's highest and is remembered until `seen_until`.
    async fn check_request_seq(
        &self,
        device_key_id: DeviceKeyId,
        seq: i64,
        window: i64,
        seen_until: DateTime<Utc>,
    ) -> RepoResult<SeqDecision>;

    /// Forget accepted `seq`s past their time that have also fallen below their device's
    /// window. Returns how many.
    async fn purge_request_seen(&self, now: DateTime<Utc>, window: i64) -> RepoResult<u64>;
}

/// Delegations relays hold for members' devices: one row per workspace, device and relay. A
/// delegation is active while it has not ended and has not expired; a relay receives a
/// workspace's events for a member, and forwards their device's requests, while it holds an
/// active one.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DelegationRepository: Send + Sync {
    /// Store a delegation, replacing the row for its workspace, device and relay unless that
    /// row is active and runs longer.
    async fn store(
        &self,
        delegation: NewDelegation,
        now: DateTime<Utc>,
    ) -> RepoResult<DelegationStored>;

    /// Whether the row for a workspace, device and relay holds an active delegation.
    async fn is_active(
        &self,
        workspace_id: ServerId,
        device_key_id: DeviceKeyId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Active delegations a relay holds here.
    async fn count_active_for_relay(
        &self,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<i64>;

    /// Whether a relay holds an active delegation of the device for a workspace the device's
    /// user is still a member of.
    async fn device_delegated(
        &self,
        device_key_id: DeviceKeyId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<bool>;

    /// Members of a workspace, local or remote, with an active delegation there.
    async fn delegated_members(
        &self,
        workspace_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<RemoteMember>>;

    /// The relays holding an active delegation of each user for a workspace.
    async fn delegated_relays(
        &self,
        workspace_id: ServerId,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<(UserId, String)>>;

    /// The workspaces where a relay holds an active delegation of the user.
    async fn delegated_workspaces(
        &self,
        user_id: UserId,
        relay_server_id: &str,
        now: DateTime<Utc>,
    ) -> RepoResult<Vec<ServerId>>;

    /// End a user's delegations for a workspace; only those for `relay_server_id` when given.
    /// Returns how many.
    async fn end_for_user<'a>(
        &self,
        workspace_id: ServerId,
        user_id: UserId,
        relay_server_id: Option<&'a str>,
    ) -> RepoResult<u64>;

    /// End every delegation a device signed. Returns how many.
    async fn end_for_device(&self, device_key_id: DeviceKeyId) -> RepoResult<u64>;

    /// End delegations that expired, whose device was revoked, or whose user is no longer a
    /// member of the workspace. Returns how many.
    async fn end_lapsed(&self, now: DateTime<Utc>) -> RepoResult<u64>;
}
