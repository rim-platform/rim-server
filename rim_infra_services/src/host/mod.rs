//! This server as the host of spaces with members from other servers: the direct space, remote
//! users and their identities, forwarded device requests, stream heads, the outbox and the
//! key packages claimed from remote users' relays.

pub mod outbox;

#[cfg(test)]
mod identity_tests;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use rim_core::auth::repository::SessionRepository;
use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::dm_policy::model::DmOutbound;
use rim_core::dm_policy::repository::DmPolicyRepository;
use rim_core::e2ee::repository::E2eeRepository;
use rim_core::host::model::{SeqDecision, UserKind};
use rim_core::host::repository::{DelegationRepository, HostRepository};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::repository::{RelayAbuseRepository, RelayRepository};
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_identity::{SignedStatement, UnverifiedDeviceRequest};
use rim_infra_db::{TransactionScope, begin_transaction};
use rim_s2s::SharedIdentity;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::dm_policy::dto::DmOperatorPolicy;
use rim_services::e2ee::service::E2eeService;
use rim_services::host::dto::{
    ConversationPredecessorDto, ForwardAuth, ForwardOutcome, ForwardRefusal, ForwardRequest,
    HostCleanup, IdentityPush, IdentityRefresh, RemoteClaim, RemoteKeyPackage, RemoteMemberRef,
    ResolvedMember, StreamHeadDto,
};
use rim_services::host::service::{DelegationCheck, HostService, RemoteKeyPackages};
use rim_services::limits::InstanceLimits;
use rim_services::relay::dto::{
    ClaimConversation, Claimer, DeliverItem, IdentityDocument, OutboxPage, PeerClaimRequest,
    PeerClaimResponse,
};
use rim_services::s2s::dto::{PeerLookup, S2sMethod, S2sRequest};
use rim_services::s2s::service::{PeerService, S2sClient};
use rim_services::session_revocation::SessionRevocations;
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::clock::Clock;
use crate::quota::{DailyCounter, Quotas};
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::relay::identity::{
    IdentityStore, Stored, VerifiedIdentity, store_remote, verify_identity,
};
use crate::relay::inbox::{RequestLimit, SENDER_WINDOW, request_limit};
use crate::relay::policy::{DmPolicy, PolicySource, outbound_refused};
use crate::relay::{
    MAX_DELIVER_BYTES, MAX_DELIVER_ITEMS, card_relays, check_relay_urls, fetch_from_relays,
    origin_of, relays_include_origin,
};
use crate::repos::AppRepos;

/// Name of the direct space.
pub const DIRECT_SPACE_NAME: &str = "Direct messages";
/// The `seq` window of device requests: 5 minutes in microseconds.
pub const REQUEST_SEQ_WINDOW: i64 = 300_000_000;
/// How long an accepted `seq` is remembered at least.
const REQUEST_SEEN_TTL: Duration = Duration::minutes(10);
/// How long outbox rows are kept.
const OUTBOX_RETENTION: Duration = Duration::days(7);
/// How old a remote identity may be before `principalDevices` refreshes it.
const REMOTE_IDENTITY_MAX_AGE: Duration = Duration::hours(1);
/// Most streams one `streamHeads` call names.
const MAX_STREAMS: usize = 256;

#[derive(Clone)]
pub struct HostRepos {
    pub host_repo: Arc<dyn HostRepository>,
    pub relay_repo: Arc<dyn RelayRepository>,
    pub key_identity_repo: Arc<dyn KeyIdentityRepository>,
    pub e2ee_repo: Arc<dyn E2eeRepository>,
    pub user_repo: Arc<dyn UserRepository>,
    pub conversation_repo: Arc<dyn ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn ServerMemberRepository>,
    pub delegation_repo: Arc<dyn DelegationRepository>,
    /// Remote users' direct sign-in sessions, ended when their identity recovers or a device
    /// is revoked.
    pub session_repo: Arc<dyn SessionRepository>,
    /// Principals this server refuses.
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
    /// Workspaces' DM rules and each user's choices.
    pub dm_policy_repo: Arc<dyn DmPolicyRepository>,
}

pub struct DefaultHostService {
    repos: HostRepos,
    db: Option<DatabaseConnection>,
    identity: SharedIdentity,
    peers: Arc<dyn PeerService>,
    client: Arc<dyn S2sClient>,
    direct_space_id: ServerId,
    /// Forwarded requests per device.
    device_limiter: RegistrationRateLimiter,
    /// Told when a remote user's devices change. Set once the provider exists.
    e2ee_service: OnceLock<Arc<dyn E2eeService>>,
    /// Told about the sessions a stored identity ended. Set once the provider exists.
    session_revocations: OnceLock<Arc<dyn SessionRevocations>>,
    /// Delegations of workspace spaces; without one only the direct-space rule applies.
    delegations: Option<Arc<dyn DelegationCheck>>,
    /// Invites each user sends to people on other servers they do not know, per day.
    dm_invites: DailyCounter,
    /// DM requests one sender principal files on this relay per day.
    dm_requests_per_day: u32,
    /// The probation of new accounts, during which they send no DM request to a stranger.
    limits: InstanceLimits,
    clock: Clock,
    /// The operator's DM settings.
    dm_policy: DmOperatorPolicy,
}

fn upstream(server_id: &str) -> ServiceError {
    ServiceError::upstream(server_id)
}

/// The relay an identity document came from, and why it may speak for the principal. Only the
/// relay named here is recorded as the document's source and as the home relay fallback, and
/// only such a relay's unsigned device activity is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum IdentitySource {
    /// A relay that pushed or presented the document and speaks for the principal: their
    /// newest card lists it, the document's newer card lists it, it is their home relay while
    /// no card is stored, it holds an active delegation of one of their devices, or it
    /// presented a login proof one of their devices signed for this host.
    Authorized(String),
    /// A relay this host fetched the document from: one of the relays the principal's newest
    /// card lists or their home relay, or the relay a local user named for a principal not
    /// held here yet.
    Fetched(String),
}

impl IdentitySource {
    fn relay(&self) -> &str {
        match self {
            Self::Authorized(relay) | Self::Fetched(relay) => relay,
        }
    }
}

impl DefaultHostService {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: HostRepos,
        db: Option<DatabaseConnection>,
        identity: SharedIdentity,
        peers: Arc<dyn PeerService>,
        client: Arc<dyn S2sClient>,
        direct_space_id: ServerId,
        device_rate_per_min: u32,
    ) -> Self {
        Self {
            repos,
            db,
            identity,
            peers,
            client,
            direct_space_id,
            device_limiter: RegistrationRateLimiter::new(60, device_rate_per_min),
            e2ee_service: OnceLock::new(),
            session_revocations: OnceLock::new(),
            delegations: None,
            dm_invites: DailyCounter::per_day(u64::from(Quotas::default().dm_requests_per_day)),
            dm_requests_per_day: Quotas::default().dm_requests_per_day,
            limits: InstanceLimits::default(),
            clock: Clock::default(),
            dm_policy: DmOperatorPolicy::default(),
        }
    }

    /// Apply the operator's DM settings.
    pub fn with_dm_policy(mut self, dm_policy: DmOperatorPolicy) -> Self {
        self.dm_policy = dm_policy;
        self
    }

    /// Whether a local user may start DMs with people on `server_id`.
    fn admits_outbound(&self, server_id: &str) -> bool {
        crate::relay::policy::outbound(&self.dm_policy, &self.own_id(), server_id)
            == DmPolicy::Deliver
    }

    /// `Forbidden` unless one of a remote user's relays is a server local users may start DMs
    /// with.
    async fn ensure_outbound_to(&self, user_id: UserId) -> ServiceResult<()> {
        let card = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?;
        let home = self.repos.host_repo.home_relay(user_id).await?;
        let relays = Self::relays_of(
            self.repos.host_repo.as_ref(),
            &self.own_id(),
            card.as_ref().map(|c| c.payload.as_str()),
            home.as_deref(),
        )
        .await?;
        if relays.iter().any(|r| self.admits_outbound(r)) {
            Ok(())
        } else {
            Err(outbound_refused())
        }
    }

    /// How a local recipient's DM settings take a DM from `sender`: `Deliver` for a sender
    /// without a principal id, who files no request.
    ///
    /// # Errors
    ///
    /// `Forbidden` when the settings refuse it.
    async fn ensure_recipient_takes(
        &self,
        sender: UserId,
        recipient: UserId,
    ) -> ServiceResult<DmPolicy> {
        let Some(principal_id) = self.principal_of(sender).await? else {
            return Ok(DmPolicy::Deliver);
        };
        let own_id = self.own_id();
        let sender_server = match self.repos.host_repo.user_kind(sender).await? {
            Some(UserKind::Remote) => self
                .repos
                .host_repo
                .home_relay(sender)
                .await?
                .unwrap_or_default(),
            _ => own_id.clone(),
        };
        let policy = PolicySource {
            relay_repo: self.repos.relay_repo.as_ref(),
            dm_policy_repo: self.repos.dm_policy_repo.as_ref(),
            operator: &self.dm_policy,
            own_server_id: &own_id,
        }
        .inbound(recipient, &principal_id, &sender_server)
        .await?;
        if policy == DmPolicy::Refuse {
            return Err(ServiceError::Forbidden(
                "they do not take direct messages from you".to_string(),
            ));
        }
        Ok(policy)
    }

    /// The principal id of `user_id`; `None` for a user without one or an unknown user.
    async fn principal_of(&self, user_id: UserId) -> ServiceResult<Option<String>> {
        Ok(self
            .repos
            .user_repo
            .get_by_id(user_id)
            .await?
            .and_then(|u| u.principal_id))
    }

    /// Refuse a conversation by `actor` that files a DM request with each of the local users
    /// `requested` when the request inbox would refuse one: a recipient holding their most
    /// pending requests, or more requests than `actor` may file today on this relay.
    ///
    /// # Errors
    ///
    /// `Forbidden` naming the full inbox; `RateLimited` until the oldest of `actor`'s requests
    /// of the window leaves it.
    async fn refuse_requests_over_limits(
        &self,
        actor: Actor,
        requested: &[UserId],
    ) -> ServiceResult<()> {
        if requested.is_empty() {
            return Ok(());
        }
        let Some(principal_id) = self.principal_of(actor.user_id).await? else {
            return Ok(());
        };
        let since = self.clock.now() - SENDER_WINDOW;
        let filed = self
            .repos
            .relay_repo
            .count_requests_from(&principal_id, since)
            .await?;
        for (more, recipient) in (0_i64..).zip(requested) {
            let pending = self
                .repos
                .relay_repo
                .count_pending_requests(*recipient)
                .await?;
            match request_limit(pending, filed + more, i64::from(self.dm_requests_per_day)) {
                None => {}
                Some(RequestLimit::RecipientFull) => {
                    crate::metrics::rate_limited("dm_requests");
                    return Err(ServiceError::Forbidden(
                        "their request inbox is full".to_string(),
                    ));
                }
                Some(RequestLimit::SenderDaily) => {
                    crate::metrics::rate_limited("dm_requests");
                    let oldest = self
                        .repos
                        .relay_repo
                        .oldest_request_from(&principal_id, since)
                        .await?;
                    let retry =
                        oldest.map_or(SENDER_WINDOW.num_seconds(), |at| (at - since).num_seconds());
                    return Err(ServiceError::RateLimited {
                        retry_after_seconds: u64::try_from(retry.max(1)).unwrap_or(1),
                    });
                }
            }
        }
        Ok(())
    }

    /// The local users among `user_ids`, but `actor`, whose DM settings take a DM from `actor`
    /// as a request.
    ///
    /// # Errors
    ///
    /// `Forbidden` when one's settings refuse it.
    async fn requested_of(&self, actor: Actor, user_ids: &[UserId]) -> ServiceResult<Vec<UserId>> {
        let mut requested = Vec::new();
        for other in user_ids.iter().filter(|id| **id != actor.user_id) {
            if self.repos.host_repo.user_kind(*other).await? == Some(UserKind::Local)
                && self.ensure_recipient_takes(actor.user_id, *other).await? == DmPolicy::Request
            {
                requested.push(*other);
            }
        }
        Ok(requested)
    }

    /// Apply the per-user quotas: invites to people on other servers, and DM requests filed
    /// here, per day.
    pub fn with_quotas(mut self, quotas: Quotas) -> Self {
        self.dm_invites = DailyCounter::per_day(u64::from(quotas.dm_requests_per_day));
        self.dm_requests_per_day = quotas.dm_requests_per_day;
        self
    }

    pub fn with_limits(mut self, limits: InstanceLimits) -> Self {
        self.limits = limits;
        self
    }

    #[cfg(test)]
    pub(crate) fn with_clock(mut self, clock: Clock) -> Self {
        self.clock = clock;
        self
    }

    /// `Validation` while `actor`'s account is in probation and the holder of one of
    /// `principal_ids` is a stranger to them: someone they do not know (see
    /// [`RelayRepository::knows`]) who is not in a workspace with them.
    async fn refuse_strangers_in_probation(
        &self,
        actor: Actor,
        principal_ids: &[String],
    ) -> ServiceResult<()> {
        if self.limits.probation_hours.is_none() || principal_ids.is_empty() {
            return Ok(());
        }
        let user = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let now = self.clock.now();
        if self
            .limits
            .probation_ends_at(user.created_at, now)
            .is_none()
        {
            return Ok(());
        }
        for principal_id in principal_ids {
            let known = user.principal_id.as_ref() == Some(principal_id)
                || self
                    .repos
                    .relay_repo
                    .knows(actor.user_id, principal_id)
                    .await?
                || self
                    .repos
                    .host_repo
                    .shares_workspace(actor.user_id, principal_id)
                    .await?;
            if !known {
                return self.limits.ensure_out_of_probation(
                    user.created_at,
                    now,
                    "send a DM request to a stranger",
                );
            }
        }
        Ok(())
    }

    /// Let workspace delegations authorize forwarded requests too.
    pub fn with_delegations(mut self, delegations: Arc<dyn DelegationCheck>) -> Self {
        self.delegations = Some(delegations);
        self
    }

    /// The service told when a remote user's devices change.
    pub fn set_e2ee_service(&self, e2ee_service: Arc<dyn E2eeService>) {
        let _ = self.e2ee_service.set(e2ee_service);
    }

    /// Report the sessions a stored recovery or device revocation ends to `revocations`, so
    /// their live connections close.
    pub fn set_session_revocations(&self, revocations: Arc<dyn SessionRevocations>) {
        let _ = self.session_revocations.set(revocations);
    }

    async fn begin(&self) -> ServiceResult<(Option<TransactionScope>, HostRepos)> {
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_host(),
            None => self.repos.clone(),
        };
        Ok((scope, repos))
    }

    async fn commit(scope: Option<TransactionScope>) -> ServiceResult<()> {
        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }
        Ok(())
    }

    fn own_id(&self) -> String {
        self.identity.current().server_id().to_string()
    }

    /// Store a fetched or pushed identity as a remote user and remember where it came from.
    ///
    /// # Errors
    /// `Validation` when the document does not verify or its key log does not extend the
    /// stored one.
    pub(crate) async fn store_identity(
        &self,
        document: &IdentityDocument,
        source: IdentitySource,
    ) -> ServiceResult<Option<Stored>> {
        let verified = verify_identity(document).map_err(ServiceError::Validation)?;
        self.store_verified(document, &verified, source).await
    }

    /// [`Self::store_identity`] for a document that verified as `verified`.
    async fn store_verified(
        &self,
        document: &IdentityDocument,
        verified: &VerifiedIdentity,
        source: IdentitySource,
    ) -> ServiceResult<Option<Stored>> {
        let served_by = source.relay();
        let home_relay = self.home_relay_for(document, served_by).await?;
        let (scope, repos) = self.begin().await?;
        let stored = store_remote(
            &IdentityStore {
                host_repo: repos.host_repo.as_ref(),
                key_identity_repo: repos.key_identity_repo.as_ref(),
                session_repo: repos.session_repo.as_ref(),
                delegation_repo: repos.delegation_repo.as_ref(),
            },
            verified,
            home_relay.as_deref(),
        )
        .await?;
        Self::commit(scope).await?;
        if let Some(stored) = &stored {
            self.sessions_ended(stored, served_by);
        }
        if stored.is_some() {
            let text = serde_json::to_string(document).map_err(ServiceError::internal)?;
            self.repos
                .relay_repo
                .store_identity(&document.principal_id, served_by, &text)
                .await?;
        }
        if let Some(stored) = &stored
            && stored.devices_changed
            && !stored.created
            && let Some(e2ee) = self.e2ee_service.get()
            && let Err(e) = e2ee.devices_changed(stored.user_id).await
        {
            warn!(error = %e, user_id = %stored.user_id, "could not announce a remote user's device change");
        }
        Ok(stored)
    }

    /// Close the live connections of the sessions a committed identity ended, and record the
    /// recovery or revocations in the audit log.
    fn sessions_ended(&self, stored: &Stored, served_by: &str) {
        if let Some(revocations) = self.session_revocations.get() {
            if stored.recovered {
                revocations.revoke_user(stored.user_id);
            }
            for device in &stored.revoked_devices {
                revocations.revoke_device(*device);
            }
        }
        if stored.recovered {
            info!(
                target: "audit",
                event = "host.recovery_applied",
                user_id = %stored.user_id,
                peer_server_id = %served_by,
                ended_delegations = stored.ended_delegations,
                ended_sessions = stored.ended_sessions,
                "applied an identity recovery from another server"
            );
        } else if !stored.revoked_devices.is_empty() {
            info!(
                target: "audit",
                event = "host.identity_revocations_applied",
                user_id = %stored.user_id,
                peer_server_id = %served_by,
                devices = stored.revoked_devices.len(),
                ended_delegations = stored.ended_delegations,
                ended_sessions = stored.ended_sessions,
                "applied device revocations in an identity from another server"
            );
        }
    }

    /// Pin the relays a document's card lists, so events can be addressed to them. Returns the
    /// first that pins.
    async fn pin_card_relays(&self, document: &IdentityDocument) -> Option<String> {
        let card = document.card.as_ref()?;
        let mut first = None;
        for url in card_relays(&card.payload) {
            let Some(origin) = origin_of(&url) else {
                continue;
            };
            match self.peers.pin(&origin, None).await {
                Ok(pinned) if pinned.server_id != self.own_id() => {
                    first.get_or_insert(pinned.server_id);
                }
                Ok(_) => {}
                Err(e) => warn!(origin = %origin, error = %e, "could not pin a relay a card lists"),
            }
        }
        first
    }

    /// The home relay to record with a stored identity: none, so the stored one stays, while
    /// the stored home relay holds an active delegation of the user; else the first relay of
    /// the document's card that pins; else `served_by`.
    async fn home_relay_for(
        &self,
        document: &IdentityDocument,
        served_by: &str,
    ) -> ServiceResult<Option<String>> {
        if let Some((user_id, UserKind::Remote)) = self
            .repos
            .host_repo
            .find_user(&document.principal_id)
            .await?
            && let Some(home) = self.repos.host_repo.home_relay(user_id).await?
            && !self
                .repos
                .delegation_repo
                .delegated_workspaces(user_id, &home, self.clock.now())
                .await?
                .is_empty()
        {
            return Ok(None);
        }
        Ok(Some(
            self.pin_card_relays(document)
                .await
                .unwrap_or_else(|| served_by.to_string()),
        ))
    }

    /// Whether the relay `relay` at `relay_origin` speaks for the remote user `user_id`, whose
    /// document verified as `verified`: their newest stored card lists the relay's origin; the
    /// document's card, of a higher version than the stored one, lists it; it is their home
    /// relay and no card is stored; or it holds an active delegation of one of their devices.
    async fn relay_speaks_for(
        &self,
        user_id: UserId,
        relay: &str,
        relay_origin: &str,
        verified: &VerifiedIdentity,
    ) -> ServiceResult<bool> {
        let stored = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?;
        match &stored {
            Some(card) if relays_include_origin(&card_relays(&card.payload), relay_origin) => {
                return Ok(true);
            }
            None if self.repos.host_repo.home_relay(user_id).await?.as_deref() == Some(relay) => {
                return Ok(true);
            }
            _ => {}
        }
        if let Some((card, _)) = &verified.card
            && stored
                .as_ref()
                .is_none_or(|s| card.version > u64::from(s.version))
            && relays_include_origin(&card.relays, relay_origin)
        {
            return Ok(true);
        }
        Ok(!self
            .repos
            .delegation_repo
            .delegated_workspaces(user_id, relay, self.clock.now())
            .await?
            .is_empty())
    }

    /// [`Self::relay_speaks_for`] for a relay known by its server id only: false when it is
    /// not pinned.
    async fn pinned_relay_speaks_for(
        &self,
        user_id: UserId,
        relay: &str,
        verified: &VerifiedIdentity,
    ) -> ServiceResult<bool> {
        match self.peers.resolve(relay, None, None).await {
            Ok(PeerLookup::Ready(keys)) => {
                self.relay_speaks_for(user_id, relay, &keys.origin, verified)
                    .await
            }
            Ok(_) => Ok(false),
            Err(e) => {
                warn!(peer_server_id = %relay, error = %e, "could not look up a relay's origin");
                Ok(false)
            }
        }
    }

    /// The relays this host fetches a remote user's identity from: those their newest card
    /// lists that are pinned, else their home relay, and then their home relay.
    async fn chosen_relays(&self, user_id: UserId) -> ServiceResult<Vec<String>> {
        let card = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?;
        let home = self.repos.host_repo.home_relay(user_id).await?;
        let mut relays = Self::relays_of(
            self.repos.host_repo.as_ref(),
            &self.own_id(),
            card.as_ref().map(|c| c.payload.as_str()),
            home.as_deref(),
        )
        .await?;
        if let Some(home) = home
            && home != self.own_id()
            && !relays.contains(&home)
        {
            relays.push(home);
        }
        Ok(relays)
    }

    /// The relays to send a remote user's traffic to: those their card lists that are pinned,
    /// else their home relay.
    pub(crate) async fn relays_of(
        host_repo: &dyn HostRepository,
        own_id: &str,
        card_payload: Option<&str>,
        home_relay: Option<&str>,
    ) -> ServiceResult<Vec<String>> {
        let mut relays = Vec::new();
        if let Some(card) = card_payload {
            for url in card_relays(card) {
                if let Some(origin) = origin_of(&url)
                    && let Some(server_id) = host_repo.peer_at_origin(&origin).await?
                    && server_id != own_id
                    && !relays.contains(&server_id)
                {
                    relays.push(server_id);
                }
            }
        }
        if relays.is_empty()
            && let Some(home) = home_relay
            && home != own_id
        {
            relays.push(home.to_string());
        }
        Ok(relays)
    }

    /// Whether the relay at `peer_origin` may carry a user's traffic without a delegation: the
    /// user is a member of the direct space and their newest card lists the relay's origin.
    async fn card_names_relay(&self, user_id: UserId, peer_origin: &str) -> ServiceResult<bool> {
        if self
            .repos
            .server_member_repo
            .get_member(self.direct_space_id, user_id)
            .await?
            .is_none()
        {
            return Ok(false);
        }
        let Some(card) = self
            .repos
            .key_identity_repo
            .get_profile_card(user_id)
            .await?
        else {
            return Ok(false);
        };
        Ok(relays_include_origin(
            &card_relays(&card.payload),
            peer_origin,
        ))
    }

    /// Count an invite `actor` sends to someone on another server against their daily quota,
    /// unless they know each other (see [`RelayRepository::knows`]).
    async fn count_dm_invite(&self, actor: Actor, principal_id: &str) -> ServiceResult<()> {
        if self
            .repos
            .relay_repo
            .knows(actor.user_id, principal_id)
            .await?
        {
            return Ok(());
        }
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = self
            .dm_invites
            .check_and_record(&actor.user_id.to_string(), Utc::now())
        {
            warn!(user_id = %actor.user_id, "a user's invites to other servers are over their daily quota");
            crate::metrics::rate_limited("dm_invites");
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }
        Ok(())
    }

    async fn check_forward(
        &self,
        request: &ForwardRequest,
        payload: String,
    ) -> Result<ForwardAuth, ForwardRefusal> {
        let internal = |e: ServiceError| {
            warn!(error = %e, "forwarded request check failed");
            ForwardRefusal::Internal
        };
        let signature = request
            .device_signature
            .clone()
            .ok_or_else(|| ForwardRefusal::BadRequest("Rim-Device-Signature is missing".into()))?;
        let statement = SignedStatement { payload, signature };
        let unverified = UnverifiedDeviceRequest::read(&statement)
            .map_err(|e| ForwardRefusal::BadRequest(e.to_string()))?;
        let forbidden = |why: &str| ForwardRefusal::Forbidden(why.to_string());
        let Some((user_id, _)) = self
            .repos
            .host_repo
            .find_user(&unverified.principal_id)
            .await
            .map_err(|e| internal(e.into()))?
        else {
            return Err(forbidden("unknown principal"));
        };
        if self
            .repos
            .relay_abuse_repo
            .is_principal_blocked(&unverified.principal_id)
            .await
            .map_err(|e| internal(e.into()))?
        {
            return Err(forbidden("this principal is blocked on this server"));
        }
        let device = self
            .repos
            .key_identity_repo
            .get_device(user_id, unverified.device_id)
            .await
            .map_err(|e| internal(e.into()))?
            .ok_or_else(|| forbidden("unknown device"))?;
        if device.revoked_at.is_some() {
            return Err(forbidden("the device is revoked"));
        }
        let key = crate::relay::identity::device_key(&device.public_key).map_err(internal)?;
        let verified = unverified
            .verify(&key)
            .map_err(|_| forbidden("the device signature does not verify"))?;
        verified
            .check_forwarded(
                &self.own_id(),
                &request.method,
                &request.path,
                &request.body,
            )
            .map_err(|e| ForwardRefusal::Forbidden(e.to_string()))?;
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = self
            .device_limiter
            .check_and_record(&device.id.to_string(), "forward", Utc::now())
        {
            crate::metrics::rate_limited("host_forward_device");
            return Err(ForwardRefusal::RateLimited {
                retry_after_seconds,
            });
        }
        let seq = i64::try_from(verified.seq).map_err(|_| forbidden("seq out of range"))?;
        match self
            .repos
            .host_repo
            .check_request_seq(
                device.id,
                seq,
                REQUEST_SEQ_WINDOW,
                Utc::now() + REQUEST_SEEN_TTL,
            )
            .await
            .map_err(|e| internal(e.into()))?
        {
            SeqDecision::Accepted => {}
            SeqDecision::Replay { highest } => {
                warn!(device_key_id = %device.id, "refused a replayed device request seq");
                return Err(ForwardRefusal::Replay { highest });
            }
        }
        let delegated = match &self.delegations {
            Some(check) => check
                .delegated(user_id, device.id, &request.peer_server_id)
                .await
                .map_err(internal)?,
            None => false,
        };
        if !delegated
            && !self
                .card_names_relay(user_id, &request.peer_origin)
                .await
                .map_err(internal)?
        {
            return Err(ForwardRefusal::NotDelegated);
        }
        Ok(ForwardAuth {
            user_id,
            device_key_id: device.id,
            principal_id: verified.principal_id,
        })
    }

    /// A principal's identity as `relay` serves it, if it answers with one for that principal.
    async fn fetch_document(&self, relay: &str, principal_id: &str) -> Option<IdentityDocument> {
        let response = self
            .client
            .send(S2sRequest {
                peer_server_id: relay.to_string(),
                method: S2sMethod::Get,
                path: format!("/principals/{principal_id}"),
                body: Vec::new(),
                content_type: None,
                headers: Vec::new(),
            })
            .await;
        match response {
            Ok(r) if r.status == 200 => serde_json::from_slice::<IdentityDocument>(&r.body)
                .ok()
                .filter(|d| d.principal_id == principal_id),
            _ => None,
        }
    }

    /// The relay a remote user's claims go to: the first of their relays that answers.
    async fn claim_from_relays(
        &self,
        relays: &[String],
        request: &PeerClaimRequest,
    ) -> ServiceResult<PeerClaimResponse> {
        let body = serde_json::to_vec(request).map_err(ServiceError::internal)?;
        let mut last = ServiceError::NotFound;
        for relay in relays {
            let response = self
                .client
                .send(S2sRequest {
                    peer_server_id: relay.clone(),
                    method: S2sMethod::Post,
                    path: "/key-packages/claim".to_string(),
                    body: body.clone(),
                    content_type: None,
                    headers: Vec::new(),
                })
                .await;
            match response {
                Ok(r) if r.status == 200 => {
                    return serde_json::from_slice(&r.body).map_err(|_| upstream(relay));
                }
                Ok(r) if r.status == 403 => {
                    return Err(ServiceError::Forbidden(
                        "the member's relay refused the claim".to_string(),
                    ));
                }
                Ok(r) if r.status == 429 => {
                    return Err(ServiceError::RateLimited {
                        retry_after_seconds: 60,
                    });
                }
                Ok(r) if r.status == 404 => last = ServiceError::NotFound,
                Ok(_) | Err(_) => last = upstream(relay),
            }
        }
        Err(last)
    }
}

#[async_trait]
impl HostService for DefaultHostService {
    fn direct_space_id(&self) -> ServerId {
        self.direct_space_id
    }

    async fn is_remote_user(&self, user_id: UserId) -> ServiceResult<bool> {
        Ok(self.repos.host_repo.user_kind(user_id).await? == Some(UserKind::Remote))
    }

    async fn join_direct_space(&self, user_id: UserId) -> ServiceResult<()> {
        self.repos
            .host_repo
            .join_as_guest(self.direct_space_id, user_id)
            .await?;
        Ok(())
    }

    #[instrument(name = "host.ensure_remote_member", skip_all, fields(actor_id = %actor.user_id))]
    async fn ensure_remote_member(
        &self,
        actor: Actor,
        member: RemoteMemberRef,
    ) -> ServiceResult<ResolvedMember> {
        if !rim_identity::is_principal_id(&member.principal_id) {
            return Err(ServiceError::Validation(
                "principalId is not a principal id".to_string(),
            ));
        }
        let (user_id, remote) = match self.repos.host_repo.find_user(&member.principal_id).await? {
            Some((user_id, UserKind::Local)) => (user_id, false),
            existing => {
                check_relay_urls(&member.relay_urls)?;
                if self.dm_policy.outbound == DmOutbound::None {
                    return Err(outbound_refused());
                }
                self.refuse_strangers_in_probation(
                    actor,
                    std::slice::from_ref(&member.principal_id),
                )
                .await?;
                self.count_dm_invite(actor, &member.principal_id).await?;
                let fetched = fetch_from_relays(
                    self.peers.as_ref(),
                    self.client.as_ref(),
                    &member.principal_id,
                    &member.relay_urls,
                    &|server_id| self.admits_outbound(server_id),
                )
                .await;
                let user_id = match (fetched, existing) {
                    (Ok((document, served_by)), None) => {
                        self.store_identity(&document, IdentitySource::Fetched(served_by))
                            .await?
                            .ok_or(ServiceError::NotFound)?
                            .user_id
                    }
                    (Ok((document, served_by)), Some((user_id, _))) => {
                        let verified =
                            verify_identity(&document).map_err(ServiceError::Validation)?;
                        if self
                            .pinned_relay_speaks_for(user_id, &served_by, &verified)
                            .await?
                        {
                            self.store_verified(
                                &document,
                                &verified,
                                IdentitySource::Authorized(served_by),
                            )
                            .await?;
                        } else {
                            warn!(user_id = %user_id, peer_server_id = %served_by, "using a stored identity; the named relay does not speak for them");
                            self.ensure_outbound_to(user_id).await?;
                        }
                        user_id
                    }
                    (Err(e @ ServiceError::Forbidden(_)), _) => return Err(e),
                    // Their relays are down: the stored copy still names them.
                    (Err(e), Some((user_id, _))) => {
                        warn!(error = %e, user_id = %user_id, "using a stored identity; the relays did not answer");
                        self.ensure_outbound_to(user_id).await?;
                        user_id
                    }
                    (Err(e), None) => return Err(e),
                };
                (user_id, true)
            }
        };
        self.join_direct_space(user_id).await?;
        Ok(ResolvedMember { user_id, remote })
    }

    async fn set_predecessor(
        &self,
        conversation_id: ConversationId,
        host_server_id: String,
        predecessor_id: Uuid,
    ) -> ServiceResult<()> {
        if !rim_s2s::is_server_id(&host_server_id) {
            return Err(ServiceError::Validation(
                "hostServerId is not a server id".to_string(),
            ));
        }
        self.repos
            .host_repo
            .set_conversation_predecessor(conversation_id, &host_server_id, predecessor_id)
            .await?;
        Ok(())
    }

    async fn predecessor(
        &self,
        conversation_id: ConversationId,
    ) -> ServiceResult<Option<ConversationPredecessorDto>> {
        Ok(self
            .repos
            .host_repo
            .conversation_predecessor(conversation_id)
            .await?
            .map(
                |(host_server_id, conversation_id)| ConversationPredecessorDto {
                    host_server_id,
                    conversation_id,
                },
            ))
    }

    #[instrument(name = "host.apply_identity", skip_all, fields(peer_server_id = %peer_server_id))]
    async fn apply_identity(
        &self,
        peer_server_id: String,
        peer_origin: String,
        document: IdentityDocument,
    ) -> ServiceResult<IdentityPush> {
        let Some((user_id, UserKind::Remote)) = self
            .repos
            .host_repo
            .find_user(&document.principal_id)
            .await?
        else {
            return Ok(IdentityPush::Applied(false));
        };
        let verified = verify_identity(&document).map_err(ServiceError::Validation)?;
        if !self
            .relay_speaks_for(user_id, &peer_server_id, &peer_origin, &verified)
            .await?
        {
            warn!(user_id = %user_id, "refused an identity from a relay that does not speak for the user");
            return Ok(IdentityPush::NotDelegated);
        }
        let stored = self
            .store_verified(
                &document,
                &verified,
                IdentitySource::Authorized(peer_server_id),
            )
            .await?;
        Ok(IdentityPush::Applied(stored.is_some()))
    }

    async fn refresh_remote(&self, principal_ids: Vec<String>) -> ServiceResult<()> {
        let now = Utc::now();
        for principal_id in principal_ids {
            let Some((user_id, UserKind::Remote)) =
                self.repos.host_repo.find_user(&principal_id).await?
            else {
                continue;
            };
            let cached = self.repos.relay_repo.cached_identity(&principal_id).await?;
            if cached
                .as_ref()
                .is_some_and(|c| now - c.fetched_at < REMOTE_IDENTITY_MAX_AGE)
            {
                continue;
            }
            for relay in self.chosen_relays(user_id).await? {
                let Some(document) = self.fetch_document(&relay, &principal_id).await else {
                    warn!(user_id = %user_id, peer_server_id = %relay, "could not refresh a remote identity");
                    continue;
                };
                match self
                    .store_identity(&document, IdentitySource::Fetched(relay.clone()))
                    .await
                {
                    Ok(_) => break,
                    Err(e) => {
                        warn!(error = %e, user_id = %user_id, peer_server_id = %relay, "a refreshed identity was refused")
                    }
                }
            }
        }
        Ok(())
    }

    async fn refresh_identity(
        &self,
        principal_id: String,
        from_relay: Option<String>,
    ) -> ServiceResult<IdentityRefresh> {
        let Some((user_id, UserKind::Remote)) =
            self.repos.host_repo.find_user(&principal_id).await?
        else {
            return Ok(IdentityRefresh::Applied(false));
        };
        let presented = from_relay.is_some();
        let relays = match from_relay {
            Some(relay) => vec![relay],
            None => self.chosen_relays(user_id).await?,
        };
        let mut refused = false;
        for relay in relays {
            let Some(document) = self.fetch_document(&relay, &principal_id).await else {
                warn!(user_id = %user_id, peer_server_id = %relay, "could not refresh a remote identity");
                continue;
            };
            let verified = match verify_identity(&document) {
                Ok(verified) => verified,
                Err(e) => {
                    warn!(error = %e, user_id = %user_id, peer_server_id = %relay, "a refreshed identity was refused");
                    refused = true;
                    continue;
                }
            };
            let source = if !presented {
                IdentitySource::Fetched(relay.clone())
            } else if self
                .pinned_relay_speaks_for(user_id, &relay, &verified)
                .await?
            {
                IdentitySource::Authorized(relay.clone())
            } else {
                warn!(user_id = %user_id, peer_server_id = %relay, "refused an identity from a relay that does not speak for the user");
                refused = true;
                continue;
            };
            match self.store_verified(&document, &verified, source).await {
                Ok(stored) => return Ok(IdentityRefresh::Applied(stored.is_some())),
                Err(e @ ServiceError::Validation(_)) => {
                    warn!(error = %e, user_id = %user_id, peer_server_id = %relay, "a refreshed identity was refused");
                    refused = true;
                }
                Err(e) => {
                    warn!(error = %e, user_id = %user_id, peer_server_id = %relay, "could not store a refreshed identity")
                }
            }
        }
        Ok(if refused {
            IdentityRefresh::Refused
        } else {
            IdentityRefresh::Unreachable
        })
    }

    async fn authenticate_forward(&self, request: ForwardRequest) -> ForwardOutcome {
        let payload = request
            .device_request
            .as_deref()
            .and_then(|h| URL_SAFE_NO_PAD.decode(h.trim()).ok())
            .and_then(|bytes| String::from_utf8(bytes).ok());
        let result = match &payload {
            None => Err(ForwardRefusal::BadRequest(
                "Rim-Device-Request is missing or not base64url".to_string(),
            )),
            Some(payload) => self.check_forward(&request, payload.clone()).await,
        };
        ForwardOutcome {
            request_payload: payload,
            result,
        }
    }

    async fn stream_heads(
        &self,
        actor: Actor,
        server_id: ServerId,
        streams: Vec<String>,
    ) -> ServiceResult<Vec<StreamHeadDto>> {
        if streams.len() > MAX_STREAMS {
            return Err(ServiceError::Validation(format!(
                "ask for at most {MAX_STREAMS} streams"
            )));
        }
        let member = self
            .repos
            .server_member_repo
            .get_member(server_id, actor.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let guest = member.member_role.is_guest();
        let principal = self
            .repos
            .user_repo
            .get_by_id(actor.user_id)
            .await?
            .and_then(|u| u.principal_id);
        let mut visible = Vec::new();
        for stream in &streams {
            let allowed = if stream == "w" {
                !guest
            } else if let Some(p) = stream.strip_prefix("u:") {
                principal.as_deref() == Some(p)
            } else if let Some(c) = stream.strip_prefix("c:") {
                match Uuid::parse_str(c) {
                    Ok(id) => match self.repos.conversation_repo.get_by_id(id.into()).await? {
                        Some(conversation) if conversation.server_id == server_id => {
                            crate::util::ensure_conversation_access_as(
                                self.repos.conversation_member_repo.as_ref(),
                                &conversation,
                                actor.user_id,
                                member.member_role,
                            )
                            .await
                            .is_ok()
                        }
                        _ => false,
                    },
                    Err(_) => false,
                }
            } else {
                false
            };
            if allowed && !visible.contains(stream) {
                visible.push(stream.clone());
            }
        }
        let heads: HashMap<String, i64> = self
            .repos
            .host_repo
            .stream_heads(server_id, &visible)
            .await?
            .into_iter()
            .collect();
        Ok(visible
            .into_iter()
            .map(|stream| StreamHeadDto {
                seq: heads.get(&stream).copied().unwrap_or(0),
                stream,
            })
            .collect())
    }

    async fn outbox_page(
        &self,
        peer_server_id: String,
        after: i64,
        limit: i64,
    ) -> ServiceResult<OutboxPage> {
        let limit = limit.clamp(1, MAX_DELIVER_ITEMS as i64);
        let mut rows = self
            .repos
            .host_repo
            .outbox_after(&peer_server_id, after, limit + 1, MAX_DELIVER_BYTES)
            .await?;
        let has_more = rows.len() as i64 > limit;
        rows.truncate(limit as usize);
        let items = rows
            .iter()
            .map(|row| outbox::deliver_item(row.id, &row.item))
            .collect::<Result<Vec<DeliverItem>, _>>()
            .map_err(ServiceError::internal)?;
        Ok(OutboxPage { items, has_more })
    }

    async fn cleanup(&self) -> ServiceResult<HostCleanup> {
        let now = Utc::now();
        Ok(HostCleanup {
            outbox_rows: self
                .repos
                .host_repo
                .delete_outbox_before(now - OUTBOX_RETENTION)
                .await?,
            seen_requests: self
                .repos
                .host_repo
                .purge_request_seen(now, REQUEST_SEQ_WINDOW)
                .await?,
        })
    }

    async fn ensure_may_request(
        &self,
        actor: Actor,
        server_id: ServerId,
        user_ids: Vec<UserId>,
        direct_message: bool,
    ) -> ServiceResult<()> {
        let in_direct_space = server_id == self.direct_space_id;
        if !in_direct_space && !direct_message {
            return Ok(());
        }
        let others: Vec<UserId> = user_ids
            .into_iter()
            .filter(|id| *id != actor.user_id)
            .collect();
        if in_direct_space {
            for other in &others {
                if self.repos.host_repo.user_kind(*other).await? == Some(UserKind::Remote) {
                    self.ensure_outbound_to(*other).await?;
                }
            }
        }
        let requested = self.requested_of(actor, &others).await?;
        let reopened = match others.as_slice() {
            [other] if direct_message => self
                .repos
                .conversation_repo
                .find_direct_between_users(server_id, actor.user_id, *other)
                .await?
                .is_some(),
            _ => false,
        };
        if !reopened {
            self.refuse_requests_over_limits(actor, &requested).await?;
        }
        if !in_direct_space || self.limits.probation_hours.is_none() {
            return Ok(());
        }
        let principal_ids: Vec<String> = self
            .repos
            .user_repo
            .get_by_ids(&others)
            .await?
            .into_iter()
            .filter_map(|user| user.principal_id)
            .collect();
        self.refuse_strangers_in_probation(actor, &principal_ids)
            .await
    }
}

#[async_trait]
impl RemoteKeyPackages for DefaultHostService {
    async fn claim(&self, claim: RemoteClaim) -> ServiceResult<Option<Vec<RemoteKeyPackage>>> {
        if self.repos.host_repo.user_kind(claim.target_user_id).await? != Some(UserKind::Remote) {
            return Ok(None);
        }
        let user = self
            .repos
            .user_repo
            .get_by_id(claim.target_user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        let principal_id = user.principal_id.ok_or(ServiceError::NotFound)?;
        let card = self
            .repos
            .key_identity_repo
            .get_profile_card(claim.target_user_id)
            .await?;
        let home = self
            .repos
            .host_repo
            .home_relay(claim.target_user_id)
            .await?;
        let mut relays = Self::relays_of(
            self.repos.host_repo.as_ref(),
            &self.own_id(),
            card.as_ref().map(|c| c.payload.as_str()),
            home.as_deref(),
        )
        .await?;
        if !relays.is_empty() {
            relays.retain(|relay| self.admits_outbound(relay));
            if relays.is_empty() {
                return Err(outbound_refused());
            }
        }
        let response = self
            .claim_from_relays(
                &relays,
                &PeerClaimRequest {
                    principal_id,
                    devices: claim.device_ids.clone(),
                    conversation: ClaimConversation {
                        host: self.own_id(),
                        conversation_id: claim.conversation_id.into(),
                        incarnation: claim.incarnation,
                    },
                    claimer: Claimer {
                        principal_id: claim.claimer_principal_id.clone(),
                        device_id: claim.claimer_device_id,
                    },
                    cipher_suite: Some(claim.cipher_suite),
                },
            )
            .await?;
        let asked: HashSet<Uuid> = claim.device_ids.iter().copied().collect();
        let mut packages = Vec::with_capacity(response.key_packages.len());
        for package in response.key_packages {
            if !asked.contains(&package.device_id) {
                continue;
            }
            let Ok(bytes) = URL_SAFE_NO_PAD.decode(&package.key_package) else {
                continue;
            };
            packages.push(RemoteKeyPackage {
                device_id: package.device_id,
                key_package: bytes,
                last_resort: package.last_resort,
            });
        }
        info!(
            claimed = packages.len(),
            user_id = %claim.target_user_id,
            "claimed a remote user's key packages from their relay"
        );
        Ok(Some(packages))
    }
}
