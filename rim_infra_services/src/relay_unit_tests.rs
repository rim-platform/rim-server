//! The relay's key package claims for hosts (the stranger rule, blocks, the claim's host), its
//! queue cap, contacts, identity fetches and remote server lookups, and the host's checks of forwarded
//! device requests (the device, its signature, the forwarded body, the `seq` window, and which
//! relay may carry a user's traffic).

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::e2ee::model::{ClaimedKeyPackage, MlsKeyPackageId};
use rim_core::e2ee::repository::MockE2eeRepository;
use rim_core::host::model::{SeqDecision, UserKind};
use rim_core::host::repository::{MockDelegationRepository, MockHostRepository};
use rim_core::key_identity::model::{DeviceKey, DeviceKeyId, StoredProfileCard};
use rim_core::key_identity::repository::MockKeyIdentityRepository;
use rim_core::relay::model::{CachedIdentity, Contact, DroppedSpace};
use rim_core::relay::repository::{
    MockRelayAbuseRepository, MockRelayRepository, MockSubscriptionRepository,
};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::MockUserRepository;
use rim_identity::testing::{TestIdentity, signing_key};
use rim_identity::{KeyState, PublicKey, Purpose, device_request_payload, sign_with_context};
use rim_s2s::host::{HostEventFields, sign_host_event};
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerIdentity,
    ServerKeyState, ServerSigner, SharedIdentity, sign_descriptor, sign_inception, timestamp,
};
use rim_services::host::dto::{ForwardRefusal, ForwardRequest};
use rim_services::host::service::{HostService, MockDelegationCheck};
use rim_services::relay::dto::{ClaimConversation, Claimer, PeerClaimRequest};
use rim_services::relay::dto::{
    DeliverItem, IdentityDocument, QueueSpace, RelayNotice, SignedJson,
};
use rim_services::relay::service::RelayService;
use rim_services::s2s::dto::{PeerDto, PeerKeysDto, PeerLookup, S2sClientError, S2sResponse};
use rim_services::s2s::service::{MockPeerService, MockS2sClient};
use rim_services::{Actor, ServiceError};
use uuid::Uuid;

use crate::host::{DefaultHostService, HostRepos};
use crate::quota::Quotas;
use crate::relay::identity::testing::document;
use crate::relay::{DefaultRelayService, RelayRepos, RelaySettings};

const PEER: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
const PEER_ORIGIN: &str = "https://relay.example";
const BOB: &str = "rimp1_bob";
const ALICE: &str = "rimp1_alice";
const QUEUE_CAP: i64 = 1 << 20;

fn identity(seed: u8) -> SharedIdentity {
    let signer = ServerSigner::from_seed([seed; 32]);
    let recovery = ServerSigner::from_seed([seed.wrapping_add(100); 32]);
    let inception = sign_inception(&signer, &recovery, Utc::now());
    let state = ServerKeyState::from_inception(&inception).expect("inception");
    let content = ServerDescriptor::new(
        state.server_id.clone(),
        0,
        1,
        vec![Role::Host, Role::Relay],
        Endpoints {
            graphql: "https://host.example/api".into(),
            websocket: "wss://host.example/api/ws".into(),
            s2s: Some("https://host.example/s2s/v1".into()),
        },
        vec![79],
        Limits {
            max_body_bytes: 8_388_608,
            clock_skew_seconds: 300,
            max_deliver_items: 256,
        },
        Registration {
            mode: RegistrationMode::Invite,
            pow_bits: None,
        },
        timestamp(Utc::now()),
    );
    let descriptor = sign_descriptor(&signer, &content).expect("descriptor");
    SharedIdentity::new(ServerIdentity {
        signer,
        state,
        log: vec![inception],
        descriptor,
        descriptor_content: content,
    })
}

/// Operator blocks of exactly the principals in `blocked`.
fn blocks(blocked: &[&str]) -> MockRelayAbuseRepository {
    let blocked: Vec<String> = blocked.iter().map(|p| p.to_string()).collect();
    let mut abuse = MockRelayAbuseRepository::new();
    abuse.expect_is_principal_blocked().returning(move |p| {
        let found = blocked.iter().any(|b| b == p);
        Box::pin(async move { Ok(found) })
    });
    abuse
}

fn device(id: DeviceKeyId, user_id: UserId, revoked: bool) -> DeviceKey {
    DeviceKey {
        id,
        user_id,
        device_id: Uuid::from_u128(1),
        public_key: PublicKey::from(&signing_key(20)).to_string(),
        name: "Laptop".to_string(),
        cert_payload: "{}".to_string(),
        cert_signature: "ed25519:x".to_string(),
        key_seq: 0,
        added_at: Utc::now(),
        last_used_at: None,
        revoked_at: revoked.then(Utc::now),
        revocation_payload: None,
        revocation_signature: None,
    }
}

/// Repositories with no expectations, where no principal is blocked.
fn empty_repos() -> RelayRepos {
    RelayRepos {
        relay_repo: Arc::new(MockRelayRepository::new()),
        host_repo: Arc::new(MockHostRepository::new()),
        key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
        e2ee_repo: Arc::new(MockE2eeRepository::new()),
        user_repo: Arc::new(MockUserRepository::new()),
        subscription_repo: Arc::new(MockSubscriptionRepository::new()),
        relay_abuse_repo: Arc::new(blocks(&[])),
    }
}

/// A relay over `repos` whose queue holds `QUEUE_CAP` bytes per user.
fn relay_service(
    repos: RelayRepos,
    peers: MockPeerService,
    client: MockS2sClient,
) -> DefaultRelayService {
    DefaultRelayService::new(
        repos,
        None,
        identity(30),
        Arc::new(peers),
        Arc::new(client),
        RelaySettings {
            queue_bytes_per_user: QUEUE_CAP,
            retention: Duration::days(30),
            quotas: Quotas::default(),
        },
    )
}

// ── The relay's claims for hosts ─────────────────────────────────────────────────────────

/// A relay holding Bob, whose one device has packages; `one_time` records each claim's flag.
fn relay(contact: bool, blocked: bool, one_time: Arc<Mutex<Vec<bool>>>) -> DefaultRelayService {
    relay_with_blocks(contact, blocked, one_time, &[])
}

/// [`relay`] with operator blocks of `server_blocks`.
fn relay_with_blocks(
    contact: bool,
    blocked: bool,
    one_time: Arc<Mutex<Vec<bool>>>,
    server_blocks: &[&str],
) -> DefaultRelayService {
    let bob = UserId::new();
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((bob, UserKind::Local))) }));
    host.expect_shares_workspace()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let mut relay = MockRelayRepository::new();
    relay
        .expect_is_blocked()
        .returning(move |_, _| Box::pin(async move { Ok(blocked) }));
    relay
        .expect_is_contact()
        .returning(move |_, _| Box::pin(async move { Ok(contact) }));
    relay
        .expect_accepted_from()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let mut keys = MockKeyIdentityRepository::new();
    keys.expect_get_device().returning(move |user, _| {
        Box::pin(async move { Ok(Some(device(DeviceKeyId::new(), user, false))) })
    });
    let mut e2ee = MockE2eeRepository::new();
    e2ee.expect_claim_key_package_for_peer()
        .returning(move |device_key_id, _, allowed| {
            one_time.lock().expect("lock").push(allowed);
            Box::pin(async move {
                Ok(Some(ClaimedKeyPackage {
                    id: MlsKeyPackageId::new(),
                    device_key_id,
                    key_package_ref: vec![1],
                    key_package: vec![2],
                    last_resort: !allowed,
                }))
            })
        });
    relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            key_identity_repo: Arc::new(keys),
            e2ee_repo: Arc::new(e2ee),
            relay_abuse_repo: Arc::new(blocks(server_blocks)),
            ..empty_repos()
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    )
}

fn claim(host: &str) -> PeerClaimRequest {
    PeerClaimRequest {
        principal_id: BOB.to_string(),
        devices: vec![Uuid::from_u128(1)],
        conversation: ClaimConversation {
            host: host.to_string(),
            conversation_id: Uuid::now_v7(),
            incarnation: 1,
        },
        claimer: Claimer {
            principal_id: ALICE.to_string(),
            device_id: Uuid::from_u128(7),
        },
        cipher_suite: None,
    }
}

#[tokio::test]
async fn a_stranger_gets_the_last_resort_package_only() {
    let flags = Arc::default();
    let svc = relay(false, false, Arc::clone(&flags));
    let answer = svc
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .expect("claim");
    assert_eq!(*flags.lock().expect("lock"), vec![false]);
    assert!(answer.key_packages[0].last_resort);
}

#[tokio::test]
async fn a_contact_gets_a_one_time_package() {
    let flags = Arc::default();
    let svc = relay(true, false, Arc::clone(&flags));
    let answer = svc
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .expect("claim");
    assert_eq!(*flags.lock().expect("lock"), vec![true]);
    assert!(!answer.key_packages[0].last_resort);
}

#[tokio::test]
async fn a_blocked_claimer_and_another_host_are_refused() {
    let svc = relay(true, true, Arc::default());
    let err = svc
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
    let err = relay(true, false, Arc::default())
        .claim_for_peer(PEER.to_string(), claim("rims1_another"))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
}

#[tokio::test]
async fn principals_blocked_on_this_server_neither_claim_nor_are_claimed() {
    let err = relay_with_blocks(true, false, Arc::default(), &[ALICE])
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
    let err = relay_with_blocks(true, false, Arc::default(), &[BOB])
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");
}

#[tokio::test]
async fn a_stranger_claims_twenty_times_a_day() {
    let svc = relay(false, false, Arc::default());
    for _ in 0..20 {
        svc.claim_for_peer(PEER.to_string(), claim(PEER))
            .await
            .expect("within the limit");
    }
    let err = svc
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .unwrap_err();
    assert!(matches!(err, ServiceError::RateLimited { .. }), "{err:?}");
    let contact = relay(true, false, Arc::default());
    for _ in 0..25 {
        contact
            .claim_for_peer(PEER.to_string(), claim(PEER))
            .await
            .expect("contacts are not limited");
    }
}

#[test]
fn a_strangers_refused_claim_is_counted_under_its_limiter() {
    let ((), got) = crate::metrics::test_support::recorded(|| {
        crate::metrics::test_support::block_on(async {
            let svc = relay(false, false, Arc::default());
            for _ in 0..21 {
                let _ = svc.claim_for_peer(PEER.to_string(), claim(PEER)).await;
            }
        })
    });
    assert_eq!(
        got.counter(
            crate::metrics::RATE_LIMIT_REJECTIONS,
            &[("limiter", "stranger_claim")]
        ),
        1
    );
}

// ── The relay queue ──────────────────────────────────────────────────────────────────────

fn host_signer(seed: u8) -> ServerSigner {
    ServerSigner::from_seed([seed; 32])
}

fn peer_keys(signer: &ServerSigner, head_seq: u32) -> PeerKeysDto {
    PeerKeysDto {
        server_id: PEER.to_string(),
        origin: PEER_ORIGIN.to_string(),
        head_seq,
        head_key: signer.public_key().to_string(),
        previous_key: None,
        s2s_endpoint: None,
    }
}

/// Peers that resolve `PEER` to `keys`, and to `refetched` when asked for a newer key.
fn peers_resolving(keys: PeerKeysDto, refetched: Option<PeerKeysDto>) -> MockPeerService {
    let mut peers = MockPeerService::new();
    peers
        .expect_resolve()
        .withf(|server_id, _, key_seq| server_id == PEER && key_seq.is_none())
        .returning(move |_, _, _| {
            let keys = keys.clone();
            Box::pin(async move { Ok(PeerLookup::Ready(keys)) })
        });
    peers
        .expect_resolve()
        .withf(|server_id, _, key_seq| server_id == PEER && key_seq.is_some())
        .returning(move |_, _, _| {
            let lookup = refetched
                .clone()
                .map_or(PeerLookup::Unknown, PeerLookup::Ready);
            Box::pin(async move { Ok(lookup) })
        });
    peers
}

/// An item from `PEER` for Bob, signed by `signer` at `key_seq`.
fn host_item(signer: &ServerSigner, key_seq: u32) -> DeliverItem {
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let statement = sign_host_event(
        signer,
        &HostEventFields {
            host: PEER,
            key_seq,
            workspace_id: &workspace_id,
            stream: "e",
            seq: None,
            event_json: r#"{"type":"MessageCreated"}"#,
            issued_at: &issued_at,
        },
    );
    DeliverItem {
        id: "1".to_string(),
        envelope: SignedJson {
            payload: statement.payload,
            signature: statement.signature,
        },
        recipients: vec![BOB.to_string()],
        device_id: None,
    }
}

/// A relay that queues each delivered item for Bob, who then has `queued` bytes queued.
/// Dropping his oldest items frees `dropped`; `drops` records the byte count of each drop.
fn queueing_relay(
    peers: MockPeerService,
    queued: i64,
    dropped: Vec<DroppedSpace>,
    drops: Arc<Mutex<Vec<i64>>>,
) -> (DefaultRelayService, UserId) {
    let bob = UserId::new();
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((bob, UserKind::Local))) }));
    let mut subscriptions = MockSubscriptionRepository::new();
    subscriptions
        .expect_is_subscribed()
        .returning(|_, _, _, _| Box::pin(async { Ok(true) }));
    let mut relay = MockRelayRepository::new();
    relay
        .expect_received_through()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay
        .expect_advance_received_through()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    relay
        .expect_insert_envelope()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_insert_item()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_queued_bytes()
        .returning(move |_| Box::pin(async move { Ok(queued) }));
    relay.expect_drop_oldest().returning(move |user_id, bytes| {
        assert_eq!(user_id, bob);
        drops.lock().expect("lock").push(bytes);
        let dropped = dropped.clone();
        Box::pin(async move { Ok(dropped) })
    });
    let svc = relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            subscription_repo: Arc::new(subscriptions),
            ..empty_repos()
        },
        peers,
        MockS2sClient::new(),
    );
    (svc, bob)
}

/// Delivers one item signed by the pinned host and returns the notices it raised.
async fn deliver_one(svc: &DefaultRelayService) -> Vec<RelayNotice> {
    let signer = host_signer(40);
    let mut notices = svc.subscribe();
    let response = svc
        .deliver(PEER.to_string(), vec![host_item(&signer, 0)])
        .await
        .expect("delivered");
    assert!(response.refused.is_empty(), "{:?}", response.refused);
    std::iter::from_fn(|| notices.try_recv().ok()).collect()
}

fn dropped_space(host: &str, workspace: u128) -> DroppedSpace {
    DroppedSpace {
        host_server_id: host.to_string(),
        workspace_id: Uuid::from_u128(workspace),
    }
}

/// A queue over its cap drops the overflow from the oldest items and names the spaces it
/// dropped from in one notice.
#[tokio::test]
async fn a_queue_over_its_cap_drops_the_overflow_and_names_the_spaces() {
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (svc, bob) = queueing_relay(
        peers_resolving(peer_keys(&host_signer(40), 0), None),
        QUEUE_CAP + 300,
        vec![dropped_space(PEER, 5), dropped_space("rims1_other", 6)],
        drops.clone(),
    );

    let notices = deliver_one(&svc).await;

    assert_eq!(*drops.lock().expect("lock"), vec![300]);
    assert_eq!(
        notices,
        vec![
            RelayNotice::Items { user_id: bob },
            RelayNotice::QueueOverflow {
                user_id: bob,
                spaces: vec![
                    QueueSpace {
                        host_server_id: PEER.to_string(),
                        workspace_id: Uuid::from_u128(5),
                    },
                    QueueSpace {
                        host_server_id: "rims1_other".to_string(),
                        workspace_id: Uuid::from_u128(6),
                    },
                ],
            },
        ]
    );
}

/// A queue at its cap drops nothing, and a drop that frees nothing sends no overflow notice.
#[tokio::test]
async fn a_queue_at_its_cap_or_with_nothing_to_drop_raises_no_overflow() {
    for (queued, expected_drops) in [(QUEUE_CAP, vec![]), (QUEUE_CAP + 1, vec![1])] {
        let drops = Arc::new(Mutex::new(Vec::new()));
        let (svc, bob) = queueing_relay(
            peers_resolving(peer_keys(&host_signer(40), 0), None),
            queued,
            vec![],
            drops.clone(),
        );

        let notices = deliver_one(&svc).await;

        assert_eq!(*drops.lock().expect("lock"), expected_drops, "{queued}");
        assert_eq!(
            notices,
            vec![RelayNotice::Items { user_id: bob }],
            "{queued}"
        );
    }
}

/// An envelope signed at a key newer than the pinned head is checked against the host's keys
/// fetched again at that seq.
#[tokio::test]
async fn an_envelope_signed_at_a_newer_key_is_checked_against_refetched_keys() {
    let rotated = host_signer(41);
    let (svc, _) = queueing_relay(
        peers_resolving(peer_keys(&host_signer(40), 0), Some(peer_keys(&rotated, 1))),
        0,
        vec![],
        Arc::new(Mutex::new(Vec::new())),
    );

    let response = svc
        .deliver(PEER.to_string(), vec![host_item(&rotated, 1)])
        .await
        .expect("delivered");

    assert!(response.refused.is_empty(), "{:?}", response.refused);
}

/// An envelope at a newer key the host cannot confirm, or at a key older than the head and
/// its predecessor, is refused as invalid and not queued.
#[tokio::test]
async fn an_envelope_at_an_unconfirmed_or_retired_key_is_refused() {
    let cases = [
        // Newer than the pin, and the host is not pinned there.
        (peer_keys(&host_signer(40), 0), None, host_signer(41), 1),
        // Newer than the pin, and the refetched keys still stop below it.
        (
            peer_keys(&host_signer(40), 0),
            Some(peer_keys(&host_signer(40), 0)),
            host_signer(41),
            1,
        ),
        // Two keys behind the head.
        (peer_keys(&host_signer(42), 2), None, host_signer(40), 0),
    ];
    for (keys, refetched, signer, key_seq) in cases {
        let mut relay = MockRelayRepository::new();
        relay
            .expect_received_through()
            .returning(|_| Box::pin(async { Ok(0) }));
        relay
            .expect_advance_received_through()
            .returning(|_, _| Box::pin(async { Ok(()) }));
        relay.expect_insert_item().never();
        let svc = relay_service(
            RelayRepos {
                relay_repo: Arc::new(relay),
                ..empty_repos()
            },
            peers_resolving(keys, refetched),
            MockS2sClient::new(),
        );

        let response = svc
            .deliver(PEER.to_string(), vec![host_item(&signer, key_seq)])
            .await
            .expect("answered");

        assert_eq!(response.refused.len(), 1, "key seq {key_seq}");
        assert_eq!(response.refused[0].reason, "invalid_envelope");
        assert_eq!(response.acked_through, "1");
    }
}

// ── Contacts ─────────────────────────────────────────────────────────────────────────────

/// A caller cannot add their own principal as a contact; another principal is stored.
#[tokio::test]
async fn the_callers_own_principal_is_not_a_contact() {
    let caller = UserId::new();
    let own = TestIdentity::new(70).principal_id();
    let other = TestIdentity::new(71).principal_id();
    let stored = other.clone();
    let mut relay = MockRelayRepository::new();
    relay
        .expect_upsert_contact()
        .withf(move |_, principal, _| principal == stored)
        .times(1)
        .returning(|_, principal, relay_urls| {
            let contact = Contact {
                principal_id: principal.to_string(),
                relay_urls,
                added_at: Utc::now(),
            };
            Box::pin(async move { Ok(contact) })
        });
    let svc = relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            user_repo: Arc::new(crate::probation_fixtures::users(vec![
                crate::probation_fixtures::account(caller, Utc::now(), Some(&own)),
            ])),
            ..empty_repos()
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    );
    let relays = vec![PEER_ORIGIN.to_string()];

    let err = svc
        .add_contact(Actor::new(caller), own, relays.clone())
        .await
        .expect_err("the caller's own principal");
    assert!(
        matches!(&err, ServiceError::Validation(m) if m == "cannot add yourself as a contact"),
        "{err:?}"
    );
    let contact = svc
        .add_contact(Actor::new(caller), other.clone(), relays)
        .await
        .expect("another principal");
    assert_eq!(contact.principal_id, other);
}

// ── Identity fetches ─────────────────────────────────────────────────────────────────────

const SERVING_RELAY: &str = "rims1_serving_relay";

/// What an identity fetch gets back: a status and body, or a transport error.
type FetchAnswer = Result<(u16, Vec<u8>), S2sClientError>;

fn alice_document() -> IdentityDocument {
    let id = TestIdentity::new(1);
    let state = KeyState::from_inception(&id.inception()).expect("inception");
    document(&id, &state)
}

/// A client whose identity fetch of `principal_id` from `SERVING_RELAY` gets `answer`.
fn serving(principal_id: &str, answer: FetchAnswer) -> MockS2sClient {
    let path = format!("/principals/{principal_id}");
    let mut client = MockS2sClient::new();
    client
        .expect_send()
        .withf(move |request| {
            request.peer_server_id == SERVING_RELAY
                && request.path == path
                && request.method == rim_services::s2s::dto::S2sMethod::Get
        })
        .times(1)
        .returning(move |_| {
            let answer = answer.clone().map(|(status, body)| S2sResponse {
                status,
                content_type: "application/json".to_string(),
                body,
                key_seq: 0,
                headers: Vec::new(),
            });
            Box::pin(async move { answer })
        });
    client
}

/// A relay with no local identity for `principal_id`, holding `cached` for it; `stored`
/// counts the identities it stores.
fn identity_relay(
    principal_id: &str,
    cached: Option<CachedIdentity>,
    client: MockS2sClient,
    stored: Arc<Mutex<usize>>,
) -> DefaultRelayService {
    let principal_id = principal_id.to_string();
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(|_| Box::pin(async { Ok(None) }));
    let mut relay = MockRelayRepository::new();
    relay
        .expect_cached_identity()
        .withf(move |p| p == principal_id)
        .returning(move |_| {
            let cached = cached.clone();
            Box::pin(async move { Ok(cached) })
        });
    relay
        .expect_store_identity()
        .returning(move |_, served_by, _| {
            assert_eq!(served_by, SERVING_RELAY);
            *stored.lock().expect("lock") += 1;
            Box::pin(async { Ok(()) })
        });
    relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            ..empty_repos()
        },
        MockPeerService::new(),
        client,
    )
}

/// `document` cached from `SERVING_RELAY`, fetched `age` ago.
fn cached(document: &IdentityDocument, age: Duration) -> Option<CachedIdentity> {
    Some(CachedIdentity {
        principal_id: document.principal_id.clone(),
        served_by: SERVING_RELAY.to_string(),
        identity: serde_json::to_string(document).expect("json"),
        fetched_at: Utc::now() - age,
    })
}

fn actor() -> Actor {
    Actor::new(UserId::new())
}

/// A cached identity younger than a day is served without a fetch.
#[tokio::test]
async fn a_cached_identity_younger_than_a_day_is_served_as_is() {
    let doc = alice_document();
    let stored = Arc::new(Mutex::new(0));
    let mut client = MockS2sClient::new();
    client.expect_send().never();
    let svc = identity_relay(
        &doc.principal_id,
        cached(&doc, Duration::hours(23)),
        client,
        stored.clone(),
    );

    let identity = svc
        .remote_identity(actor(), doc.principal_id.clone(), None)
        .await
        .expect("served from the cache");

    assert_eq!(identity.document, doc);
    assert_eq!(identity.served_by, SERVING_RELAY);
    assert_eq!(*stored.lock().expect("lock"), 0);
}

/// An older cached identity is fetched again from the relay that served it, verified, and
/// stored.
#[tokio::test]
async fn a_stale_cached_identity_is_refetched_from_the_relay_that_served_it() {
    let doc = alice_document();
    let stored = Arc::new(Mutex::new(0));
    let body = serde_json::to_vec(&doc).expect("json");
    let svc = identity_relay(
        &doc.principal_id,
        cached(&doc, Duration::hours(25)),
        serving(&doc.principal_id, Ok((200, body))),
        stored.clone(),
    );

    let identity = svc
        .remote_identity(actor(), doc.principal_id.clone(), None)
        .await
        .expect("refetched");

    assert_eq!(identity.document, doc);
    assert_eq!(identity.served_by, SERVING_RELAY);
    assert_eq!(*stored.lock().expect("lock"), 1);
}

/// A refetch is refused as the serving relay being unavailable when the relay fails, answers
/// with something that is not an identity, serves another principal's identity, or serves
/// one whose signatures do not verify; a 404 is NotFound. Nothing is stored.
#[tokio::test]
async fn a_refetch_that_fails_its_checks_is_refused_and_not_stored() {
    let doc = alice_document();
    let other = {
        let id = TestIdentity::new(2);
        document(
            &id,
            &KeyState::from_inception(&id.inception()).expect("inception"),
        )
    };
    let mut tampered = doc.clone();
    tampered.devices[0].certificate.signature = doc.key_log[0].signature.clone();
    let json = |d: &IdentityDocument| serde_json::to_vec(d).expect("json");
    let cases: Vec<(&str, FetchAnswer)> = vec![
        ("transport error", Err(S2sClientError::UnknownPeer)),
        ("server error", Ok((500, Vec::new()))),
        ("not json", Ok((200, b"<html>".to_vec()))),
        ("another principal", Ok((200, json(&other)))),
        ("bad signature", Ok((200, json(&tampered)))),
    ];
    for (case, answer) in cases {
        let stored = Arc::new(Mutex::new(0));
        let svc = identity_relay(
            &doc.principal_id,
            cached(&doc, Duration::hours(25)),
            serving(&doc.principal_id, answer),
            stored.clone(),
        );

        let err = svc
            .remote_identity(actor(), doc.principal_id.clone(), None)
            .await
            .expect_err(case);

        assert!(
            matches!(
                &err,
                ServiceError::UpstreamUnavailable { host_server_id: Some(id), origin: None }
                    if id == SERVING_RELAY
            ),
            "{case}: {err:?}"
        );
        assert_eq!(*stored.lock().expect("lock"), 0, "{case}");
    }

    let svc = identity_relay(
        &doc.principal_id,
        cached(&doc, Duration::hours(25)),
        serving(&doc.principal_id, Ok((404, Vec::new()))),
        Arc::new(Mutex::new(0)),
    );
    let err = svc
        .remote_identity(actor(), doc.principal_id.clone(), None)
        .await
        .expect_err("not served");
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");
}

/// Without a cached identity there is no relay to ask, so a relay URL is required.
#[tokio::test]
async fn an_identity_never_cached_needs_a_relay_url() {
    let doc = alice_document();
    let mut client = MockS2sClient::new();
    client.expect_send().never();
    let svc = identity_relay(&doc.principal_id, None, client, Arc::new(Mutex::new(0)));

    let err = svc
        .remote_identity(actor(), doc.principal_id.clone(), None)
        .await
        .expect_err("no relay to ask");

    assert!(
        matches!(&err, ServiceError::Validation(m) if m == "relayUrl is required"),
        "{err:?}"
    );
}

// ── Remote servers ───────────────────────────────────────────────────────────────────────

/// Peers that pin any origin as `PEER` and hold `discovery` for it.
fn pinning_peers(discovery: Option<&str>) -> MockPeerService {
    let discovery = discovery.map(str::to_string);
    let mut peers = MockPeerService::new();
    peers.expect_pin().returning(|origin, _| {
        let pinned = PeerDto {
            server_id: PEER.to_string(),
            origin: origin.to_string(),
            status: "active".to_string(),
            head_seq: 0,
            descriptor_version: 1,
            descriptor_payload: "{}".to_string(),
            fetched_at: Utc::now(),
        };
        Box::pin(async move { Ok(pinned) })
    });
    peers
        .expect_discovery_document()
        .withf(|server_id| server_id == PEER)
        .returning(move |_| {
            let discovery = discovery.clone();
            Box::pin(async move { Ok(discovery) })
        });
    peers
}

/// A remote server is looked up by its bare origin, a trailing slash allowed, and answered
/// with its pinned id and discovery document.
#[tokio::test]
async fn a_remote_server_is_pinned_by_its_origin() {
    let svc = relay_service(
        empty_repos(),
        pinning_peers(Some(r#"{"serverId":"x"}"#)),
        MockS2sClient::new(),
    );

    let server = svc
        .remote_server(actor(), format!("{PEER_ORIGIN}/"))
        .await
        .expect("pinned");

    assert_eq!(server.server_id, PEER);
    assert_eq!(server.origin, PEER_ORIGIN);
    assert_eq!(server.discovery, r#"{"serverId":"x"}"#);
}

/// An origin with a path, query or a scheme other than http(s) is refused before pinning, and
/// a pinned server without a discovery document is NotFound.
#[tokio::test]
async fn a_remote_server_needs_a_bare_origin_and_a_discovery_document() {
    let mut peers = MockPeerService::new();
    peers.expect_pin().never();
    let svc = relay_service(empty_repos(), peers, MockS2sClient::new());
    for origin in [
        format!("{PEER_ORIGIN}/s2s"),
        format!("{PEER_ORIGIN}?x=1"),
        "ftp://relay.example".to_string(),
    ] {
        let err = svc
            .remote_server(actor(), origin.clone())
            .await
            .expect_err(&origin);
        assert!(
            matches!(&err, ServiceError::Validation(m) if m == "origin must be scheme://host[:port]"),
            "{origin}: {err:?}"
        );
    }

    let svc = relay_service(empty_repos(), pinning_peers(None), MockS2sClient::new());
    let err = svc
        .remote_server(actor(), PEER_ORIGIN.to_string())
        .await
        .expect_err("no discovery document");
    assert!(matches!(err, ServiceError::NotFound), "{err:?}");
}

// ── The host's checks of forwarded requests ─────────────────────────────────────────────

/// What the host's repositories answer for Bob, a remote user here.
#[derive(Clone, Copy)]
struct Facts {
    known: bool,
    revoked: bool,
    seq: SeqDecision,
    direct_member: bool,
    card_names_relay: bool,
    /// An operator blocked Bob on this server.
    server_blocked: bool,
}

const FACTS: Facts = Facts {
    known: true,
    revoked: false,
    seq: SeqDecision::Accepted,
    direct_member: true,
    card_names_relay: true,
    server_blocked: false,
};

fn host(facts: Facts, delegated: Option<bool>) -> (DefaultHostService, String) {
    let bob = UserId::new();
    let direct = ServerId::new();
    let shared = identity(40);
    let own_id = shared.current().server_id().to_string();
    let mut host = MockHostRepository::new();
    host.expect_find_user().returning(move |_| {
        Box::pin(async move { Ok(facts.known.then_some((bob, UserKind::Remote))) })
    });
    host.expect_check_request_seq()
        .returning(move |_, _, _, _| Box::pin(async move { Ok(facts.seq) }));
    let device_key_id = DeviceKeyId::new();
    let mut keys = MockKeyIdentityRepository::new();
    keys.expect_get_device().returning(move |user, _| {
        Box::pin(async move { Ok(Some(device(device_key_id, user, facts.revoked))) })
    });
    keys.expect_get_profile_card().returning(move |user| {
        let relay = if facts.card_names_relay {
            PEER_ORIGIN
        } else {
            "https://elsewhere.example"
        };
        Box::pin(async move {
            Ok(Some(StoredProfileCard {
                user_id: user,
                version: 1,
                payload: format!(r#"{{"relays":["{relay}"]}}"#),
                signature: "ed25519:c".to_string(),
                updated_at: Utc::now(),
            }))
        })
    });
    let mut members = MockServerMemberRepository::new();
    members
        .expect_get_member()
        .returning(move |server_id, user_id| {
            Ok(facts.direct_member.then(|| ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: ServerMemberRole::Guest,
                joined_at: Utc::now(),
            }))
        });
    let mut service = DefaultHostService::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(MockRelayRepository::new()),
            key_identity_repo: Arc::new(keys),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(members),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            relay_abuse_repo: Arc::new(blocks(if facts.server_blocked { &[BOB] } else { &[] })),
        },
        None,
        shared,
        Arc::new(MockPeerService::new()),
        Arc::new(MockS2sClient::new()),
        direct,
        120,
    );
    if let Some(delegated) = delegated {
        let mut check = MockDelegationCheck::new();
        check
            .expect_delegated()
            .returning(move |_, _, _| Box::pin(async move { Ok(delegated) }));
        service = service.with_delegations(Arc::new(check));
    }
    (service, own_id)
}

fn forwarded(host_id: &str, body: &[u8], sent: &[u8]) -> ForwardRequest {
    let payload = device_request_payload(
        BOB,
        Uuid::from_u128(1),
        host_id,
        "POST",
        "/api",
        1_791_504_000_000_000,
        body,
        "2026-10-09T00:00:00Z",
    );
    let signature = sign_with_context(
        Purpose::DeviceRequest.context(),
        &signing_key(20),
        payload.clone(),
    )
    .signature;
    ForwardRequest {
        peer_server_id: PEER.to_string(),
        peer_origin: PEER_ORIGIN.to_string(),
        device_request: Some(URL_SAFE_NO_PAD.encode(payload)),
        device_signature: Some(signature),
        method: "POST".to_string(),
        path: "/api".to_string(),
        body: sent.to_vec(),
    }
}

async fn check(facts: Facts, delegated: Option<bool>) -> Result<(), ForwardRefusal> {
    let (svc, own_id) = host(facts, delegated);
    let outcome = svc
        .authenticate_forward(forwarded(&own_id, b"{}", b"{}"))
        .await;
    assert!(outcome.request_payload.is_some());
    outcome.result.map(|_| ())
}

#[tokio::test]
async fn a_member_of_the_direct_space_whose_card_names_the_relay_is_accepted() {
    assert_eq!(check(FACTS, None).await, Ok(()));
}

#[tokio::test]
async fn unknown_users_and_revoked_devices_are_refused() {
    let unknown = check(
        Facts {
            known: false,
            ..FACTS
        },
        None,
    )
    .await;
    assert!(
        matches!(unknown, Err(ForwardRefusal::Forbidden(_))),
        "{unknown:?}"
    );
    let revoked = check(
        Facts {
            revoked: true,
            ..FACTS
        },
        None,
    )
    .await;
    assert!(
        matches!(revoked, Err(ForwardRefusal::Forbidden(_))),
        "{revoked:?}"
    );
}

#[tokio::test]
async fn a_principal_blocked_on_this_server_cannot_forward() {
    let blocked = check(
        Facts {
            server_blocked: true,
            ..FACTS
        },
        None,
    )
    .await;
    assert!(
        matches!(&blocked, Err(ForwardRefusal::Forbidden(why)) if why.contains("blocked")),
        "{blocked:?}"
    );
}

#[tokio::test]
async fn another_body_host_or_missing_headers_are_refused() {
    let (svc, own_id) = host(FACTS, None);
    let tampered = svc
        .authenticate_forward(forwarded(&own_id, b"{}", b"{\"x\":1}"))
        .await;
    assert!(matches!(tampered.result, Err(ForwardRefusal::Forbidden(_))));
    let elsewhere = svc
        .authenticate_forward(forwarded("rims1_another", b"{}", b"{}"))
        .await;
    assert!(matches!(
        elsewhere.result,
        Err(ForwardRefusal::Forbidden(_))
    ));
    let mut bare = forwarded(&own_id, b"{}", b"{}");
    bare.device_request = None;
    let outcome = svc.authenticate_forward(bare).await;
    assert!(outcome.request_payload.is_none());
    assert!(matches!(outcome.result, Err(ForwardRefusal::BadRequest(_))));
    let mut unsigned = forwarded(&own_id, b"{}", b"{}");
    unsigned.device_signature =
        Some(sign_with_context(Purpose::DeviceRequest.context(), &signing_key(21), "{}").signature);
    let outcome = svc.authenticate_forward(unsigned).await;
    assert!(matches!(outcome.result, Err(ForwardRefusal::Forbidden(_))));
}

#[tokio::test]
async fn a_seq_outside_the_window_is_a_replay() {
    let replay = check(
        Facts {
            seq: SeqDecision::Replay { highest: 42 },
            ..FACTS
        },
        None,
    )
    .await;
    assert_eq!(replay, Err(ForwardRefusal::Replay { highest: 42 }));
}

#[tokio::test]
async fn a_relay_needs_the_card_or_a_delegation() {
    let not_named = Facts {
        card_names_relay: false,
        ..FACTS
    };
    assert_eq!(
        check(not_named, None).await,
        Err(ForwardRefusal::NotDelegated)
    );
    let outside = Facts {
        direct_member: false,
        ..FACTS
    };
    assert_eq!(
        check(outside, None).await,
        Err(ForwardRefusal::NotDelegated)
    );
    assert_eq!(check(not_named, Some(true)).await, Ok(()));
    assert_eq!(
        check(not_named, Some(false)).await,
        Err(ForwardRefusal::NotDelegated)
    );
}

#[tokio::test]
async fn forwarded_requests_are_limited_per_device() {
    let (svc, own_id) = host(FACTS, None);
    for _ in 0..120 {
        let outcome = svc
            .authenticate_forward(forwarded(&own_id, b"{}", b"{}"))
            .await;
        assert_eq!(outcome.result.map(|_| ()), Ok(()));
    }
    let outcome = svc
        .authenticate_forward(forwarded(&own_id, b"{}", b"{}"))
        .await;
    assert!(matches!(
        outcome.result,
        Err(ForwardRefusal::RateLimited { .. })
    ));
}

// ── Probation of new accounts ────────────────────────────────────────────────────────────

/// A host with a one-hour probation, read at `now`, where `ada` holds `CONTACT_ID` as a
/// contact and shares a workspace with the holder of `COWORKER_ID`; `accounts` are the users
/// it knows.
fn probation_host(
    ada: UserId,
    accounts: Vec<rim_core::user::model::User>,
    now: chrono::DateTime<Utc>,
) -> DefaultHostService {
    let contact = TestIdentity::new(61).principal_id();
    let coworker = TestIdentity::new(62).principal_id();
    let mut relay = MockRelayRepository::new();
    relay.expect_is_contact().returning(move |user, principal| {
        let known = user == ada && principal == contact;
        Box::pin(async move { Ok(known) })
    });
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(|_| Box::pin(async { Ok(None) }));
    host.expect_shares_workspace()
        .returning(move |user, principal| {
            let shared = user == ada && principal == coworker;
            Box::pin(async move { Ok(shared) })
        });
    DefaultHostService::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(relay),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(crate::probation_fixtures::users(accounts)),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
        },
        None,
        identity(41),
        Arc::new(MockPeerService::new()),
        Arc::new(MockS2sClient::new()),
        ServerId::new(),
        120,
    )
    .with_limits(rim_services::limits::InstanceLimits {
        probation_hours: Some(1),
        ..Default::default()
    })
    .with_clock(crate::clock::Clock::fixed(now))
}

#[tokio::test]
async fn an_account_in_probation_sends_no_dm_request_to_a_stranger() {
    use crate::probation_fixtures::account;
    let created = Utc::now();
    let in_probation = created + Duration::minutes(20);
    let (ada, contact, coworker, stranger) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let stranger_principal = TestIdentity::new(63).principal_id();
    let accounts = vec![
        account(ada, created, Some(&TestIdentity::new(60).principal_id())),
        account(
            contact,
            created,
            Some(&TestIdentity::new(61).principal_id()),
        ),
        account(
            coworker,
            created,
            Some(&TestIdentity::new(62).principal_id()),
        ),
        account(stranger, created, Some(&stranger_principal)),
    ];
    let refusal = "a new account can send a DM request to a stranger in 40 minutes";
    let refused = |result: Result<(), ServiceError>| match result {
        Err(ServiceError::Validation(message)) => assert_eq!(message, refusal),
        other => panic!("expected the probation refusal, got {other:?}"),
    };

    let host = probation_host(ada, accounts.clone(), in_probation);
    host.ensure_may_request(Actor::new(ada), vec![ada, contact, coworker])
        .await
        .expect("contacts and coworkers are no strangers");
    refused(
        host.ensure_may_request(Actor::new(ada), vec![contact, stranger])
            .await,
    );
    refused(
        host.ensure_remote_member(
            Actor::new(ada),
            rim_services::host::dto::RemoteMemberRef {
                principal_id: TestIdentity::new(64).principal_id(),
                relay_urls: vec!["https://relay.example".to_string()],
            },
        )
        .await
        .map(|_| ()),
    );

    let later = probation_host(ada, accounts.clone(), created + Duration::hours(1));
    later
        .ensure_may_request(Actor::new(ada), vec![stranger])
        .await
        .expect("probation is over");
    let off = probation_host(ada, accounts, in_probation)
        .with_limits(rim_services::limits::InstanceLimits::default());
    off.ensure_may_request(Actor::new(ada), vec![stranger])
        .await
        .expect("no probation on this instance");
}
