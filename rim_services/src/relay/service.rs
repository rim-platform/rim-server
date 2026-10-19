use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::relay::model::{AbuseReportId, DmRequestId, DmRequestStatus};
use rim_core::user::model::UserId;
use tokio::sync::broadcast;

use crate::e2ee::command::MlsCaller;
use crate::operator::OperatorCaller;
use crate::relay::dto::{
    AbuseReportDto, ContactDto, DeliverItem, DeliverResponse, DmRequestDto, FileAbuseReport,
    IdentityDocument, PeerClaimRequest, PeerClaimResponse, PrincipalBlockDto,
    PrincipalBlockOutcome, PrincipalRecordDto, PrincipalUnblockOutcome, RelayCleanup,
    RelayItemPageDto, RelayNotice, RemoteIdentityDto, RemoteServerDto, SignedJson,
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

    /// Send a notice to a local user's devices online.
    fn publish(&self, notice: RelayNotice);

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

    /// A local user's identity as served to the server `peer_server_id`: `None` for anyone
    /// else, and for a server the operator's inbound DM setting does not admit unless the user
    /// holds an active subscription to a workspace it hosts.
    async fn identity_for_peer(
        &self,
        peer_server_id: String,
        principal_id: String,
    ) -> ServiceResult<Option<IdentityDocument>>;

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

    /// Hand out key packages of a local user's devices to a host; `Forbidden` for a host the
    /// operator's inbound DM setting does not admit unless the user holds an active
    /// subscription to a workspace it hosts.
    async fn claim_for_peer(
        &self,
        peer_server_id: String,
        request: PeerClaimRequest,
    ) -> ServiceResult<PeerClaimResponse>;

    /// File an event of this server's direct space, or of a DM in a workspace it hosts, for
    /// local members through the request inbox: an invite files by the member's DM settings (a
    /// request is held), and a pending request holds the conversation's events. What a member
    /// accepted flows live, not through the queue.
    async fn file_local(&self, envelope: SignedJson, recipients: Vec<String>) -> ServiceResult<()>;

    /// Whether a conversation stays out of a local user's list and live events: its request is
    /// pending or ignored, or the user blocked the direct-space conversation's creator.
    async fn conversation_hidden(
        &self,
        user_id: UserId,
        conversation_id: uuid::Uuid,
    ) -> ServiceResult<bool>;

    /// Whether a local user's DM request for a conversation is pending.
    async fn request_pending(
        &self,
        user_id: UserId,
        conversation_id: uuid::Uuid,
    ) -> ServiceResult<bool>;

    /// Local users whose request for a direct-space conversation is pending, by principal id.
    async fn pending_holders(&self, conversation_id: uuid::Uuid) -> ServiceResult<Vec<String>>;

    /// Pull what a host's outbox holds above what was received from it. Returns how many items
    /// were taken.
    async fn pull_outbox(&self, peer_server_id: String) -> ServiceResult<u64>;

    /// Push a local user's identity to the hosts that know them, after a change to their card,
    /// devices or key log.
    async fn identity_changed(&self, user_id: UserId) -> ServiceResult<()>;

    /// Delete acked and expired items, orphaned envelopes and expired requests.
    async fn cleanup(&self) -> ServiceResult<RelayCleanup>;
}

/// The operator's abuse tools: principals this server refuses (their sessions are ended, their
/// signups, logins and deliveries refused) and the abuse reports users and the abuse address
/// file. Each method takes who runs it and refuses a user who is not an instance operator with
/// `Forbidden`.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RelayAbuseService: Send + Sync {
    /// Ban a principal for `reason`, in one transaction: block it, ignore its pending DM
    /// requests, and for the account holding it here end its sessions, revoke its invites that
    /// have uses left, block its email and record the reason as a resolved abuse report. With
    /// `cascade`, every account its invites created is banned the same way, recursively, with
    /// the reason suffixed "(invited by <username>)". Returns one outcome per account, the
    /// principal's first, then breadth first.
    async fn block_principal(
        &self,
        by: OperatorCaller,
        principal_id: String,
        reason: String,
        cascade: bool,
    ) -> ServiceResult<Vec<PrincipalBlockOutcome>>;

    /// Lift a principal's block and its account's email block. Sessions and invites stay
    /// ended.
    async fn unblock_principal(
        &self,
        by: OperatorCaller,
        principal_id: String,
    ) -> ServiceResult<PrincipalUnblockOutcome>;

    /// The account holding a principal here, its inviter, its invite tree, its open reports
    /// and its block.
    async fn principal_record(
        &self,
        by: OperatorCaller,
        principal_id: String,
    ) -> ServiceResult<PrincipalRecordDto>;

    /// Every block, newest first.
    async fn principal_blocks(&self, by: OperatorCaller) -> ServiceResult<Vec<PrincipalBlockDto>>;

    /// File a report from the abuse address.
    async fn file_report(
        &self,
        by: OperatorCaller,
        report: FileAbuseReport,
    ) -> ServiceResult<AbuseReportDto>;

    /// Reports, oldest first; only unresolved ones when `open_only`.
    async fn reports(
        &self,
        by: OperatorCaller,
        open_only: bool,
    ) -> ServiceResult<Vec<AbuseReportDto>>;

    /// Mark a report resolved. `None` when there is no such report.
    async fn resolve_report(
        &self,
        by: OperatorCaller,
        id: AbuseReportId,
    ) -> ServiceResult<Option<AbuseReportDto>>;
}
