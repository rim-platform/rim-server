use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::relay::model::{DmRequestId, DmRequestStatus};
use rim_core::user::model::UserId;
use tokio::sync::broadcast;

use crate::e2ee::command::MlsCaller;
use crate::relay::dto::{
    ContactDto, DeliverItem, DeliverResponse, DmRequestDto, IdentityDocument, PeerClaimRequest,
    PeerClaimResponse, RelayCleanup, RelayItemPageDto, RelayNotice, RemoteIdentityDto,
    RemoteServerDto, SignedJson,
};
use crate::{Actor, ServiceResult};

/// This server as its users' relay: contacts, blocks and the request inbox, the queue of host
/// events with per-device cursors, identities served to and fetched from other servers, and
/// what other servers deliver.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RelayService: Send + Sync {
    async fn contacts(&self, actor: Actor) -> ServiceResult<Vec<ContactDto>>;

    /// Add or update a contact. Relay URLs follow the device-link endpoint rules.
    async fn add_contact(
        &self,
        actor: Actor,
        principal_id: String,
        relay_urls: Vec<String>,
    ) -> ServiceResult<ContactDto>;

    async fn remove_contact(&self, actor: Actor, principal_id: String) -> ServiceResult<bool>;

    /// Block a principal: its pending requests are ignored, its invites and claims refused.
    async fn block_principal(&self, actor: Actor, principal_id: String) -> ServiceResult<bool>;

    async fn unblock_principal(&self, actor: Actor, principal_id: String) -> ServiceResult<bool>;

    async fn dm_requests(
        &self,
        actor: Actor,
        status: Option<DmRequestStatus>,
    ) -> ServiceResult<Vec<DmRequestDto>>;

    /// Release the held items, let later ones flow, and add the sender as a contact.
    async fn accept_dm_request(&self, actor: Actor, id: DmRequestId)
    -> ServiceResult<DmRequestDto>;

    /// Delete the held items and drop later ones of the conversation.
    async fn ignore_dm_request(&self, actor: Actor, id: DmRequestId)
    -> ServiceResult<DmRequestDto>;

    /// Report a request as abuse; it is ignored too.
    async fn report_dm_request(
        &self,
        actor: Actor,
        id: DmRequestId,
        reason: String,
    ) -> ServiceResult<bool>;

    /// The session device's items above `after` (its cursor when `None`), at most `limit`.
    async fn relay_items(
        &self,
        caller: MlsCaller,
        after: Option<i64>,
        limit: i64,
    ) -> ServiceResult<RelayItemPageDto>;

    /// Move the session device's cursor up to `up_to`.
    async fn ack_relay_items(&self, caller: MlsCaller, up_to: i64) -> ServiceResult<bool>;

    /// Delete the user's queue, requests, contacts and blocks here.
    async fn leave_relay(&self, actor: Actor) -> ServiceResult<bool>;

    /// Live notices for every local user; subscribers filter by user.
    fn subscribe(&self) -> broadcast::Receiver<RelayNotice>;

    /// Check a device request a local user's session sends through this relay: it names the
    /// session's principal and device, which is not revoked, and its signature verifies.
    /// Returns the host it names.
    async fn check_outgoing_request(
        &self,
        user_id: UserId,
        device_key_id: Option<DeviceKeyId>,
        payload: String,
        signature: String,
    ) -> ServiceResult<String>;

    /// A local user's identity as served to other servers; `None` for anyone else.
    async fn local_identity(&self, principal_id: String)
    -> ServiceResult<Option<IdentityDocument>>;

    /// A principal's identity: a local one directly, else fetched from `relay_url` (or the
    /// relay that served it before) and cached for 24 hours.
    async fn remote_identity(
        &self,
        actor: Actor,
        principal_id: String,
        relay_url: Option<String>,
    ) -> ServiceResult<RemoteIdentityDto>;

    /// Pin the server at `origin` and return its discovery document.
    async fn remote_server(&self, actor: Actor, origin: String) -> ServiceResult<RemoteServerDto>;

    /// Store what a host delivered. Items at or below what was received from it before are
    /// skipped.
    async fn deliver(
        &self,
        peer_server_id: String,
        items: Vec<DeliverItem>,
    ) -> ServiceResult<DeliverResponse>;

    /// Hand out key packages of a local user's devices to a host.
    async fn claim_for_peer(
        &self,
        peer_server_id: String,
        request: PeerClaimRequest,
    ) -> ServiceResult<PeerClaimResponse>;

    /// File an event of this server's direct space for local members through the request
    /// inbox: an invite from a stranger files a request and is held, and a pending request holds
    /// the conversation's events. What a member already accepted flows live, not through the
    /// queue.
    async fn file_local(&self, envelope: SignedJson, recipients: Vec<String>) -> ServiceResult<()>;

    /// Whether a conversation stays out of a local user's list and live events: its request is
    /// pending or ignored, or the user blocked the direct-space conversation's creator.
    async fn conversation_hidden(
        &self,
        user_id: UserId,
        conversation_id: uuid::Uuid,
    ) -> ServiceResult<bool>;

    /// Local users whose request for a direct-space conversation is pending, by principal id.
    async fn pending_holders(&self, conversation_id: uuid::Uuid) -> ServiceResult<Vec<String>>;

    /// Pull what a host's outbox holds above what was received from it. Returns how many items
    /// were taken.
    async fn pull_outbox(&self, peer_server_id: String) -> ServiceResult<u64>;

    /// Push a local user's identity to the hosts that know them, after a card or device change.
    async fn identity_changed(&self, user_id: UserId) -> ServiceResult<()>;

    /// Delete acked and expired items, orphaned envelopes and expired requests.
    async fn cleanup(&self) -> ServiceResult<RelayCleanup>;
}
