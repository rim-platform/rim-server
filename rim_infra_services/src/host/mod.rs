//! This server as the host of spaces with members from other servers: the direct space, remote
//! users and their identities, forwarded device requests, stream heads, the outbox and the
//! key packages claimed from remote users' relays.

pub mod outbox;

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock};

use async_trait::async_trait;
use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};
use uuid::Uuid;

use rim_core::conversation::model::ConversationId;
use rim_core::conversation::repository::ConversationRepository;
use rim_core::conversation_member::repository::ConversationMemberRepository;
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
use rim_services::e2ee::service::E2eeService;
use rim_services::host::dto::{
    ConversationPredecessorDto, ForwardAuth, ForwardOutcome, ForwardRefusal, ForwardRequest,
    HostCleanup, RemoteClaim, RemoteKeyPackage, RemoteMemberRef, ResolvedMember, StreamHeadDto,
};
use rim_services::host::service::{DelegationCheck, HostService, RemoteKeyPackages};
use rim_services::relay::dto::{
    ClaimConversation, Claimer, DeliverItem, IdentityDocument, OutboxPage, PeerClaimRequest,
    PeerClaimResponse,
};
use rim_services::s2s::dto::{S2sMethod, S2sRequest};
use rim_services::s2s::service::{PeerService, S2sClient};
use rim_services::{Actor, ServiceError, ServiceResult};

use crate::quota::{DailyCounter, Quotas};
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::relay::identity::{IdentityStore, Stored, store_remote, verify_identity};
use crate::relay::{
    MAX_DELIVER_BYTES, MAX_DELIVER_ITEMS, card_relays, check_relay_urls, fetch_from_relays,
    origin_of,
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
    /// Principals this server refuses.
    pub relay_abuse_repo: Arc<dyn RelayAbuseRepository>,
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
    /// Delegations of workspace spaces; without one only the direct-space rule applies.
    delegations: Option<Arc<dyn DelegationCheck>>,
    /// Invites each user sends to people on other servers who are not their contacts, per
    /// day.
    dm_invites: DailyCounter,
}

fn upstream(server_id: &str) -> ServiceError {
    ServiceError::upstream(server_id)
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
            delegations: None,
            dm_invites: DailyCounter::per_day(Quotas::default().dm_requests_per_day),
        }
    }

    /// Apply the per-user quotas: invites to people on other servers per day.
    pub fn with_quotas(mut self, quotas: Quotas) -> Self {
        self.dm_invites = DailyCounter::per_day(quotas.dm_requests_per_day);
        self
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
    pub(crate) async fn store_identity(
        &self,
        document: &IdentityDocument,
        served_by: &str,
    ) -> ServiceResult<Option<Stored>> {
        let verified = verify_identity(document).map_err(ServiceError::Validation)?;
        let home_relay = self.pin_card_relays(document).await;
        let (scope, repos) = self.begin().await?;
        let stored = store_remote(
            &IdentityStore {
                host_repo: repos.host_repo.as_ref(),
                key_identity_repo: repos.key_identity_repo.as_ref(),
            },
            &verified,
            home_relay.as_deref().or(Some(served_by)),
        )
        .await?;
        Self::commit(scope).await?;
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
        Ok(card_relays(&card.payload)
            .iter()
            .filter_map(|url| origin_of(url))
            .any(|origin| origin == peer_origin))
    }

    /// Count an invite `actor` sends to someone on another server against their daily quota,
    /// unless that person is their contact.
    async fn count_dm_invite(&self, actor: Actor, principal_id: &str) -> ServiceResult<()> {
        if self
            .repos
            .relay_repo
            .is_contact(actor.user_id, principal_id)
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
                self.count_dm_invite(actor, &member.principal_id).await?;
                let fetched = fetch_from_relays(
                    self.peers.as_ref(),
                    self.client.as_ref(),
                    &member.principal_id,
                    &member.relay_urls,
                )
                .await;
                let user_id = match (fetched, existing) {
                    (Ok((document, served_by)), _) => {
                        self.store_identity(&document, &served_by)
                            .await?
                            .ok_or(ServiceError::NotFound)?
                            .user_id
                    }
                    // Their relays are down: the stored copy still names them.
                    (Err(e), Some((user_id, _))) => {
                        warn!(error = %e, user_id = %user_id, "using a stored identity; the relays did not answer");
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
        document: IdentityDocument,
    ) -> ServiceResult<bool> {
        match self
            .repos
            .host_repo
            .find_user(&document.principal_id)
            .await?
        {
            Some((_, UserKind::Remote)) => {}
            _ => return Ok(false),
        }
        Ok(self
            .store_identity(&document, &peer_server_id)
            .await?
            .is_some())
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
            let relay = match cached.map(|c| c.served_by) {
                Some(relay) => Some(relay),
                None => self.repos.host_repo.home_relay(user_id).await?,
            };
            let Some(relay) = relay else {
                continue;
            };
            match self.fetch_document(&relay, &principal_id).await {
                Some(document) => {
                    if let Err(e) = self.store_identity(&document, &relay).await {
                        warn!(error = %e, user_id = %user_id, "a refreshed identity was refused");
                    }
                }
                _ => {
                    warn!(user_id = %user_id, peer_server_id = %relay, "could not refresh a remote identity")
                }
            }
        }
        Ok(())
    }

    async fn refresh_identity(
        &self,
        principal_id: String,
        from_relay: Option<String>,
    ) -> ServiceResult<bool> {
        let Some((user_id, UserKind::Remote)) =
            self.repos.host_repo.find_user(&principal_id).await?
        else {
            return Ok(false);
        };
        let relays = match from_relay {
            Some(relay) => vec![relay],
            None => {
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
                relays
            }
        };
        for relay in relays {
            let Some(document) = self.fetch_document(&relay, &principal_id).await else {
                warn!(user_id = %user_id, peer_server_id = %relay, "could not refresh a remote identity");
                continue;
            };
            match self.store_identity(&document, &relay).await {
                Ok(stored) => return Ok(stored.is_some()),
                Err(e) => {
                    warn!(error = %e, user_id = %user_id, peer_server_id = %relay, "a refreshed identity was refused")
                }
            }
        }
        Ok(false)
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
        let relays = Self::relays_of(
            self.repos.host_repo.as_ref(),
            &self.own_id(),
            card.as_ref().map(|c| c.payload.as_str()),
            home.as_deref(),
        )
        .await?;
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
