use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::conversation::model::ConversationId;
use rim_core::server::model::ServerId;
use rim_core::user::model::UserId;
use uuid::Uuid;

use crate::host::dto::{
    ForwardOutcome, ForwardRequest, HostCleanup, RemoteClaim, RemoteKeyPackage, RemoteMemberRef,
    StreamHeadDto,
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

    /// Make sure a local user is a member of the direct space (a guest).
    async fn join_direct_space(&self, user_id: UserId) -> ServiceResult<()>;

    /// The user for a person a local user adds to a conversation: a local user, or a remote
    /// one whose identity is fetched from the first of their relays that answers and stored.
    /// Either way they become a member of the direct space.
    async fn ensure_remote_member(
        &self,
        actor: Actor,
        member: RemoteMemberRef,
    ) -> ServiceResult<UserId>;

    /// Record the conversation a successor replaces.
    async fn set_predecessor(
        &self,
        conversation_id: ConversationId,
        host_server_id: String,
        predecessor_id: Uuid,
    ) -> ServiceResult<()>;

    /// Apply an identity a relay pushed, for a principal held here as remote. Returns whether
    /// it was applied.
    async fn apply_identity(
        &self,
        peer_server_id: String,
        document: IdentityDocument,
    ) -> ServiceResult<bool>;

    /// Refetch remote principals whose stored identity is over an hour old, from their home
    /// relay. Failures keep the stored copy.
    async fn refresh_remote(&self, principal_ids: Vec<String>) -> ServiceResult<()>;

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
}

/// Claims key packages of remote users' devices from their relays.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RemoteKeyPackages: Send + Sync {
    /// `None` when the target user is local; otherwise what their relay handed out.
    async fn claim(&self, claim: RemoteClaim) -> ServiceResult<Option<Vec<RemoteKeyPackage>>>;
}
