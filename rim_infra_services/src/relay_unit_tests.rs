//! The relay's key package claims for hosts (the stranger rule, blocks, the claim's host) and
//! the host's checks of forwarded device requests (the device, its signature, the forwarded
//! body, the `seq` window, and which relay may carry a user's traffic).

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
use rim_core::relay::repository::{
    MockRelayAbuseRepository, MockRelayRepository, MockSubscriptionRepository,
};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::MockUserRepository;
use rim_identity::testing::signing_key;
use rim_identity::{PublicKey, Purpose, device_request_payload, sign_with_context};
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerIdentity,
    ServerKeyState, ServerSigner, SharedIdentity, sign_descriptor, sign_inception, timestamp,
};
use rim_services::ServiceError;
use rim_services::host::dto::{ForwardRefusal, ForwardRequest};
use rim_services::host::service::{HostService, MockDelegationCheck};
use rim_services::relay::dto::{ClaimConversation, Claimer, PeerClaimRequest};
use rim_services::relay::service::RelayService;
use rim_services::s2s::service::{MockPeerService, MockS2sClient};
use uuid::Uuid;

use crate::host::{DefaultHostService, HostRepos};
use crate::quota::Quotas;
use crate::relay::{DefaultRelayService, RelayRepos, RelaySettings};

const PEER: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
const PEER_ORIGIN: &str = "https://relay.example";
const BOB: &str = "rimp1_bob";
const ALICE: &str = "rimp1_alice";

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
    DefaultRelayService::new(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            key_identity_repo: Arc::new(keys),
            e2ee_repo: Arc::new(e2ee),
            user_repo: Arc::new(MockUserRepository::new()),
            subscription_repo: Arc::new(MockSubscriptionRepository::new()),
            relay_abuse_repo: Arc::new(blocks(server_blocks)),
        },
        None,
        identity(30),
        Arc::new(MockPeerService::new()),
        Arc::new(MockS2sClient::new()),
        RelaySettings {
            queue_bytes_per_user: 1 << 20,
            retention: Duration::days(30),
            quotas: Quotas::default(),
        },
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
