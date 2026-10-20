use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rim_core::repository::error::{RepoError, RepoResult};
use rim_core::s2s::model::ForgottenPeer;
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerSigner, sign_descriptor,
    sign_inception, sign_rotation,
};

use super::*;

const ORIGIN: &str = "https://peer.example";

/// Pins in memory, with the conditional updates, the origin limit and the forgotten heads the
/// database repository keeps.
#[derive(Default)]
struct MemoryPeers {
    peers: Mutex<HashMap<String, (PeerServer, Vec<SignedRecord>)>>,
    forgotten: Mutex<HashMap<String, ForgottenPeer>>,
}

impl MemoryPeers {
    fn set_rotated_at(&self, id: &str, at: DateTime<Utc>) {
        let mut peers = self.peers.lock().expect("lock");
        peers.get_mut(id).expect("peer").0.rotated_at = Some(at);
    }

    fn set_fetched_at(&self, id: &str, at: DateTime<Utc>) {
        let mut peers = self.peers.lock().expect("lock");
        peers.get_mut(id).expect("peer").0.fetched_at = at;
    }
}

fn row(pin: &PeerPin, status: PeerStatus) -> PeerServer {
    PeerServer {
        server_id: pin.server_id.clone(),
        origin: pin.origin.clone(),
        head_seq: pin.head_seq,
        head_key: pin.head_key.clone(),
        previous_key: pin.previous_key.clone(),
        rotated_at: pin.rotated_at,
        rotation_signer: pin.rotation_signer,
        descriptor_version: pin.descriptor_version,
        descriptor: pin.descriptor.clone(),
        status,
        first_seen_at: Utc::now(),
        fetched_at: Utc::now(),
        last_request_at: None,
        handle_domain: None,
    }
}

#[async_trait]
impl PeerServerRepository for MemoryPeers {
    async fn find(&self, server_id: &str) -> RepoResult<Option<PeerServer>> {
        Ok(self
            .peers
            .lock()
            .expect("lock")
            .get(server_id)
            .map(|(p, _)| p.clone()))
    }

    async fn key_log(&self, server_id: &str) -> RepoResult<Vec<SignedRecord>> {
        Ok(self
            .peers
            .lock()
            .expect("lock")
            .get(server_id)
            .map(|(_, log)| log.clone())
            .unwrap_or_default())
    }

    async fn insert(&self, pin: PeerPin, status: PeerStatus) -> RepoResult<PeerServer> {
        let mut peers = self.peers.lock().expect("lock");
        if peers.contains_key(&pin.server_id) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        if peers
            .values()
            .filter(|(p, _)| p.origin == pin.origin)
            .count()
            >= MAX_PEERS_PER_ORIGIN
        {
            return Err(RepoError::new(RepoErrorKind::PreconditionFailed));
        }
        let peer = row(&pin, status);
        self.forgotten.lock().expect("lock").remove(&pin.server_id);
        peers.insert(pin.server_id.clone(), (peer.clone(), pin.log));
        Ok(peer)
    }

    async fn forgotten(&self, server_id: &str) -> RepoResult<Option<ForgottenPeer>> {
        Ok(self.forgotten.lock().expect("lock").get(server_id).cloned())
    }

    async fn forget_idle(
        &self,
        idle_before: DateTime<Utc>,
        kept: &[String],
    ) -> RepoResult<Vec<String>> {
        let mut peers = self.peers.lock().expect("lock");
        let idle: Vec<String> = peers
            .iter()
            .filter(|(id, (p, _))| {
                p.status == PeerStatus::Active && p.fetched_at < idle_before && !kept.contains(id)
            })
            .map(|(id, _)| id.clone())
            .collect();
        for id in &idle {
            if let Some((peer, log)) = peers.remove(id) {
                self.forgotten.lock().expect("lock").insert(
                    id.clone(),
                    ForgottenPeer {
                        server_id: id.clone(),
                        head_seq: peer.head_seq,
                        head_payload: log[peer.head_seq as usize].payload.clone(),
                    },
                );
            }
        }
        Ok(idle)
    }

    async fn advance(&self, pin: PeerPin, expected: u32) -> RepoResult<Option<PeerServer>> {
        let mut peers = self.peers.lock().expect("lock");
        let Some((peer, log)) = peers.get_mut(&pin.server_id) else {
            return Ok(None);
        };
        if peer.head_seq != expected || peer.status != PeerStatus::Active {
            return Ok(None);
        }
        *peer = PeerServer {
            first_seen_at: peer.first_seen_at,
            ..row(&pin, PeerStatus::Active)
        };
        *log = pin.log;
        Ok(Some(peer.clone()))
    }

    async fn mark_fetched(
        &self,
        server_id: &str,
        descriptor: Option<(u32, SignedRecord)>,
    ) -> RepoResult<()> {
        let mut peers = self.peers.lock().expect("lock");
        if let Some((peer, _)) = peers.get_mut(server_id) {
            peer.fetched_at = Utc::now();
            if let Some((version, record)) = descriptor
                && version > peer.descriptor_version
            {
                peer.descriptor_version = version;
                peer.descriptor = record;
            }
        }
        Ok(())
    }

    async fn set_status(&self, server_id: &str, status: PeerStatus) -> RepoResult<bool> {
        let mut peers = self.peers.lock().expect("lock");
        Ok(peers
            .get_mut(server_id)
            .map(|(peer, _)| peer.status = status)
            .is_some())
    }

    async fn replace_status(
        &self,
        server_id: &str,
        from: PeerStatus,
        to: PeerStatus,
    ) -> RepoResult<bool> {
        let mut peers = self.peers.lock().expect("lock");
        Ok(match peers.get_mut(server_id) {
            Some((peer, _)) if peer.status == from => {
                peer.status = to;
                true
            }
            _ => false,
        })
    }

    async fn delete(&self, server_id: &str) -> RepoResult<bool> {
        let head = self.forgotten.lock().expect("lock").remove(server_id);
        Ok(self.peers.lock().expect("lock").remove(server_id).is_some() || head.is_some())
    }

    async fn touch(&self, _server_id: &str) -> RepoResult<()> {
        Ok(())
    }

    async fn set_handle_domain(&self, server_id: &str, domain: Option<String>) -> RepoResult<()> {
        if let Some((peer, _)) = self.peers.lock().expect("lock").get_mut(server_id) {
            peer.handle_domain = domain;
        }
        Ok(())
    }
}

/// Serves whatever document was last set for an origin, and counts fetches.
#[derive(Default)]
struct FakeFetcher {
    docs: Mutex<HashMap<String, Vec<u8>>>,
    fetches: AtomicUsize,
}

impl FakeFetcher {
    fn serve(&self, origin: &str, doc: Vec<u8>) {
        self.docs
            .lock()
            .expect("lock")
            .insert(origin.to_string(), doc);
    }

    fn fetches(&self) -> usize {
        self.fetches.load(Ordering::SeqCst)
    }

    fn stop_serving(&self, origin: &str) {
        self.docs.lock().expect("lock").remove(origin);
    }
}

#[async_trait]
impl DiscoveryFetcher for FakeFetcher {
    async fn fetch(&self, origin: &str) -> Result<Vec<u8>, NetError> {
        self.fetches.fetch_add(1, Ordering::SeqCst);
        self.docs
            .lock()
            .expect("lock")
            .get(origin)
            .cloned()
            .ok_or_else(|| NetError::Transport("unreachable".into()))
    }
}

/// A peer server's keys and log, able to rotate and to publish its discovery document.
#[derive(Clone)]
struct Peer {
    seed: u8,
    recovery_seed: u8,
    log: Vec<SignedStatement>,
    state: ServerKeyState,
    version: u32,
    handle_domain: Option<String>,
}

impl Peer {
    fn new(seed: u8) -> Self {
        let signer = ServerSigner::from_seed([seed; 32]);
        let recovery_seed = seed.wrapping_add(100);
        let recovery = ServerSigner::from_seed([recovery_seed; 32]);
        let inception = sign_inception(&signer, &recovery, Utc::now());
        let state = ServerKeyState::from_inception(&inception).expect("inception");
        Self {
            seed,
            recovery_seed,
            log: vec![inception],
            state,
            version: 1,
            handle_domain: None,
        }
    }

    fn signer(&self) -> ServerSigner {
        ServerSigner::from_seed([self.seed; 32])
    }

    fn id(&self) -> String {
        self.state.server_id.clone()
    }

    fn rotate(&mut self, seed: u8, kind: ServerRotationSigner) {
        let next = ServerSigner::from_seed([seed; 32]);
        let by = match kind {
            ServerRotationSigner::Server => self.signer(),
            ServerRotationSigner::Recovery => ServerSigner::from_seed([self.recovery_seed; 32]),
        };
        let entry = sign_rotation(
            &self.state,
            &by,
            kind,
            &next.public_key(),
            &self.state.recovery_key,
            Utc::now(),
        );
        self.state = self.state.apply(&entry).expect("rotate");
        self.log.push(entry);
        self.seed = seed;
        self.version += 1;
    }

    fn document(&self, s2s: Option<&str>) -> Vec<u8> {
        let descriptor = sign_descriptor(
            &self.signer(),
            &ServerDescriptor::new(
                self.id(),
                self.state.seq,
                self.version,
                vec![Role::Host, Role::Relay],
                Endpoints {
                    graphql: format!("{ORIGIN}/api"),
                    websocket: "wss://peer.example/api/ws".into(),
                    s2s: s2s.map(str::to_string),
                },
                vec![79],
                Limits {
                    max_body_bytes: 8 * 1024 * 1024,
                    clock_skew_seconds: 300,
                    max_deliver_items: 256,
                },
                Registration {
                    mode: RegistrationMode::Invite,
                    pow_bits: None,
                },
                self.handle_domain.clone(),
                "2026-10-09T00:00:00Z".into(),
            ),
        )
        .expect("descriptor");
        let stmt = |s: &SignedStatement| serde_json::json!({"payload": s.payload, "signature": s.signature});
        serde_json::to_vec(&serde_json::json!({
            "rim": "server-discovery",
            "keyLog": self.log.iter().map(stmt).collect::<Vec<_>>(),
            "descriptor": stmt(&descriptor),
        }))
        .expect("json")
    }

    fn publish(&self, fetcher: &FakeFetcher) {
        fetcher.serve(ORIGIN, self.document(Some("https://peer.example/s2s/v1")));
    }
}

fn service_over(repo: &Arc<MemoryPeers>, fetcher: &Arc<FakeFetcher>) -> DefaultPeerService {
    DefaultPeerService::new(
        Arc::clone(repo) as Arc<dyn PeerServerRepository>,
        Arc::clone(fetcher) as Arc<dyn DiscoveryFetcher>,
        false,
    )
}

fn service() -> (DefaultPeerService, Arc<MemoryPeers>, Arc<FakeFetcher>) {
    let repo = Arc::new(MemoryPeers::default());
    let fetcher = Arc::new(FakeFetcher::default());
    (service_over(&repo, &fetcher), repo, fetcher)
}

fn ready(lookup: PeerLookup) -> PeerKeysDto {
    match lookup {
        PeerLookup::Ready(keys) => keys,
        other => panic!("expected a ready peer, got {other:?}"),
    }
}

#[tokio::test]
async fn an_unknown_peer_is_pinned_from_its_origin() {
    let (service, _, fetcher) = service();
    let peer = Peer::new(1);
    peer.publish(&fetcher);
    let keys = ready(
        service
            .resolve(&peer.id(), Some("https://Peer.Example"), Some(0))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 0);
    assert_eq!(keys.head_key, peer.signer().public_key().to_string());
    assert_eq!(keys.origin, ORIGIN);
    assert_eq!(
        keys.s2s_endpoint.as_deref(),
        Some("https://peer.example/s2s/v1")
    );
    assert_eq!(keys.key_for_seq(0), Some(keys.head_key.as_str()));
    assert_eq!(keys.key_for_seq(1), None);
}

#[tokio::test]
async fn an_origin_serving_another_server_does_not_pin() {
    let (service, repo, fetcher) = service();
    let peer = Peer::new(1);
    Peer::new(2).publish(&fetcher);
    assert_eq!(
        service
            .resolve(&peer.id(), Some(ORIGIN), Some(0))
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
    assert!(repo.find(&peer.id()).await.expect("find").is_none());
    // No origin to pin from, or one that is not https.
    assert_eq!(
        service
            .resolve(&peer.id(), None, None)
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
    assert_eq!(
        service
            .resolve(&peer.id(), Some("http://peer.example"), None)
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
}

#[tokio::test]
async fn a_peer_with_server_to_server_off_or_elsewhere_is_not_pinned() {
    let (service, _, fetcher) = service();
    let peer = Peer::new(1);
    fetcher.serve(ORIGIN, peer.document(None));
    assert_eq!(
        service
            .resolve(&peer.id(), Some(ORIGIN), None)
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
    fetcher.serve(
        ORIGIN,
        peer.document(Some("https://elsewhere.example/s2s/v1")),
    );
    assert!(matches!(
        service.pin(ORIGIN, Some(&peer.id())).await,
        Err(ServiceError::Validation(_))
    ));
}

#[tokio::test]
async fn a_server_rotation_keeps_the_previous_key_for_the_grace_period() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    let old_key = peer.signer().public_key().to_string();

    peer.rotate(11, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    let keys = ready(
        service
            .resolve(&peer.id(), None, Some(1))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 1);
    assert_eq!(keys.key_for_seq(0), Some(old_key.as_str()));
    assert_eq!(
        keys.key_for_seq(1),
        Some(peer.signer().public_key().to_string().as_str())
    );

    // 24 hours after the rotation was learned, the old key is refused.
    repo.set_rotated_at(&peer.id(), Utc::now() - chrono::Duration::hours(25));
    let keys = ready(
        service
            .resolve(&peer.id(), None, Some(1))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.key_for_seq(0), None);
}

#[tokio::test]
async fn a_recovery_rotation_drops_the_previous_key_at_once() {
    let (service, _, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    peer.rotate(12, ServerRotationSigner::Recovery);
    peer.publish(&fetcher);
    let keys = ready(
        service
            .resolve(&peer.id(), None, Some(1))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 1);
    assert_eq!(keys.previous_key, None);
    assert_eq!(keys.key_for_seq(0), None);
}

#[tokio::test]
async fn a_forked_log_marks_the_peer_until_reset() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    let mut fork = peer.clone();
    peer.rotate(11, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    ready(
        service
            .resolve(&peer.id(), None, Some(1))
            .await
            .expect("resolve"),
    );

    // Another history from the same inception, fetched by a service whose minute is fresh.
    fork.rotate(13, ServerRotationSigner::Server);
    fork.rotate(14, ServerRotationSigner::Server);
    fork.publish(&fetcher);
    let later = service_over(&repo, &fetcher);
    assert_eq!(
        later
            .resolve(&peer.id(), None, Some(2))
            .await
            .expect("resolve"),
        PeerLookup::Forked
    );
    let stored = repo.find(&peer.id()).await.expect("find").expect("peer");
    assert_eq!(stored.status, PeerStatus::Forked);
    assert_eq!(
        later
            .resolve(&peer.id(), Some(ORIGIN), Some(0))
            .await
            .expect("resolve"),
        PeerLookup::Forked
    );
    assert!(matches!(
        service_over(&repo, &fetcher).pin(ORIGIN, None).await,
        Err(ServiceError::Forbidden(_))
    ));

    assert!(later.reset(&peer.id()).await.expect("reset"));
    let keys = ready(
        service_over(&repo, &fetcher)
            .resolve(&fork.id(), Some(ORIGIN), Some(2))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 2);
}

#[tokio::test]
async fn refreshes_are_limited_to_one_a_minute_per_peer() {
    let (service, _, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.publish(&fetcher);
    ready(
        service
            .resolve(&peer.id(), Some(ORIGIN), None)
            .await
            .expect("resolve"),
    );
    assert_eq!(fetcher.fetches(), 1);
    peer.rotate(11, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    // The pin is a second old: its peer's minute is used up, so the higher seq is not fetched.
    let keys = ready(
        service
            .resolve(&peer.id(), None, Some(1))
            .await
            .expect("resolve"),
    );
    assert_eq!(fetcher.fetches(), 1);
    assert_eq!(keys.head_seq, 0);
}

#[tokio::test]
async fn a_stale_pin_is_refreshed_and_a_failed_refresh_keeps_it() {
    let (_, repo, fetcher) = service();
    let peer = Peer::new(1);
    peer.publish(&fetcher);
    service_over(&repo, &fetcher)
        .pin(ORIGIN, None)
        .await
        .expect("pin");
    repo.set_fetched_at(&peer.id(), Utc::now() - chrono::Duration::hours(7));
    fetcher.docs.lock().expect("lock").clear();
    let keys = ready(
        service_over(&repo, &fetcher)
            .resolve(&peer.id(), None, None)
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 0);
    assert_eq!(fetcher.fetches(), 2);
}

#[tokio::test]
async fn an_equal_log_takes_a_newer_descriptor() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.publish(&fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    peer.version = 5;
    peer.publish(&fetcher);
    repo.set_fetched_at(&peer.id(), Utc::now() - chrono::Duration::hours(7));
    ready(
        service_over(&repo, &fetcher)
            .resolve(&peer.id(), None, None)
            .await
            .expect("resolve"),
    );
    let stored = repo.find(&peer.id()).await.expect("find").expect("peer");
    assert_eq!(stored.descriptor_version, 5);
}

/// Forget every idle pin in `repo`, as the purge does once they have been idle long enough.
async fn forget_all(repo: &MemoryPeers) -> Vec<String> {
    repo.forget_idle(Utc::now() + chrono::Duration::seconds(1), &[])
        .await
        .expect("forget")
}

#[tokio::test]
async fn a_forgotten_peer_whose_log_extends_its_head_is_pinned_again() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.rotate(11, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    assert_eq!(forget_all(&repo).await, vec![peer.id()]);
    assert!(repo.find(&peer.id()).await.expect("find").is_none());

    peer.rotate(12, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    let keys = ready(
        service_over(&repo, &fetcher)
            .resolve(&peer.id(), Some(ORIGIN), Some(2))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 2);
    assert!(repo.forgotten(&peer.id()).await.expect("head").is_none());
}

#[tokio::test]
async fn a_forgotten_peer_serving_another_history_is_pinned_forked_until_reset() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    let mut fork = peer.clone();
    peer.rotate(11, ServerRotationSigner::Server);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    forget_all(&repo).await;

    fork.rotate(13, ServerRotationSigner::Server);
    fork.rotate(14, ServerRotationSigner::Server);
    fork.publish(&fetcher);
    assert!(matches!(
        service_over(&repo, &fetcher).pin(ORIGIN, None).await,
        Err(ServiceError::Forbidden(_))
    ));
    assert!(service.reset(&peer.id()).await.expect("reset"));
    peer.publish(&fetcher);
    service
        .pin(ORIGIN, Some(&peer.id()))
        .await
        .expect("pin again");
    forget_all(&repo).await;
    fork.publish(&fetcher);
    assert_eq!(
        service_over(&repo, &fetcher)
            .resolve(&peer.id(), Some(ORIGIN), Some(2))
            .await
            .expect("resolve"),
        PeerLookup::Forked
    );
    let stored = repo.find(&peer.id()).await.expect("find").expect("peer");
    assert_eq!(stored.status, PeerStatus::Forked);

    assert!(service.reset(&peer.id()).await.expect("reset"));
    let keys = ready(
        service_over(&repo, &fetcher)
            .resolve(&fork.id(), Some(ORIGIN), Some(2))
            .await
            .expect("resolve"),
    );
    assert_eq!(keys.head_seq, 2);
}

#[tokio::test]
async fn a_forgotten_peer_serving_a_log_short_of_its_head_is_not_pinned() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    let older = peer.clone();
    peer.rotate(11, ServerRotationSigner::Recovery);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    forget_all(&repo).await;

    older.publish(&fetcher);
    assert_eq!(
        service_over(&repo, &fetcher)
            .resolve(&peer.id(), Some(ORIGIN), Some(0))
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
    assert!(matches!(
        service_over(&repo, &fetcher).pin(ORIGIN, None).await,
        Err(ServiceError::Conflict(_))
    ));
    assert!(repo.find(&peer.id()).await.expect("find").is_none());
    assert!(repo.forgotten(&peer.id()).await.expect("head").is_some());
}

#[tokio::test]
async fn a_reset_forgets_a_forgotten_head_too() {
    let (service, repo, fetcher) = service();
    let peer = Peer::new(1);
    peer.publish(&fetcher);
    service.pin(ORIGIN, Some(&peer.id())).await.expect("pin");
    forget_all(&repo).await;
    assert!(service.reset(&peer.id()).await.expect("reset"));
    assert!(repo.forgotten(&peer.id()).await.expect("head").is_none());
    assert!(!service.reset(&peer.id()).await.expect("reset"));
}

#[tokio::test]
async fn an_origin_holds_at_most_its_limit_of_pins() {
    let (_, repo, fetcher) = service();
    let peers: Vec<Peer> = (1..=5).map(Peer::new).collect();
    for peer in &peers[..MAX_PEERS_PER_ORIGIN] {
        peer.publish(&fetcher);
        service_over(&repo, &fetcher)
            .pin(ORIGIN, None)
            .await
            .expect("pin");
    }
    let last = &peers[MAX_PEERS_PER_ORIGIN];
    last.publish(&fetcher);
    assert!(matches!(
        service_over(&repo, &fetcher).pin(ORIGIN, None).await,
        Err(ServiceError::Conflict(_))
    ));
    assert_eq!(
        service_over(&repo, &fetcher)
            .resolve(&last.id(), Some(ORIGIN), None)
            .await
            .expect("resolve"),
        PeerLookup::Unknown
    );
    // A server pinned already is refreshed, not refused.
    peers[0].publish(&fetcher);
    service_over(&repo, &fetcher)
        .pin(ORIGIN, None)
        .await
        .expect("refresh");
}

#[tokio::test]
async fn a_peer_declaring_its_own_host_gets_its_handle_domain_without_another_fetch() {
    let (service, _, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.handle_domain = Some("peer.example".into());
    peer.publish(&fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        Some("peer.example".to_string())
    );
    assert_eq!(fetcher.fetches(), 1);
}

#[tokio::test]
async fn a_delegated_handle_domain_counts_only_while_it_points_back_at_the_peer() {
    const APEX: &str = "https://peer.org";
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.handle_domain = Some("peer.org".into());
    peer.publish(&fetcher);
    let delegate =
        |to: &str| serde_json::to_vec(&serde_json::json!({ "delegate": to })).expect("json");
    fetcher.serve(APEX, delegate(ORIGIN));
    service.pin(ORIGIN, None).await.expect("pin");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        Some("peer.org".to_string())
    );
    assert_eq!(fetcher.fetches(), 3);

    // The domain does not answer: what was recorded stays.
    fetcher.stop_serving(APEX);
    let service = service_over(&repo, &fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        Some("peer.org".to_string())
    );

    // The domain now delegates to another server declaring it: the peer loses it.
    const OTHER: &str = "https://other.example";
    let mut other = Peer::new(2);
    other.handle_domain = Some("peer.org".into());
    fetcher.serve(OTHER, other.document(Some("https://other.example/s2s/v1")));
    fetcher.serve(APEX, delegate(OTHER));
    let service = service_over(&repo, &fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        None
    );
    assert_eq!(
        repo.find(&peer.id())
            .await
            .expect("find")
            .expect("peer")
            .handle_domain,
        None
    );
}

#[tokio::test]
async fn a_checked_domain_the_descriptor_no_longer_declares_does_not_count() {
    let (service, repo, fetcher) = service();
    let mut peer = Peer::new(1);
    peer.handle_domain = Some("peer.example".into());
    peer.publish(&fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    // A newer descriptor declares another domain; the stored check is for the old one.
    peer.handle_domain = Some("elsewhere.example".into());
    peer.version += 1;
    peer.publish(&fetcher);
    repo.mark_fetched(
        &peer.id(),
        Some((
            peer.version,
            serde_json::from_slice::<serde_json::Value>(&peer.document(None))
                .ok()
                .and_then(|d| {
                    Some(SignedRecord {
                        payload: d["descriptor"]["payload"].as_str()?.to_string(),
                        signature: d["descriptor"]["signature"].as_str()?.to_string(),
                    })
                })
                .expect("descriptor"),
        )),
    )
    .await
    .expect("mark");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        None
    );
    // A blocked peer has no handle domain either.
    let (service, _, fetcher) = self::service();
    let mut peer = Peer::new(3);
    peer.handle_domain = Some("peer.example".into());
    peer.publish(&fetcher);
    service.pin(ORIGIN, None).await.expect("pin");
    service.set_blocked(&peer.id(), true).await.expect("block");
    assert_eq!(
        service.handle_domain(&peer.id()).await.expect("domain"),
        None
    );
}
