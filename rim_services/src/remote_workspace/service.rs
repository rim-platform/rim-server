use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::key_identity::model::DeviceKeyId;
use rim_core::user::model::UserId;
use uuid::Uuid;

use crate::relay::dto::SignedJson;
use crate::remote_workspace::dto::{
    CancelEntry, JoinAnswer, JoinChallengeDto, JoinChallengeRequest, JoinRequest, PresenceUpdate,
    RemoteJoinInput, RemoteJoinResultDto, RemoteSubscriptionDto, RemoteWorkspaceCleanup,
    RevocationFanoutDto, SubscriptionResult,
};
use crate::{Actor, ServiceResult};

/// This server as the host of workspaces with members whose relay is elsewhere: joins through
/// a relay, the delegations that let a relay carry a member's traffic, presence the relay
/// reports, and revocations relays and apps bring.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceHostService: Send + Sync {
    /// A single-use login challenge for a person joining through `peer_server_id`, with the
    /// workspace the request's invite or public link names when it names one. The invite is
    /// checked as a preview checks it and is not used; the link as a link preview checks it,
    /// with the workspace's name and the link's mode.
    ///
    /// # Errors
    /// `Validation` for both an invite and a link; `NotFound` for an unknown invite and for
    /// every invalid link; `Forbidden` for an invite that is revoked, used up or expired, and
    /// for a link that does not admit accounts from other servers; `RateLimited` while the
    /// relay is cut off from link lookups.
    async fn join_challenge(
        &self,
        peer_server_id: String,
        request: JoinChallengeRequest,
    ) -> ServiceResult<JoinChallengeDto>;

    /// Admit a person a relay presents: verify their identity and proof, join them by invite,
    /// through a public link (or as a current member), store them, and accept the delegation.
    /// A public link that takes requests files a join request instead and accepts no
    /// delegation.
    ///
    /// # Errors
    /// `Validation` for an identity, proof or delegation that does not verify; the refusals of
    /// `joinServerByInvite` for the invite and those of a join through a public link for the
    /// link; `Forbidden` for a delegation naming another relay.
    async fn join(&self, peer_server_id: String, request: JoinRequest)
    -> ServiceResult<JoinAnswer>;

    /// Accept or refuse each delegation a relay presents, in order.
    async fn subscribe(
        &self,
        peer_server_id: String,
        delegations: Vec<SignedJson>,
    ) -> ServiceResult<Vec<SubscriptionResult>>;

    /// End a relay's delegations for the named users and workspaces. Returns how many ended.
    async fn cancel(&self, peer_server_id: String, entries: Vec<CancelEntry>)
    -> ServiceResult<u64>;

    /// Apply presence a relay reports for members it holds delegations of. Returns how many
    /// updates applied.
    async fn presence(
        &self,
        peer_server_id: String,
        updates: Vec<PresenceUpdate>,
    ) -> ServiceResult<u64>;

    /// Apply a device revocation against the stored key log, end the device's delegations and
    /// sessions, and answer the signed acknowledgement; a repeat gets the same one.
    /// `peer_server_id` is the relay that brought it, `None` when the app did.
    async fn apply_revocation(
        &self,
        peer_server_id: Option<String>,
        principal_id: Option<String>,
        revocation: SignedJson,
    ) -> ServiceResult<SignedJson>;

    /// [`Self::apply_revocation`] for a revocation an app brings straight to this server, with no
    /// session: every attempt counts against the caller's IP.
    ///
    /// # Errors
    /// `RateLimited` once the IP has made too many attempts, and those of `apply_revocation`.
    async fn apply_revocation_directly(
        &self,
        remote_ip: String,
        revocation: SignedJson,
    ) -> ServiceResult<SignedJson>;

    /// End expired delegations and those of revoked devices or departed members.
    async fn cleanup(&self) -> ServiceResult<RemoteWorkspaceCleanup>;
}

/// This server as the relay of people who are members of workspaces hosted elsewhere: joins,
/// delegation renewal, subscriptions, presence reports and revocation fan-out.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceRelayService: Send + Sync {
    /// Pin the host and fetch a join challenge from it, with the workspace the invite or public
    /// link names.
    async fn remote_join_challenge(
        &self,
        actor: Actor,
        host_server_id: String,
        host_origin: String,
        request: JoinChallengeRequest,
    ) -> ServiceResult<JoinChallengeDto>;

    /// Present the caller to a host with their proof and delegation, and record the
    /// subscription; or, when the host filed a join request instead, record the request so its
    /// decision reaches the caller.
    async fn join_remote_workspace(
        &self,
        actor: Actor,
        input: RemoteJoinInput,
    ) -> ServiceResult<RemoteJoinResultDto>;

    /// Present fresh delegations of the caller's devices to their hosts. One entry per
    /// delegation, in order.
    async fn renew_delegations(
        &self,
        actor: Actor,
        delegations: Vec<SignedJson>,
    ) -> ServiceResult<Vec<RemoteSubscriptionDto>>;

    /// The caller's subscriptions.
    async fn remote_subscriptions(&self, actor: Actor)
    -> ServiceResult<Vec<RemoteSubscriptionDto>>;

    /// Where one of the caller's device revocations stands at each host.
    async fn device_revocation_status(
        &self,
        actor: Actor,
        device_id: Uuid,
    ) -> ServiceResult<Vec<RevocationFanoutDto>>;

    /// Cancel the caller's subscriptions at every host, then delete their queue, DM requests,
    /// the join requests they filed at other hosts, contacts and blocks here.
    async fn leave_relay(&self, actor: Actor) -> ServiceResult<bool>;

    /// A local user's device opened its relay stream.
    fn stream_opened(&self, user_id: UserId);

    /// A local user's device closed its relay stream.
    fn stream_closed(&self, user_id: UserId);

    /// Send every revocation due for an attempt. Returns how many attempts were made.
    async fn send_due_revocations(&self) -> ServiceResult<u64>;

    /// Mark subscriptions past their expiry as lapsed, and forget join requests filed at other
    /// hosts past theirs.
    async fn cleanup(&self) -> ServiceResult<RemoteWorkspaceCleanup>;
}

/// Starts sending a local device's revocation to every host holding its person, once the
/// revocation is committed.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait RevocationFanout: Send + Sync {
    async fn revocation_committed(
        &self,
        user_id: UserId,
        device_key_id: DeviceKeyId,
    ) -> ServiceResult<()>;
}
