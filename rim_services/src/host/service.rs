use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use uuid::Uuid;

use crate::host::dto::{
    ConversationPredecessorDto, ForwardOutcome, ForwardRequest, HostCleanup, IdentityPush,
    RemoteClaim, RemoteKeyPackage, RemoteMemberRef, ResolvedMember, StreamHeadDto,
};
use crate::relay::dto::{IdentityDocument, OutboxPage};
use crate::{Actor, ServiceResult};

/// This server as the host of spaces with members from other servers: the direct space,
/// remote users and their identities, forwarded device requests, event stream heads and the
/// outbox other relays pull.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait HostService: Send + Sync {
    /// The relay's direct space.
    fn direct_space_id(&self) -> ServerId;

    /// Whether a user is someone whose relay is another server.
    async fn is_remote_user(&self, user_id: UserId) -> ServiceResult<bool>;

    /// Make sure a local user is a member of the direct space (a guest).
    async fn join_direct_space(&self, user_id: UserId) -> ServiceResult<()>;

    /// The user for a person a local user adds to a conversation: a local user, or a remote
    /// one whose identity is fetched from the first of their relays that answers and stored.
    /// Either way they become a member of the direct space.
    async fn ensure_remote_member(
        &self,
        actor: Actor,
        member: RemoteMemberRef,
    ) -> ServiceResult<ResolvedMember>;

    /// Record the conversation a successor replaces.
    async fn set_predecessor(
        &self,
        conversation_id: ConversationId,
        host_server_id: String,
        predecessor_id: Uuid,
    ) -> ServiceResult<()>;

    /// The conversation `conversation_id` succeeds, if it is a successor.
    async fn predecessor(
        &self,
        conversation_id: ConversationId,
    ) -> ServiceResult<Option<ConversationPredecessorDto>>;

    /// Apply an identity the relay `peer_server_id` at `peer_origin` pushed, for a principal
    /// held here as remote. The relay must speak for the principal: their newest stored card
    /// lists its origin, the pushed card (newer than the stored one) lists it, it is their home
    /// relay while no card is stored, or it holds an active delegation of one of their devices;
    /// otherwise nothing is stored and the answer is `NotDelegated`.
    ///
    /// # Errors
    /// `Validation` when the document does not verify or its key log does not extend the
    /// stored one.
    async fn apply_identity(
        &self,
        peer_server_id: String,
        peer_origin: String,
        document: IdentityDocument,
    ) -> ServiceResult<IdentityPush>;

    /// Refetch remote principals whose stored identity is over an hour old, from the relays
    /// their newest card lists and then their home relay. Failures keep the stored copy.
    async fn refresh_remote(&self, principal_ids: Vec<String>) -> ServiceResult<()>;

    /// Fetch a principal held here as remote from `from_relay`, or else from the relays their
    /// newest card lists and then their home relay, and store what verifies. What `from_relay`
    /// serves is stored only when it speaks for the principal, as for `apply_identity`. Returns
    /// whether an identity was stored; a relay that is down keeps the stored copy.
    async fn refresh_identity(
        &self,
        principal_id: String,
        from_relay: Option<String>,
    ) -> ServiceResult<bool>;

    /// Check a forwarded request: the device request and its `seq`, and that the relay may
    /// carry the user's traffic.
    async fn authenticate_forward(&self, request: ForwardRequest) -> ForwardOutcome;

    /// The current `seq` of each named stream of a space the caller can see.
    async fn stream_heads(
        &self,
        actor: Actor,
        server_id: ServerId,
        streams: Vec<String>,
    ) -> ServiceResult<Vec<StreamHeadDto>>;

    /// A relay's outbox above `after`, for `GET /s2s/v1/outbox`.
    async fn outbox_page(
        &self,
        peer_server_id: String,
        after: i64,
        limit: i64,
    ) -> ServiceResult<OutboxPage>;

    /// Delete outbox rows older than 7 days and accepted request `seq`s that left the window.
    async fn cleanup(&self) -> ServiceResult<HostCleanup>;

    /// Whether `actor` may start a conversation in the direct space with `user_ids`:
    /// `Forbidden` when one is on a server the operator's outbound DM setting does not admit,
    /// or a local one's DM settings refuse the actor; `Validation` naming the time left while
    /// `actor`'s account is in probation and one is a stranger to them, someone neither their
    /// contact nor in a workspace with them, whom the conversation would send a DM request.
    async fn ensure_may_request(&self, actor: Actor, user_ids: Vec<UserId>) -> ServiceResult<()>;
}

/// Claims key packages of remote users' devices from their relays.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RemoteKeyPackages: Send + Sync {
    /// `None` when the target user is local; otherwise what their relay handed out.
    async fn claim(&self, claim: RemoteClaim) -> ServiceResult<Option<Vec<RemoteKeyPackage>>>;
}

/// Whether a relay holds an active delegation a device signed for a space on this host, which
/// lets it forward that device's requests.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DelegationCheck: Send + Sync {
    async fn delegated(
        &self,
        user_id: UserId,
        device_key_id: rim_core::key_identity::model::DeviceKeyId,
        relay_server_id: &str,
    ) -> ServiceResult<bool>;
}
