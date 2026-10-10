//! Pinning peers from their discovery documents and keeping the pins current.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use rim_core::s2s::model::{PeerPin, PeerRotationSigner, PeerServer, PeerStatus, SignedRecord};
use rim_core::s2s::repository::PeerServerRepository;
use rim_identity::SignedStatement;
use rim_s2s::{
    LogRelation, ServerDescriptor, ServerKeyState, ServerRotationSigner, compare_logs,
    verify_descriptor,
};
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::relay::dto::SignedJson;
use rim_services::s2s::dto::{PeerBlockChange, PeerDto, PeerKeysDto, PeerLookup};
use rim_services::s2s::service::PeerService;
use rim_services::{ServiceError, ServiceResult};
use serde::Deserialize;

use crate::alert_rate_limiter::AlertRateLimiter;
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
    per_peer: AlertRateLimiter,
    overall: AlertRateLimiter,
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
            per_peer: AlertRateLimiter::new(60, 1),
            overall: AlertRateLimiter::new(60, 60),
        }
    }

    /// Whether a fetch for `server_id` fits one per peer per minute and 60 a minute in all.
    fn may_fetch(&self, server_id: &str) -> bool {
        let now = Utc::now();
        if matches!(
            self.per_peer.check_and_record(server_id, now),
            RateLimitDecision::Limited { .. }
        ) {
            crate::metrics::rate_limited("peer_discovery");
            return false;
        }
        let allowed = matches!(
            self.overall.check_and_record("all", now),
            RateLimitDecision::Allowed
        );
        if !allowed {
            crate::metrics::rate_limited("peer_discovery");
        }
        allowed
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

    /// Pin a peer not pinned before. A concurrent pin of the same peer wins and is returned.
    async fn insert_pin(
        &self,
        origin: &str,
        found: &VerifiedDiscovery,
    ) -> ServiceResult<PeerServer> {
        let pin = pin_from(origin, found, None);
        match self.repo.insert(pin).await {
            Ok(peer) => {
                tracing::info!(
                    target: "audit",
                    event = "s2s.peer_pinned",
                    peer_server_id = %peer.server_id,
                    head_seq = peer.head_seq,
                    "pinned a peer server"
                );
                Ok(peer)
            }
            Err(e) if e.kind() == rim_core::repository::error::RepoErrorKind::Conflict => self
                .repo
                .find(&found.state.server_id)
                .await?
                .ok_or(ServiceError::Conflict("peer pin raced".to_string())),
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
        self.repo
            .find(&peer.server_id)
            .await
            .map_err(failed)?
            .ok_or_else(|| RefreshError::Failed("peer was reset".to_string()))
    }

    /// Refresh within the limits; a failure keeps the pin as it was.
    async fn refresh_or_keep(&self, peer: PeerServer) -> ServiceResult<PeerLookup> {
        if !self.may_fetch(&peer.server_id) {
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
    ) -> ServiceResult<PeerLookup> {
        let Some(peer) = self.repo.find(server_id).await? else {
            let Some(origin) = origin.and_then(|o| normalize_origin(o, self.allow_http).ok())
            else {
                return Ok(PeerLookup::Unknown);
            };
            if !self.may_fetch(server_id) {
                return Ok(PeerLookup::Unknown);
            }
            return match self.fetch_verified(&origin, Some(server_id)).await {
                Ok(found) => {
                    let peer = self.insert_pin(&origin, &found).await?;
                    Ok(lookup(&peer, Utc::now()))
                }
                Err(reason) => {
                    tracing::warn!(
                        peer_server_id = %server_id,
                        reason = %reason,
                        "could not pin an unknown peer"
                    );
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
            return self.refresh_or_keep(peer).await;
        }
        Ok(lookup(&peer, Utc::now()))
    }

    async fn refresh(&self, server_id: &str) -> ServiceResult<PeerLookup> {
        match self.repo.find(server_id).await? {
            Some(peer) if peer.status == PeerStatus::Active => self.refresh_or_keep(peer).await,
            Some(peer) => Ok(lookup(&peer, Utc::now())),
            None => Ok(PeerLookup::Unknown),
        }
    }

    async fn pin(&self, origin: &str, expected_server_id: Option<&str>) -> ServiceResult<PeerDto> {
        let origin = normalize_origin(origin, self.allow_http)
            .map_err(|e| ServiceError::Validation(e.to_string()))?;
        let bytes = self.fetcher.fetch(&origin).await.map_err(|e| {
            tracing::warn!(error = %e, "discovery fetch failed");
            ServiceError::Unavailable
        })?;
        let found = verify_discovery(&bytes, &origin, expected_server_id)
            .map_err(ServiceError::Validation)?;
        let peer = match self.repo.find(&found.state.server_id).await? {
            None => self.insert_pin(&origin, &found).await?,
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
        Ok(to_dto(&peer))
    }

    async fn get(&self, server_id: &str) -> ServiceResult<Option<PeerDto>> {
        Ok(self.repo.find(server_id).await?.as_ref().map(to_dto))
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
