//! Pinning peers from their discovery documents and keeping the pins current.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::repository::error::RepoErrorKind;
use rim_core::s2s::model::{
    MAX_PEERS_PER_ORIGIN, PeerPin, PeerRotationSigner, PeerServer, PeerStatus, SignedRecord,
};
use rim_core::s2s::repository::PeerServerRepository;
use rim_identity::SignedStatement;
use rim_s2s::{
    LogRelation, ServerDescriptor, ServerKeyState, ServerRotationSigner, compare_logs,
    verify_descriptor,
};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::relay::dto::SignedJson;
use rim_services::s2s::dto::{
    DiscoveryRequester, PeerBlockChange, PeerDto, PeerKeysDto, PeerLookup,
};
use rim_services::s2s::service::PeerService;
use rim_services::{ServiceError, ServiceResult};
use serde::Deserialize;

use crate::keyed_rate_limiter::KeyedRateLimiter;
use crate::s2s::handle_domain::{DomainCheck, check_handle_domain};
use crate::s2s::net::{NetError, check_url_host, http_client, normalize_origin, read_limited};

/// Path of the discovery document below an origin.
pub const DISCOVERY_PATH: &str = "/.well-known/rim-server";

/// Largest discovery document fetched.
pub const MAX_DISCOVERY_BYTES: usize = 64 * 1024;

/// How long a discovery fetch may take.
const DISCOVERY_TIMEOUT: Duration = Duration::from_secs(5);

/// A pin older than this is refetched when the peer is used.
const REFRESH_AFTER_HOURS: i64 = 6;

/// How long the previous key stays accepted after a `signer: server` rotation.
const ROTATION_GRACE_HOURS: i64 = 24;

/// How long a fetch that failed is answered with its failure instead of being made again.
const FAILED_FETCH_MINUTES: i64 = 5;

/// The most origins and failed fetches remembered at once.
const MAX_REMEMBERED: usize = 10_000;

/// Discovery fetches a signed-in user's requests may cause per minute.
const FETCHES_PER_USER_PER_MINUTE: u32 = 10;

/// Discovery fetches unauthenticated requests from one address may cause per minute.
const FETCHES_PER_ADDRESS_PER_MINUTE: u32 = 5;

/// Discovery fetches of one kind per minute, across every requester.
const FETCHES_PER_KIND_PER_MINUTE: u32 = 60;

/// What a discovery fetch is for. Each kind has a budget of its own, so fetches of one kind
/// never use up another's.
#[derive(Debug, Clone, Copy)]
enum FetchKind {
    /// Refetching a pinned peer.
    Refresh,
    /// Pinning a peer that a request names by its key id.
    Introduced,
    /// An explicit pin of an origin.
    Pin,
    /// Checking a pinned peer's handle domain.
    HandleDomain,
}

impl FetchKind {
    fn key(self) -> &'static str {
        match self {
            Self::Refresh => "refresh",
            Self::Introduced => "introduced",
            Self::Pin => "pin",
            Self::HandleDomain => "handle-domain",
        }
    }
}

/// Why a fetch failed, kept to answer the same way while it is remembered.
#[derive(Debug, Clone)]
enum FetchFailure {
    Unreachable,
    Invalid(String),
}

impl FetchFailure {
    fn to_error(&self) -> ServiceError {
        match self {
            Self::Unreachable => ServiceError::Unavailable,
            Self::Invalid(reason) => ServiceError::Validation(reason.clone()),
        }
    }
}

/// Recent outcomes of discovery fetches: the server id each origin pinned last, and the fetches
/// that failed with when they failed. Each holds at most [`MAX_REMEMBERED`] entries.
#[derive(Default)]
struct RecentFetches {
    pinned: Mutex<HashMap<String, String>>,
    failed: Mutex<HashMap<String, (DateTime<Utc>, FetchFailure)>>,
}

impl RecentFetches {
    /// The server id `origin` pinned last.
    fn pinned_at(&self, origin: &str) -> Option<String> {
        self.pinned.lock().ok()?.get(origin).cloned()
    }

    /// Remember that `origin` pinned `server_id`, and forget a failure of its fetches.
    fn record_pinned(&self, origin: &str, server_id: &str) {
        if let Ok(mut pinned) = self.pinned.lock() {
            if pinned.len() >= MAX_REMEMBERED && !pinned.contains_key(origin) {
                pinned.clear();
            }
            pinned.insert(origin.to_string(), server_id.to_string());
        }
    }

    /// The failure of the fetch `key` when it failed within [`FAILED_FETCH_MINUTES`].
    fn failed(&self, key: &str, now: DateTime<Utc>) -> Option<FetchFailure> {
        let failed = self.failed.lock().ok()?;
        let (at, failure) = failed.get(key)?;
        (now - *at < chrono::Duration::minutes(FAILED_FETCH_MINUTES)).then(|| failure.clone())
    }

    /// Remember that the fetch `key` failed at `now`. When full of failures that have not
    /// expired, the failure is not remembered.
    fn record_failed(&self, key: &str, failure: FetchFailure, now: DateTime<Utc>) {
        let Ok(mut failed) = self.failed.lock() else {
            return;
        };
        if failed.len() >= MAX_REMEMBERED {
            failed.retain(|_, (at, _)| now - *at < chrono::Duration::minutes(FAILED_FETCH_MINUTES));
        }
        if failed.len() < MAX_REMEMBERED {
            failed.insert(key.to_string(), (now, failure));
        }
    }
}

/// Fetches a server's discovery document.
#[async_trait]
pub trait DiscoveryFetcher: Send + Sync {
    /// The document at `origin` (already normalized), at most 64 KiB.
    async fn fetch(&self, origin: &str) -> Result<Vec<u8>, NetError>;
}

/// Fetches discovery documents over HTTP: 5 seconds, 64 KiB, no redirects, and no private
/// addresses unless allowed.
pub struct HttpDiscoveryFetcher {
    client: reqwest::Client,
    allow_private: bool,
}

impl HttpDiscoveryFetcher {
    pub fn new(allow_private: bool) -> Result<Self, NetError> {
        Ok(Self {
            client: http_client(allow_private, DISCOVERY_TIMEOUT)?,
            allow_private,
        })
    }
}

#[async_trait]
impl DiscoveryFetcher for HttpDiscoveryFetcher {
    async fn fetch(&self, origin: &str) -> Result<Vec<u8>, NetError> {
        let url = url::Url::parse(&format!("{origin}{DISCOVERY_PATH}"))
            .map_err(|e| NetError::BadOrigin(e.to_string()))?;
        check_url_host(&url, self.allow_private)?;
        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NetError::Transport(e.to_string()))?;
        if response.status() != reqwest::StatusCode::OK {
            return Err(NetError::Transport(format!(
                "discovery answered {}",
                response.status()
            )));
        }
        read_limited(response, MAX_DISCOVERY_BYTES).await
    }
}

#[derive(Deserialize)]
struct WireStatement {
    payload: String,
    signature: String,
}

impl From<WireStatement> for SignedStatement {
    fn from(w: WireStatement) -> Self {
        Self {
            payload: w.payload,
            signature: w.signature,
        }
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct DiscoveryDocument {
    rim: String,
    key_log: Vec<WireStatement>,
    descriptor: WireStatement,
}

/// A discovery document whose log replays and whose descriptor verifies against it.
pub struct VerifiedDiscovery {
    pub state: ServerKeyState,
    pub log: Vec<SignedStatement>,
    pub descriptor: SignedStatement,
    pub content: ServerDescriptor,
}

/// Verify a discovery document fetched from `origin`: the log replays, its inception hashes to
/// `expected_server_id` when given, the descriptor verifies against the head, and its
/// `endpoints.s2s` is at `origin`.
pub fn verify_discovery(
    bytes: &[u8],
    origin: &str,
    expected_server_id: Option<&str>,
) -> Result<VerifiedDiscovery, String> {
    let doc: DiscoveryDocument = serde_json::from_slice(bytes).map_err(|e| {
        tracing::info!(detail = %e, "refused a malformed discovery document");
        "not a discovery document".to_string()
    })?;
    if doc.rim != "server-discovery" {
        return Err("not a discovery document".to_string());
    }
    let log: Vec<SignedStatement> = doc.key_log.into_iter().map(Into::into).collect();
    let state = ServerKeyState::replay(&log).map_err(|e| format!("key log: {e}"))?;
    if expected_server_id.is_some_and(|id| id != state.server_id) {
        return Err("the key log is for another server".to_string());
    }
    let descriptor: SignedStatement = doc.descriptor.into();
    let content = verify_descriptor(&state, &descriptor).map_err(|e| format!("descriptor: {e}"))?;
    let at_origin = content.endpoints.s2s.as_deref().is_some_and(|s2s| {
        s2s.strip_prefix(origin)
            .is_some_and(|rest| rest.is_empty() || rest.starts_with('/'))
    });
    if !at_origin {
        return Err("the descriptor has no server-to-server endpoint at this origin".to_string());
    }
    Ok(VerifiedDiscovery {
        state,
        log,
        descriptor,
        content,
    })
}

fn to_record(s: &SignedStatement) -> SignedRecord {
    SignedRecord {
        payload: s.payload.clone(),
        signature: s.signature.clone(),
    }
}

fn pin_from(origin: &str, found: &VerifiedDiscovery, rotated_at: Option<DateTime<Utc>>) -> PeerPin {
    let (previous_key, rotation_signer) = match &found.state.previous {
        Some((key, signer)) => (
            Some(key.to_string()),
            Some(match signer {
                ServerRotationSigner::Server => PeerRotationSigner::Server,
                ServerRotationSigner::Recovery => PeerRotationSigner::Recovery,
            }),
        ),
        None => (None, None),
    };
    PeerPin {
        server_id: found.state.server_id.clone(),
        origin: origin.to_string(),
        head_seq: found.state.seq,
        head_key: found.state.server_key.to_string(),
        rotated_at: previous_key.as_ref().and(rotated_at),
        previous_key,
        rotation_signer,
        descriptor_version: found.content.version,
        descriptor: to_record(&found.descriptor),
        log: found.log.iter().map(to_record).collect(),
    }
}

/// The keys a pinned peer signs with at `now`.
pub fn peer_keys(peer: &PeerServer, now: DateTime<Utc>) -> PeerKeysDto {
    let previous_key = match (&peer.previous_key, peer.rotation_signer, peer.rotated_at) {
        (Some(key), Some(PeerRotationSigner::Server), Some(at))
            if now < at + chrono::Duration::hours(ROTATION_GRACE_HOURS) =>
        {
            Some(key.clone())
        }
        _ => None,
    };
    let s2s_endpoint = serde_json::from_str::<ServerDescriptor>(&peer.descriptor.payload)
        .ok()
        .and_then(|d| d.endpoints.s2s);
    PeerKeysDto {
        server_id: peer.server_id.clone(),
        origin: peer.origin.clone(),
        head_seq: peer.head_seq,
        head_key: peer.head_key.clone(),
        previous_key,
        s2s_endpoint,
    }
}

/// The handle domain a pinned peer's descriptor declares.
fn declared_handle_domain(peer: &PeerServer) -> Option<String> {
    serde_json::from_str::<ServerDescriptor>(&peer.descriptor.payload)
        .ok()
        .and_then(|d| d.handle_domain)
}

fn to_dto(peer: &PeerServer) -> PeerDto {
    PeerDto {
        server_id: peer.server_id.clone(),
        origin: peer.origin.clone(),
        status: peer.status.as_str().to_string(),
        head_seq: peer.head_seq,
        descriptor_version: peer.descriptor_version,
        descriptor_payload: peer.descriptor.payload.clone(),
        fetched_at: peer.fetched_at,
    }
}

fn lookup(peer: &PeerServer, now: DateTime<Utc>) -> PeerLookup {
    match peer.status {
        PeerStatus::Active => PeerLookup::Ready(peer_keys(peer, now)),
        PeerStatus::Forked => PeerLookup::Forked,
        PeerStatus::Blocked => PeerLookup::Blocked,
    }
}

enum RefreshError {
    Forked,
    Failed(String),
}

/// Pins peers through a [`DiscoveryFetcher`] and stores them in a [`PeerServerRepository`].
pub struct DefaultPeerService {
    repo: Arc<dyn PeerServerRepository>,
    fetcher: Arc<dyn DiscoveryFetcher>,
    allow_http: bool,
    per_peer: KeyedRateLimiter,
    per_user: KeyedRateLimiter,
    per_address: KeyedRateLimiter,
    per_kind: KeyedRateLimiter,
    recent: RecentFetches,
}

impl DefaultPeerService {
    /// `allow_http` accepts `http` origins (development and tests only).
    pub fn new(
        repo: Arc<dyn PeerServerRepository>,
        fetcher: Arc<dyn DiscoveryFetcher>,
        allow_http: bool,
    ) -> Self {
        Self {
            repo,
            fetcher,
            allow_http,
            per_peer: KeyedRateLimiter::new(60, 1),
            per_user: KeyedRateLimiter::new(60, FETCHES_PER_USER_PER_MINUTE),
            per_address: KeyedRateLimiter::new(60, FETCHES_PER_ADDRESS_PER_MINUTE),
            per_kind: KeyedRateLimiter::new(60, FETCHES_PER_KIND_PER_MINUTE),
            recent: RecentFetches::default(),
        }
    }

    /// Whether `requester`'s share of the budget allows another fetch now; this server's own
    /// work has no share and always may.
    fn requester_may_fetch(&self, requester: DiscoveryRequester, now: DateTime<Utc>) -> bool {
        let decision = match requester {
            DiscoveryRequester::Server => return true,
            DiscoveryRequester::User(user_id) => {
                self.per_user.check_and_record(&user_id.to_string(), now)
            }
            DiscoveryRequester::Address(address) => self.per_address.check_and_record(
                &address.map_or_else(|| "unknown".to_string(), |ip| ip.to_string()),
                now,
            ),
        };
        matches!(decision, RateLimitDecision::Allowed)
    }

    /// Whether a fetch for `requester`, of the peer or origin `key` and of `kind`, fits:
    /// `requester`'s share first, then once a minute per `key` (none for an explicit pin, whose
    /// repeats the remembered outcomes answer), then the budget of `kind`. A refusal is
    /// counted in the `peer_discovery` metric.
    fn may_fetch(&self, key: &str, kind: FetchKind, requester: DiscoveryRequester) -> bool {
        let now = Utc::now();
        let allowed = self.requester_may_fetch(requester, now)
            && (matches!(kind, FetchKind::Pin)
                || matches!(
                    self.per_peer.check_and_record(key, now),
                    RateLimitDecision::Allowed
                ))
            && matches!(
                self.per_kind.check_and_record(kind.key(), now),
                RateLimitDecision::Allowed
            );
        if !allowed {
            crate::metrics::rate_limited("peer_discovery");
        }
        allowed
    }

    /// The pin of `origin` when it is `active`, fetched within [`REFRESH_AFTER_HOURS`] and of
    /// `expected_server_id` when one is given: the pin of that id, else the one the origin
    /// pinned last here.
    async fn fresh_pin_at(
        &self,
        origin: &str,
        expected_server_id: Option<&str>,
    ) -> ServiceResult<Option<PeerServer>> {
        let Some(server_id) = expected_server_id
            .map(str::to_string)
            .or_else(|| self.recent.pinned_at(origin))
        else {
            return Ok(None);
        };
        let Some(peer) = self.repo.find(&server_id).await? else {
            return Ok(None);
        };
        let fresh = peer.status == PeerStatus::Active
            && peer.origin == origin
            && Utc::now() - peer.fetched_at < chrono::Duration::hours(REFRESH_AFTER_HOURS);
        Ok(fresh.then_some(peer))
    }

    async fn fetch_verified(
        &self,
        origin: &str,
        expected: Option<&str>,
    ) -> Result<VerifiedDiscovery, String> {
        let bytes = self
            .fetcher
            .fetch(origin)
            .await
            .map_err(|e| e.to_string())?;
        verify_discovery(&bytes, origin, expected)
    }

    /// Pin a peer not pinned now. A concurrent pin of the same peer wins and is returned. A peer
    /// whose idle pin was forgotten is pinned `forked` when the served log does not hold the
    /// forgotten head.
    ///
    /// # Errors
    /// `Conflict` when the served log stops short of the forgotten head (it may be an older
    /// view, or a log that never reached it), or when the origin has as many pins as allowed.
    async fn insert_pin(
        &self,
        origin: &str,
        found: &VerifiedDiscovery,
    ) -> ServiceResult<PeerServer> {
        let server_id = &found.state.server_id;
        let status = match self.repo.forgotten(server_id).await? {
            None => PeerStatus::Active,
            Some(head) => match usize::try_from(head.head_seq)
                .ok()
                .and_then(|seq| found.log.get(seq))
            {
                None => {
                    return Err(ServiceError::Conflict(
                        "the served key log stops short of the head pinned before".to_string(),
                    ));
                }
                Some(entry) if entry.payload == head.head_payload => PeerStatus::Active,
                Some(_) => PeerStatus::Forked,
            },
        };
        let pin = pin_from(origin, found, None);
        match self.repo.insert(pin, status).await {
            Ok(peer) if peer.status == PeerStatus::Forked => {
                tracing::error!(
                    target: "audit",
                    event = "s2s.peer_forked",
                    peer_server_id = %peer.server_id,
                    pinned_seq = peer.head_seq,
                    "a peer server's key log does not extend the head pinned before its pin \
                     was forgotten; refusing it until `rim_server peer reset` after an \
                     out-of-band check"
                );
                Ok(peer)
            }
            Ok(peer) => {
                tracing::info!(
                    target: "audit",
                    event = "s2s.peer_pinned",
                    peer_server_id = %peer.server_id,
                    head_seq = peer.head_seq,
                    "pinned a peer server"
                );
                Ok(self.record_handle_domain(peer).await?)
            }
            Err(e) if e.kind() == RepoErrorKind::Conflict => self
                .repo
                .find(server_id)
                .await?
                .ok_or(ServiceError::Conflict("peer pin raced".to_string())),
            Err(e) if e.kind() == RepoErrorKind::PreconditionFailed => {
                Err(ServiceError::Conflict(format!(
                    "{MAX_PEERS_PER_ORIGIN} servers are pinned at this origin already; \
                     `rim_server peer reset` one of them"
                )))
            }
            Err(e) => Err(e.into()),
        }
    }

    /// Refetch a pinned peer from its pinned origin and apply what it serves.
    async fn refresh_pinned(&self, peer: &PeerServer) -> Result<PeerServer, RefreshError> {
        let found = self
            .fetch_verified(&peer.origin, Some(&peer.server_id))
            .await
            .map_err(RefreshError::Failed)?;
        let pinned: Vec<SignedStatement> = self
            .repo
            .key_log(&peer.server_id)
            .await
            .map_err(|e| RefreshError::Failed(e.to_string()))?
            .into_iter()
            .map(|r| SignedStatement {
                payload: r.payload,
                signature: r.signature,
            })
            .collect();
        let failed =
            |e: rim_core::repository::error::RepoError| RefreshError::Failed(e.to_string());
        match compare_logs(&pinned, &found.log) {
            LogRelation::Equal | LogRelation::Behind => {
                let newer = (found.content.version > peer.descriptor_version
                    && found.state.seq == peer.head_seq)
                    .then(|| (found.content.version, to_record(&found.descriptor)));
                self.repo
                    .mark_fetched(&peer.server_id, newer)
                    .await
                    .map_err(failed)?;
            }
            LogRelation::Extends => {
                let pin = pin_from(&peer.origin, &found, Some(Utc::now()));
                if self
                    .repo
                    .advance(pin, peer.head_seq)
                    .await
                    .map_err(failed)?
                    .is_some()
                {
                    tracing::info!(
                        target: "audit",
                        event = "s2s.peer_rotated",
                        peer_server_id = %peer.server_id,
                        from_seq = peer.head_seq,
                        to_seq = found.state.seq,
                        signer = found.state.head_signer().map(ServerRotationSigner::as_str),
                        "a peer server rotated its key"
                    );
                }
            }
            LogRelation::Forked => {
                self.repo
                    .set_status(&peer.server_id, PeerStatus::Forked)
                    .await
                    .map_err(failed)?;
                tracing::error!(
                    target: "audit",
                    event = "s2s.peer_forked",
                    peer_server_id = %peer.server_id,
                    pinned_seq = peer.head_seq,
                    "a peer server's key log no longer extends its pin; refusing it until \
                     `rim_server peer reset` after an out-of-band check"
                );
                return Err(RefreshError::Forked);
            }
        }
        let fresh = self
            .repo
            .find(&peer.server_id)
            .await
            .map_err(failed)?
            .ok_or_else(|| RefreshError::Failed("peer was reset".to_string()))?;
        self.record_handle_domain(fresh).await.map_err(failed)
    }

    /// Check the handle domain the peer's descriptor declares and record the outcome: the
    /// domain when it points back at the peer, `None` when the descriptor declares none or the
    /// domain points elsewhere. A check that cannot run now (the domain does not answer, or the
    /// fetch limits are reached) keeps what was recorded. Returns the peer as recorded.
    ///
    /// # Errors
    /// What the repository returns.
    async fn record_handle_domain(
        &self,
        mut peer: PeerServer,
    ) -> Result<PeerServer, rim_core::repository::error::RepoError> {
        let checked = match declared_handle_domain(&peer) {
            None => None,
            Some(domain) => {
                let check = if self.may_fetch(
                    &format!("handle-domain:{domain}"),
                    FetchKind::HandleDomain,
                    DiscoveryRequester::Server,
                ) {
                    check_handle_domain(
                        self.fetcher.as_ref(),
                        &peer.origin,
                        &peer.server_id,
                        &domain,
                        self.allow_http,
                    )
                    .await
                } else {
                    DomainCheck::Unavailable("over the fetch limits".to_string())
                };
                match check {
                    DomainCheck::Verified => Some(domain),
                    DomainCheck::Refused(reason) => {
                        tracing::warn!(
                            peer_server_id = %peer.server_id,
                            handle_domain = %domain,
                            reason = %reason,
                            "a peer's handle domain does not point back at it; its people get no handles"
                        );
                        None
                    }
                    DomainCheck::Unavailable(reason) => {
                        tracing::warn!(
                            peer_server_id = %peer.server_id,
                            handle_domain = %domain,
                            reason = %reason,
                            "could not check a peer's handle domain; keeping what was recorded"
                        );
                        return Ok(peer);
                    }
                }
            }
        };
        if checked != peer.handle_domain {
            self.repo
                .set_handle_domain(&peer.server_id, checked.clone())
                .await?;
            tracing::info!(
                target: "audit",
                event = "s2s.peer_handle_domain",
                peer_server_id = %peer.server_id,
                handle_domain = checked.as_deref().unwrap_or(""),
                "recorded a peer server's handle domain"
            );
            peer.handle_domain = checked;
        }
        Ok(peer)
    }

    /// Refresh within the limits, charged to `requester`; a failure keeps the pin as it was.
    async fn refresh_or_keep(
        &self,
        peer: PeerServer,
        requester: DiscoveryRequester,
    ) -> ServiceResult<PeerLookup> {
        if !self.may_fetch(&peer.server_id, FetchKind::Refresh, requester) {
            return Ok(lookup(&peer, Utc::now()));
        }
        match self.refresh_pinned(&peer).await {
            Ok(fresh) => Ok(lookup(&fresh, Utc::now())),
            Err(RefreshError::Forked) => Ok(PeerLookup::Forked),
            Err(RefreshError::Failed(reason)) => {
                tracing::warn!(
                    peer_server_id = %peer.server_id,
                    reason = %reason,
                    "peer refresh failed; keeping its pin"
                );
                Ok(lookup(&peer, Utc::now()))
            }
        }
    }
}

#[async_trait]
impl PeerService for DefaultPeerService {
    async fn resolve(
        &self,
        server_id: &str,
        origin: Option<&str>,
        key_seq: Option<u32>,
        requester: DiscoveryRequester,
    ) -> ServiceResult<PeerLookup> {
        let Some(peer) = self.repo.find(server_id).await? else {
            let Some(origin) = origin.and_then(|o| normalize_origin(o, self.allow_http).ok())
            else {
                return Ok(PeerLookup::Unknown);
            };
            // Keyed by id and origin together, so naming an id with another origin uses up
            // neither the minute nor the remembered outcome of the id at its own origin.
            let key = format!("{server_id} {origin}");
            if self.recent.failed(&key, Utc::now()).is_some()
                || !self.may_fetch(&key, FetchKind::Introduced, requester)
            {
                return Ok(PeerLookup::Unknown);
            }
            return match self.fetch_verified(&origin, Some(server_id)).await {
                Ok(found) => match self.insert_pin(&origin, &found).await {
                    Ok(peer) => {
                        // The new pin counts as the peer's fetch of this minute.
                        self.per_peer.check_and_record(server_id, Utc::now());
                        self.recent.record_pinned(&origin, &peer.server_id);
                        Ok(lookup(&peer, Utc::now()))
                    }
                    Err(ServiceError::Conflict(reason)) => {
                        tracing::warn!(
                            peer_server_id = %server_id,
                            reason = %reason,
                            "could not pin an unknown peer"
                        );
                        Ok(PeerLookup::Unknown)
                    }
                    Err(e) => Err(e),
                },
                Err(reason) => {
                    tracing::warn!(
                        peer_server_id = %server_id,
                        reason = %reason,
                        "could not pin an unknown peer"
                    );
                    self.recent
                        .record_failed(&key, FetchFailure::Invalid(reason), Utc::now());
                    Ok(PeerLookup::Unknown)
                }
            };
        };
        if peer.status != PeerStatus::Active {
            return Ok(lookup(&peer, Utc::now()));
        }
        let stale = Utc::now() - peer.fetched_at > chrono::Duration::hours(REFRESH_AFTER_HOURS);
        let ahead = key_seq.is_some_and(|seq| seq > peer.head_seq);
        if stale || ahead {
            return self.refresh_or_keep(peer, requester).await;
        }
        Ok(lookup(&peer, Utc::now()))
    }

    async fn refresh(&self, server_id: &str) -> ServiceResult<PeerLookup> {
        match self.repo.find(server_id).await? {
            Some(peer) if peer.status == PeerStatus::Active => {
                self.refresh_or_keep(peer, DiscoveryRequester::Server).await
            }
            Some(peer) => Ok(lookup(&peer, Utc::now())),
            None => Ok(PeerLookup::Unknown),
        }
    }

    async fn pin(
        &self,
        origin: &str,
        expected_server_id: Option<&str>,
        requester: DiscoveryRequester,
    ) -> ServiceResult<PeerDto> {
        let origin = normalize_origin(origin, self.allow_http)
            .map_err(|e| ServiceError::Validation(e.to_string()))?;
        if let Some(peer) = self.fresh_pin_at(&origin, expected_server_id).await? {
            return Ok(to_dto(&peer));
        }
        let key = format!("{origin} {}", expected_server_id.unwrap_or_default());
        if let Some(failure) = self.recent.failed(&key, Utc::now()) {
            return Err(failure.to_error());
        }
        if !self.may_fetch(&key, FetchKind::Pin, requester) {
            return Err(ServiceError::RateLimited {
                retry_after_seconds: 60,
            });
        }
        let failed = |failure: FetchFailure| {
            let error = failure.to_error();
            self.recent.record_failed(&key, failure, Utc::now());
            error
        };
        let bytes = self.fetcher.fetch(&origin).await.map_err(|e| {
            tracing::warn!(error = %e, "discovery fetch failed");
            failed(FetchFailure::Unreachable)
        })?;
        let found = verify_discovery(&bytes, &origin, expected_server_id)
            .map_err(|reason| failed(FetchFailure::Invalid(reason)))?;
        let peer = match self.repo.find(&found.state.server_id).await? {
            None => {
                let peer = self.insert_pin(&origin, &found).await?;
                if peer.status == PeerStatus::Forked {
                    return Err(ServiceError::Forbidden("peer_forked".to_string()));
                }
                peer
            }
            Some(peer) => match self.refresh_pinned(&peer).await {
                Ok(fresh) => fresh,
                Err(RefreshError::Forked) => {
                    return Err(ServiceError::Forbidden("peer_forked".to_string()));
                }
                Err(RefreshError::Failed(reason)) => {
                    tracing::warn!(reason = %reason, "peer refresh failed");
                    return Err(ServiceError::Unavailable);
                }
            },
        };
        self.recent.record_pinned(&origin, &peer.server_id);
        Ok(to_dto(&peer))
    }

    async fn discovery_document(&self, server_id: &str) -> ServiceResult<Option<String>> {
        let Some(peer) = self.repo.find(server_id).await? else {
            return Ok(None);
        };
        let statement = |payload: &str, signature: &str| serde_json::json!({ "payload": payload, "signature": signature });
        let log: Vec<serde_json::Value> = self
            .repo
            .key_log(server_id)
            .await?
            .iter()
            .map(|e| statement(&e.payload, &e.signature))
            .collect();
        Ok(Some(
            serde_json::json!({
                "rim": "server-discovery",
                "keyLog": log,
                "descriptor": statement(&peer.descriptor.payload, &peer.descriptor.signature),
            })
            .to_string(),
        ))
    }

    async fn handle_domain(&self, server_id: &str) -> ServiceResult<Option<String>> {
        Ok(self
            .repo
            .find(server_id)
            .await?
            .filter(|peer| peer.status == PeerStatus::Active)
            .and_then(|peer| {
                peer.handle_domain
                    .clone()
                    .filter(|checked| declared_handle_domain(&peer).as_ref() == Some(checked))
            }))
    }

    async fn pinned_key_log(&self, server_id: &str) -> ServiceResult<Vec<SignedJson>> {
        Ok(self
            .repo
            .key_log(server_id)
            .await?
            .into_iter()
            .map(|e| SignedJson {
                payload: e.payload,
                signature: e.signature,
            })
            .collect())
    }

    async fn reset(&self, server_id: &str) -> ServiceResult<bool> {
        let existed = self.repo.delete(server_id).await?;
        if existed {
            tracing::info!(
                target: "audit",
                event = "s2s.peer_reset",
                peer_server_id = %server_id,
                "forgot a peer server's pin"
            );
        }
        Ok(existed)
    }

    async fn set_blocked(&self, server_id: &str, blocked: bool) -> ServiceResult<PeerBlockChange> {
        let Some(peer) = self.repo.find(server_id).await? else {
            return Ok(PeerBlockChange::NotPinned);
        };
        let (from, to) = if blocked {
            (PeerStatus::Active, PeerStatus::Blocked)
        } else {
            (PeerStatus::Blocked, PeerStatus::Active)
        };
        if peer.status == PeerStatus::Forked {
            return Ok(PeerBlockChange::Forked);
        }
        if peer.status == to {
            return Ok(PeerBlockChange::Unchanged);
        }
        if !self.repo.replace_status(server_id, from, to).await? {
            // Changed since it was read: report what is stored now.
            return Ok(match self.repo.find(server_id).await?.map(|p| p.status) {
                None => PeerBlockChange::NotPinned,
                Some(PeerStatus::Forked) => PeerBlockChange::Forked,
                Some(_) => PeerBlockChange::Unchanged,
            });
        }
        tracing::info!(
            target: "audit",
            event = if blocked { "s2s.peer_blocked" } else { "s2s.peer_unblocked" },
            peer_server_id = %server_id,
            "an operator {} a peer server",
            if blocked { "blocked" } else { "unblocked" }
        );
        Ok(PeerBlockChange::Changed)
    }
}

#[cfg(test)]
#[path = "peers_unit_tests.rs"]
mod tests;
