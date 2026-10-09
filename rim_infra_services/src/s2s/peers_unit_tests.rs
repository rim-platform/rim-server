use std::collections::HashMap;
use std::sync::Mutex;
use std::sync::atomic::{AtomicUsize, Ordering};

use rim_core::repository::error::{RepoError, RepoErrorKind, RepoResult};
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerSigner, sign_descriptor,
    sign_inception, sign_rotation,
};

use super::*;

const ORIGIN: &str = "https://peer.example";

/// Pins in memory, with the conditional updates the database repository makes.
#[derive(Default)]
struct MemoryPeers {
    peers: Mutex<HashMap<String, (PeerServer, Vec<SignedRecord>)>>,
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

    async fn insert(&self, pin: PeerPin) -> RepoResult<PeerServer> {
        let mut peers = self.peers.lock().expect("lock");
        if peers.contains_key(&pin.server_id) {
            return Err(RepoError::new(RepoErrorKind::Conflict));
        }
        let peer = row(&pin, PeerStatus::Active);
        peers.insert(pin.server_id.clone(), (peer.clone(), pin.log));
        Ok(peer)
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

    async fn delete(&self, server_id: &str) -> RepoResult<bool> {
        Ok(self.peers.lock().expect("lock").remove(server_id).is_some())
    }

    async fn touch(&self, _server_id: &str) -> RepoResult<()> {
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
