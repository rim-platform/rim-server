//! The relay's key package claims for hosts (the stranger rule, blocks, the claim's host), the
//! operator's DM settings at its claims, identities and request inbox, its held bytes, its
//! queue cap, oversized envelopes, items without a conversation and each peer's daily bytes,
//! contacts, identity fetches and remote server lookups, and the host's checks of forwarded
//! device requests (the device, its signature, the forwarded body, the `seq` window, and which
//! relay may carry a user's traffic).

use std::sync::{Arc, Mutex};

use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use chrono::{Duration, Utc};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::dm_policy::model::DmInbound;
use rim_core::dm_policy::repository::MockDmPolicyRepository;
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
use rim_services::dm_policy::dto::DmOperatorPolicy;
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
use crate::relay::identity::testing::{document, signed};
use crate::relay::{DefaultRelayService, QUEUED_BYTES_PER_PEER_PER_DAY, RelayRepos, RelaySettings};

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
        None,
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
        dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
    }
}

/// A relay over `repos` whose queue holds `QUEUE_CAP` bytes per user.
fn relay_service(
    repos: RelayRepos,
    peers: MockPeerService,
    client: MockS2sClient,
) -> DefaultRelayService {
    relay_with_peer_bytes(repos, peers, client, QUEUED_BYTES_PER_PEER_PER_DAY)
}

/// [`relay_service`] queueing `peer_bytes` from each peer per day.
fn relay_with_peer_bytes(
    repos: RelayRepos,
    peers: MockPeerService,
    client: MockS2sClient,
    peer_bytes: u64,
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
            queued_bytes_per_peer_per_day: peer_bytes,
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
    relay_service(
        claim_repos(contact, blocked, one_time, server_blocks),
        MockPeerService::new(),
        MockS2sClient::new(),
    )
}

/// The repositories of [`relay_with_blocks`].
fn claim_repos(
    contact: bool,
    blocked: bool,
    one_time: Arc<Mutex<Vec<bool>>>,
    server_blocks: &[&str],
) -> RelayRepos {
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
    RelayRepos {
        relay_repo: Arc::new(relay),
        host_repo: Arc::new(host),
        key_identity_repo: Arc::new(keys),
        e2ee_repo: Arc::new(e2ee),
        relay_abuse_repo: Arc::new(blocks(server_blocks)),
        ..empty_repos()
    }
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

// ── The operator's DM settings at the relay ──────────────────────────────────────────────

const OTHER: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

fn inbound(inbound: DmInbound, partners: &[&str]) -> DmOperatorPolicy {
    DmOperatorPolicy {
        inbound,
        partners: partners.iter().map(|p| p.to_string()).collect(),
        ..Default::default()
    }
}

/// Subscriptions where the user holds an active one at `hosts` only.
fn subscribed_at(hosts: &[&str]) -> MockSubscriptionRepository {
    let hosts: Vec<String> = hosts.iter().map(|h| h.to_string()).collect();
    let mut subscriptions = MockSubscriptionRepository::new();
    subscriptions
        .expect_subscribed_hosts()
        .returning(move |_, _| {
            let hosts = hosts.clone();
            Box::pin(async move { Ok(hosts) })
        });
    subscriptions
}

/// A claiming relay under `policy` whose user is subscribed at `hosts`.
fn gated_relay(policy: DmOperatorPolicy, hosts: &[&str]) -> DefaultRelayService {
    relay_service(
        RelayRepos {
            subscription_repo: Arc::new(subscribed_at(hosts)),
            ..claim_repos(false, false, Arc::default(), &[])
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    )
    .with_dm_policy(policy)
}

#[tokio::test]
async fn a_host_the_operator_does_not_admit_claims_only_where_the_user_is_subscribed() {
    for policy in [
        inbound(DmInbound::None, &[]),
        inbound(DmInbound::Partners, &[OTHER]),
    ] {
        let err = gated_relay(policy.clone(), &[OTHER])
            .claim_for_peer(PEER.to_string(), claim(PEER))
            .await
            .expect_err("not admitted");
        assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
        gated_relay(policy, &[PEER])
            .claim_for_peer(PEER.to_string(), claim(PEER))
            .await
            .expect("the user's own host");
    }
    gated_relay(inbound(DmInbound::Partners, &[PEER]), &[])
        .claim_for_peer(PEER.to_string(), claim(PEER))
        .await
        .expect("a partner");
}

#[tokio::test]
async fn a_server_the_operator_does_not_admit_reads_no_identity() {
    let svc = gated_relay(inbound(DmInbound::Partners, &[OTHER]), &[]);
    assert_eq!(
        svc.identity_for_peer(PEER.to_string(), BOB.to_string())
            .await
            .expect("answered"),
        None
    );
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(|_| Box::pin(async { Ok(None) }));
    let unknown = relay_service(
        RelayRepos {
            host_repo: Arc::new(host),
            ..empty_repos()
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    );
    assert_eq!(
        unknown
            .identity_for_peer(PEER.to_string(), BOB.to_string())
            .await
            .expect("answered"),
        None,
        "nobody local holds the principal"
    );
}

/// Alice, who sends Bob invites: her identity and the head of her key log.
fn sender() -> (TestIdentity, KeyState) {
    let id = TestIdentity::new(1);
    let state = KeyState::from_inception(&id.inception()).expect("inception");
    (id, state)
}

fn sender_id() -> String {
    sender().0.principal_id()
}

/// A card of `id` at `version`, listing `relays`.
fn card_of(id: &TestIdentity, version: u64, relays: &[&str]) -> SignedJson {
    let state = KeyState::from_inception(&id.inception()).expect("inception");
    signed(&id.profile_card(&state, version, "Alice", relays, &[]))
}

/// Alice's card at `version`, listing `relays`.
fn sender_card(version: u64, relays: &[&str]) -> SignedJson {
    card_of(&sender().0, version, relays)
}

/// Alice's identity as a relay serves it, with `card`.
fn sender_document(card: Option<SignedJson>) -> IdentityDocument {
    let (id, _) = sender();
    IdentityDocument {
        principal_id: id.principal_id(),
        key_log: vec![signed(&id.inception())],
        devices: Vec::new(),
        card,
        username: None,
    }
}

/// A `DmInvite` from `PEER` to Bob naming `from` and carrying `card`, signed by `signer`.
fn invite_item(signer: &ServerSigner, from: &str, card: serde_json::Value) -> DeliverItem {
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let event = serde_json::json!({
        "type": "DmInvite",
        "workspace_id": workspace_id,
        "conversation_id": Uuid::from_u128(9),
        "from_principal_id": from,
        "from_card": card,
        "member_principal_ids": [from, BOB],
    })
    .to_string();
    let stream = format!("u:{BOB}");
    let statement = sign_host_event(
        signer,
        &HostEventFields {
            host: PEER,
            key_seq: 0,
            workspace_id: &workspace_id,
            stream: &stream,
            seq: Some(1),
            event_json: &event,
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

/// Alice's invite to Bob from `PEER`, carrying `card`.
fn alice_invite(card: &SignedJson) -> DeliverItem {
    invite_item(
        &host_signer(40),
        &sender_id(),
        serde_json::to_value(card).expect("json"),
    )
}

/// What Bob's relay holds about Alice when her invite arrives from `PEER`.
#[derive(Clone)]
struct RelayInbox {
    policy: DmOperatorPolicy,
    /// How Alice is held here, with her stored card; she is unknown when `None`.
    held: Option<(UserKind, Option<SignedJson>)>,
    /// Alice is Bob's contact.
    contact: bool,
    cached: Option<CachedIdentity>,
    /// What `PEER` answers when asked for Alice's identity; asking fails the test when `None`.
    served: Option<FetchAnswer>,
    /// Requests `PEER` filed here in the last day.
    by_host_today: i64,
    /// Hosts where Alice, when local, holds an active subscription.
    subscribed_at: Vec<&'static str>,
    /// Bob's choice for DMs from outside his workspaces.
    outside: rim_core::dm_policy::model::OutsideDm,
    /// Bob's pending requests.
    pending: i64,
    /// Requests Alice filed here in the last day.
    filed_today: i64,
    /// Bytes the items of Bob's pending requests take.
    held_bytes: i64,
    /// Bob's accepted conversations from `PEER`.
    accepted: i64,
    /// Every row filed, as filed.
    rows: Arc<Mutex<Vec<rim_core::relay::model::NewDmRequest>>>,
}

impl Default for RelayInbox {
    /// Alice unknown here, a stranger to Bob with default settings, whose identity `PEER`
    /// serves with a card naming it.
    fn default() -> Self {
        Self {
            policy: DmOperatorPolicy::default(),
            held: None,
            contact: false,
            cached: None,
            served: Some(Ok((
                200,
                serde_json::to_vec(&sender_document(Some(sender_card(1, &[PEER_ORIGIN]))))
                    .expect("json"),
            ))),
            by_host_today: 0,
            subscribed_at: Vec::new(),
            outside: rim_core::dm_policy::model::OutsideDm::Requests,
            pending: 0,
            filed_today: 0,
            held_bytes: 0,
            accepted: 0,
            rows: Arc::default(),
        }
    }
}

/// Bob's relay as `inbox` says; `filed` records the status of each request it files.
fn inbox_relay(
    inbox: RelayInbox,
    filed: Arc<Mutex<Vec<rim_core::relay::model::DmRequestStatus>>>,
) -> DefaultRelayService {
    use rim_core::key_identity::model::{KeyLogEntry, KeyLogEntryType};
    use rim_core::relay::model::{DmRequest, DmRequestId};

    let bob = UserId::new();
    let alice = UserId::new();
    let alice_principal = sender_id();
    let held = inbox.held.clone();
    let mut host = MockHostRepository::new();
    host.expect_find_user().returning(move |principal| {
        let found = if principal == BOB {
            Some((bob, UserKind::Local))
        } else if principal == alice_principal {
            held.as_ref().map(|(kind, _)| (alice, *kind))
        } else {
            None
        };
        Box::pin(async move { Ok(found) })
    });
    let mut keys = MockKeyIdentityRepository::new();
    keys.expect_list_key_log().returning(move |user_id| {
        let inception = sender().0.inception();
        let log = vec![KeyLogEntry {
            user_id,
            seq: 0,
            entry_type: KeyLogEntryType::Inception,
            payload: inception.payload,
            signature: inception.signature,
            created_at: Utc::now(),
        }];
        Box::pin(async move { Ok(log) })
    });
    let stored = inbox.held.and_then(|(_, card)| card);
    keys.expect_get_profile_card().returning(move |user_id| {
        let card = stored.clone().map(|card| {
            let version =
                serde_json::from_str::<serde_json::Value>(&card.payload).expect("card")["version"]
                    .as_u64()
                    .expect("version");
            StoredProfileCard {
                user_id,
                version: u32::try_from(version).expect("version"),
                payload: card.payload,
                signature: card.signature,
                updated_at: Utc::now(),
            }
        });
        Box::pin(async move { Ok(card) })
    });
    let mut subscriptions = MockSubscriptionRepository::new();
    subscriptions
        .expect_is_subscribed()
        .returning(|_, _, _, _| Box::pin(async { Ok(false) }));
    let subscribed_at: Vec<String> = inbox.subscribed_at.iter().map(|h| h.to_string()).collect();
    subscriptions
        .expect_subscribed_hosts()
        .returning(move |user_id, _| {
            assert_eq!(user_id, alice, "only the sender's subscriptions are read");
            let hosts = subscribed_at.clone();
            Box::pin(async move { Ok(hosts) })
        });
    let mut relay = MockRelayRepository::new();
    relay
        .expect_received_through()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay
        .expect_advance_received_through()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    relay
        .expect_find_request()
        .returning(|_, _, _| Box::pin(async { Ok(None) }));
    relay
        .expect_is_blocked()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let contact = inbox.contact;
    relay
        .expect_is_contact()
        .returning(move |_, _| Box::pin(async move { Ok(contact) }));
    relay
        .expect_accepted_from()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let pending = inbox.pending;
    relay
        .expect_count_pending_requests()
        .returning(move |_| Box::pin(async move { Ok(pending) }));
    let filed_today = inbox.filed_today;
    relay
        .expect_count_requests_from()
        .returning(move |_, _| Box::pin(async move { Ok(filed_today) }));
    let held_bytes = inbox.held_bytes;
    relay
        .expect_user_held_bytes()
        .returning(move |_| Box::pin(async move { Ok(held_bytes) }));
    let by_host_today = inbox.by_host_today;
    relay
        .expect_count_requests_via_host()
        .withf(|host, _| host == PEER)
        .returning(move |_, _| Box::pin(async move { Ok(by_host_today) }));
    let cached = inbox.cached.clone();
    relay.expect_cached_identity().returning(move |_| {
        let cached = cached.clone();
        Box::pin(async move { Ok(cached) })
    });
    relay.expect_store_identity().never();
    let accepted = inbox.accepted;
    relay
        .expect_count_accepted_requests()
        .withf(|_, host| host == PEER)
        .returning(move |_, _| Box::pin(async move { Ok(accepted) }));
    let rows = Arc::clone(&inbox.rows);
    relay.expect_insert_request().returning(move |new| {
        filed.lock().expect("lock").push(new.status);
        rows.lock().expect("lock").push(new.clone());
        Box::pin(async move {
            Ok(DmRequest {
                id: DmRequestId::new(),
                user_id: new.user_id,
                from_principal_id: new.from_principal_id,
                card_payload: new.card_payload,
                card_signature: new.card_signature,
                host_server_id: new.host_server_id,
                conversation_id: new.conversation_id,
                status: new.status,
                created_at: Utc::now(),
                decided_at: None,
                expires_at: new.expires_at,
                host_user_id: None,
            })
        })
    });
    relay
        .expect_insert_envelope()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_insert_item()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_queued_bytes()
        .returning(|_| Box::pin(async { Ok(0) }));
    let mut dm = MockDmPolicyRepository::new();
    dm.expect_shared_workspaces()
        .returning(|_, _| Box::pin(async { Ok(Vec::new()) }));
    dm.expect_user_workspaces()
        .returning(|_| Box::pin(async { Ok(Vec::new()) }));
    let outside = inbox.outside;
    dm.expect_outside()
        .returning(move |_| Box::pin(async move { Ok(outside) }));
    let mut client = MockS2sClient::new();
    let path = format!("/principals/{}", sender_id());
    match inbox.served {
        Some(answer) => {
            client
                .expect_send()
                .withf(move |request| request.peer_server_id == PEER && request.path == path)
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
        }
        None => {
            client.expect_send().never();
        }
    }
    let signer = host_signer(40);
    relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            key_identity_repo: Arc::new(keys),
            subscription_repo: Arc::new(subscriptions),
            dm_policy_repo: Arc::new(dm),
            ..empty_repos()
        },
        peers_resolving(peer_keys(&signer, 0), None),
        client,
    )
    .with_dm_policy(inbox.policy)
}

/// What Bob's relay as `inbox` says does with `item` from `PEER`: the refusal reasons and the
/// requests filed.
async fn file_invite(
    inbox: RelayInbox,
    item: DeliverItem,
) -> (Vec<String>, Vec<rim_core::relay::model::DmRequestStatus>) {
    let filed = Arc::default();
    let svc = inbox_relay(inbox, Arc::clone(&filed));
    let response = svc
        .deliver(PEER.to_string(), vec![item])
        .await
        .expect("delivered");
    let refused = response.refused.into_iter().map(|r| r.reason).collect();
    let filed = filed.lock().expect("lock").clone();
    (refused, filed)
}

/// Refused as `reason`, with nothing filed.
fn refused_as(reason: &str) -> (Vec<String>, Vec<rim_core::relay::model::DmRequestStatus>) {
    (vec![reason.to_string()], Vec::new())
}

fn filed_as(
    status: rim_core::relay::model::DmRequestStatus,
) -> (Vec<String>, Vec<rim_core::relay::model::DmRequestStatus>) {
    (Vec::new(), vec![status])
}

/// An invite that would take Bob's held bytes past their cap is refused as `limited` and files
/// no request; one that fits files a request.
#[tokio::test]
async fn an_invite_past_the_recipients_held_bytes_is_refused_as_limited() {
    use rim_core::relay::model::DmRequestStatus;

    use crate::relay::inbox::MAX_HELD_BYTES_PER_USER;

    let invite = alice_invite(&sender_card(1, &[PEER_ORIGIN]));
    let size = i64::try_from(invite.envelope.payload.len() + invite.envelope.signature.len())
        .expect("size");
    let holding = |held_bytes| RelayInbox {
        held_bytes,
        ..RelayInbox::default()
    };
    assert_eq!(
        file_invite(holding(MAX_HELD_BYTES_PER_USER - size), invite.clone()).await,
        filed_as(DmRequestStatus::Pending)
    );
    assert_eq!(
        file_invite(holding(MAX_HELD_BYTES_PER_USER - size + 1), invite).await,
        refused_as("limited")
    );
}

#[tokio::test]
async fn an_invite_from_a_server_the_operator_admits_files_a_request() {
    use rim_core::relay::model::DmRequestStatus;
    for policy in [
        DmOperatorPolicy::default(),
        inbound(DmInbound::Open, &[]),
        inbound(DmInbound::Partners, &[PEER]),
    ] {
        let inbox = RelayInbox {
            policy,
            ..RelayInbox::default()
        };
        assert_eq!(
            file_invite(inbox, alice_invite(&sender_card(1, &[PEER_ORIGIN]))).await,
            filed_as(DmRequestStatus::Pending)
        );
    }
}

#[tokio::test]
async fn an_invite_from_a_server_the_operator_refuses_is_refused_as_blocked() {
    for policy in [
        inbound(DmInbound::None, &[PEER]),
        inbound(DmInbound::Partners, &[OTHER]),
    ] {
        let inbox = RelayInbox {
            policy,
            ..RelayInbox::default()
        };
        assert_eq!(
            file_invite(inbox, alice_invite(&sender_card(1, &[PEER_ORIGIN]))).await,
            refused_as("blocked")
        );
    }
}

/// An invite whose card is not the named sender's, signed by their key, is refused and filed
/// nowhere, even when the sender it names is Bob's contact.
#[tokio::test]
async fn an_invite_whose_card_does_not_verify_for_its_sender_is_refused() {
    let alice = sender_card(1, &[PEER_ORIGIN]);
    let mallory = TestIdentity::new(2);
    let mallorys_card = card_of(&mallory, 1, &[PEER_ORIGIN]);
    let cases = [
        (
            "a card signed by another key",
            serde_json::to_value(SignedJson {
                payload: alice.payload.clone(),
                signature: mallorys_card.signature.clone(),
            })
            .expect("json"),
        ),
        (
            "another principal's card",
            serde_json::to_value(&mallorys_card).expect("json"),
        ),
        (
            "an unsigned card",
            serde_json::json!({ "payload": "{}", "signature": "ed25519:c" }),
        ),
        ("no card", serde_json::Value::Null),
    ];
    for contact in [false, true] {
        for (what, card) in cases.clone() {
            let inbox = RelayInbox {
                contact,
                served: (!card.is_null())
                    .then(|| RelayInbox::default().served)
                    .flatten(),
                ..RelayInbox::default()
            };
            assert_eq!(
                file_invite(inbox, invite_item(&host_signer(40), &sender_id(), card)).await,
                refused_as("invalid_invite"),
                "{what}, contact {contact}"
            );
        }
    }
    let served = RelayInbox {
        served: None,
        ..RelayInbox::default()
    };
    assert_eq!(
        file_invite(
            served,
            invite_item(
                &host_signer(40),
                "rimp1_alice",
                serde_json::to_value(&alice).expect("json"),
            ),
        )
        .await,
        refused_as("invalid_invite"),
        "a sender that is no principal id"
    );
}

/// What `PEER` may answer when it serves no usable identity for Alice: not found, a
/// transport failure, or a document that does not verify.
fn unserved() -> Vec<FetchAnswer> {
    vec![
        Ok((404, Vec::new())),
        Err(S2sClientError::Transport("down".to_string())),
        Ok((
            200,
            serde_json::to_vec(&IdentityDocument {
                principal_id: sender_id(),
                ..alice_document_of(&TestIdentity::new(2))
            })
            .expect("json"),
        )),
    ]
}

/// An invite from a sender held and cached nowhere here, whose identity the host does not
/// serve, cannot be checked: it is filed as a stranger's request, whatever standing the name
/// it carries has with Bob, and is refused where a stranger's would be.
#[tokio::test]
async fn an_invite_whose_sender_cannot_be_verified_is_a_strangers_request() {
    use rim_core::dm_policy::model::OutsideDm;
    use rim_core::relay::model::DmRequestStatus;
    let card = sender_card(1, &[PEER_ORIGIN]);
    for answer in unserved() {
        for contact in [false, true] {
            let inbox = RelayInbox {
                served: Some(answer.clone()),
                contact,
                ..RelayInbox::default()
            };
            assert_eq!(
                file_invite(inbox, alice_invite(&card)).await,
                filed_as(DmRequestStatus::Pending),
                "{answer:?}, contact {contact}"
            );
        }
        let closed = RelayInbox {
            served: Some(answer.clone()),
            contact: true,
            outside: OutsideDm::None,
            ..RelayInbox::default()
        };
        assert_eq!(
            file_invite(closed, alice_invite(&card)).await,
            refused_as("blocked"),
            "{answer:?}: Bob takes no DMs from strangers"
        );
        let open = RelayInbox {
            served: Some(answer.clone()),
            contact: true,
            policy: inbound(DmInbound::Open, &[]),
            ..RelayInbox::default()
        };
        assert_eq!(
            file_invite(open, alice_invite(&card)).await,
            filed_as(DmRequestStatus::Pending),
            "{answer:?}: an open server still files a request"
        );
    }
}

/// What Bob's relay does with this server's own invite from Alice, a local user, with the
/// request inbox's counts at `counts`: the requests filed and the notices raised.
async fn file_own_invite(
    counts: (i64, i64),
) -> (Vec<rim_core::relay::model::DmRequestStatus>, usize) {
    let filed = Arc::default();
    let inbox = RelayInbox {
        pending: counts.0,
        filed_today: counts.1,
        served: None,
        ..RelayInbox::default()
    };
    let svc = inbox_relay(inbox, Arc::clone(&filed));
    let mut notices = svc.subscribe();
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let event = serde_json::json!({
        "type": "DmInvite",
        "workspace_id": workspace_id,
        "conversation_id": Uuid::from_u128(9),
        "from_principal_id": ALICE,
        "from_card": { "payload": "{}", "signature": "ed25519:c" },
        "member_principal_ids": [ALICE, BOB],
    })
    .to_string();
    let stream = format!("u:{BOB}");
    let statement = sign_host_event(
        &ServerSigner::from_seed([30; 32]),
        &HostEventFields {
            host: PEER,
            key_seq: 0,
            workspace_id: &workspace_id,
            stream: &stream,
            seq: Some(1),
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    svc.file_local(
        SignedJson {
            payload: statement.payload,
            signature: statement.signature,
        },
        vec![BOB.to_string()],
    )
    .await
    .expect("filed");
    let mut raised = 0;
    while notices.try_recv().is_ok() {
        raised += 1;
    }
    let filed = filed.lock().expect("lock").clone();
    (filed, raised)
}

#[tokio::test]
async fn an_own_invite_past_a_limit_is_kept_as_ignored_and_raises_no_notice() {
    use crate::relay::inbox::MAX_PENDING_PER_USER;
    use rim_core::relay::model::DmRequestStatus;
    let daily = i64::from(Quotas::default().dm_requests_per_day);
    assert_eq!(
        file_own_invite((0, 0)).await,
        (vec![DmRequestStatus::Pending], 2),
        "within the limits it is a request, held with its invite"
    );
    for counts in [(MAX_PENDING_PER_USER, 0), (0, daily)] {
        assert_eq!(
            file_own_invite(counts).await,
            (vec![DmRequestStatus::Ignored], 0),
            "past {counts:?} the conversation stays hidden"
        );
    }
}

/// An unverified sender's request counts against the host's daily requests and Bob's pending
/// ones; past the host's, its identity is not even fetched.
#[tokio::test]
async fn an_unverified_senders_request_is_counted_per_host_and_per_recipient() {
    use crate::relay::inbox::{MAX_PENDING_PER_USER, MAX_REQUESTS_PER_HOST_PER_DAY};
    use rim_core::relay::model::DmRequestStatus;
    let card = sender_card(1, &[PEER_ORIGIN]);
    let unserved = || Some(Ok((404, Vec::new())));
    assert_eq!(
        file_invite(
            RelayInbox {
                served: unserved(),
                by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY - 1,
                ..RelayInbox::default()
            },
            alice_invite(&card)
        )
        .await,
        filed_as(DmRequestStatus::Pending)
    );
    assert_eq!(
        file_invite(
            RelayInbox {
                served: None,
                by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY,
                ..RelayInbox::default()
            },
            alice_invite(&card)
        )
        .await,
        refused_as("limited"),
        "past the host's limit"
    );
    assert_eq!(
        file_invite(
            RelayInbox {
                served: unserved(),
                pending: MAX_PENDING_PER_USER,
                ..RelayInbox::default()
            },
            alice_invite(&card)
        )
        .await,
        refused_as("limited"),
        "past Bob's pending requests"
    );
}

/// A sender held here whose card fails against the held log is refused when the host serves
/// no newer identity for them.
#[tokio::test]
async fn a_card_failing_the_held_log_is_refused_when_the_host_serves_none() {
    let mallory = card_of(&TestIdentity::new(2), 1, &[PEER_ORIGIN]);
    let forged = SignedJson {
        payload: sender_card(1, &[PEER_ORIGIN]).payload,
        signature: mallory.signature,
    };
    for kind in [UserKind::Remote, UserKind::Local] {
        let fetched = kind == UserKind::Remote;
        for answer in unserved() {
            let inbox = RelayInbox {
                held: Some((kind, Some(sender_card(1, &[PEER_ORIGIN])))),
                contact: true,
                served: fetched.then(|| answer.clone()),
                ..RelayInbox::default()
            };
            assert_eq!(
                file_invite(inbox, alice_invite(&forged)).await,
                refused_as("invalid_invite"),
                "{kind:?}, {answer:?}"
            );
        }
    }
}

/// Another identity's document, served under its own principal id.
fn alice_document_of(id: &TestIdentity) -> IdentityDocument {
    IdentityDocument {
        principal_id: id.principal_id(),
        key_log: vec![signed(&id.inception())],
        devices: Vec::new(),
        card: None,
        username: None,
    }
}

/// A host the sender's newest card does not list is refused, whether the sender is unknown
/// here, held as a remote user with a newer card, or local; the invite's own older card
/// listing the host does not help.
#[tokio::test]
async fn a_host_the_senders_newest_card_does_not_name_is_refused() {
    const ELSEWHERE: &str = "https://elsewhere.example";
    let cases = [
        (
            "unknown sender",
            RelayInbox {
                served: Some(Ok((
                    200,
                    serde_json::to_vec(&sender_document(Some(sender_card(1, &[ELSEWHERE]))))
                        .expect("json"),
                ))),
                ..RelayInbox::default()
            },
            sender_card(1, &[ELSEWHERE]),
        ),
        (
            "a newer card served",
            RelayInbox {
                served: Some(Ok((
                    200,
                    serde_json::to_vec(&sender_document(Some(sender_card(2, &[ELSEWHERE]))))
                        .expect("json"),
                ))),
                ..RelayInbox::default()
            },
            sender_card(1, &[PEER_ORIGIN]),
        ),
        (
            "a newer card stored",
            RelayInbox {
                held: Some((UserKind::Remote, Some(sender_card(2, &[ELSEWHERE])))),
                served: None,
                contact: true,
                ..RelayInbox::default()
            },
            sender_card(1, &[PEER_ORIGIN]),
        ),
        (
            "a local sender",
            RelayInbox {
                held: Some((UserKind::Local, Some(sender_card(1, &[ELSEWHERE])))),
                served: None,
                contact: true,
                subscribed_at: vec![OTHER],
                ..RelayInbox::default()
            },
            sender_card(1, &[ELSEWHERE]),
        ),
    ];
    for (what, inbox, card) in cases {
        assert_eq!(
            file_invite(inbox, alice_invite(&card)).await,
            refused_as("invalid_invite"),
            "{what}"
        );
    }
}

/// A valid invite is filed as before: from a stranger as a request, from a contact as
/// accepted, checked against the identity held or cached here without a fetch; a local sender
/// subscribed at the host is spoken for by it.
#[tokio::test]
async fn a_valid_invite_is_filed_by_the_policy() {
    use rim_core::relay::model::DmRequestStatus;
    let card = sender_card(1, &[PEER_ORIGIN]);
    let cases = [
        (
            "held as remote, a contact",
            RelayInbox {
                held: Some((UserKind::Remote, Some(card.clone()))),
                contact: true,
                served: None,
                ..RelayInbox::default()
            },
            DmRequestStatus::Accepted,
        ),
        (
            "cached",
            RelayInbox {
                cached: cached(&sender_document(Some(card.clone())), Duration::hours(1)),
                served: None,
                ..RelayInbox::default()
            },
            DmRequestStatus::Pending,
        ),
        (
            "local, subscribed at the host",
            RelayInbox {
                held: Some((
                    UserKind::Local,
                    Some(sender_card(1, &["https://elsewhere.example"])),
                )),
                served: None,
                subscribed_at: vec![PEER],
                ..RelayInbox::default()
            },
            DmRequestStatus::Pending,
        ),
    ];
    for (what, inbox, status) in cases {
        assert_eq!(
            file_invite(inbox, alice_invite(&card)).await,
            filed_as(status),
            "{what}"
        );
    }
    let local = sender_card(1, &["https://elsewhere.example"]);
    assert_eq!(
        file_invite(
            RelayInbox {
                held: Some((UserKind::Local, Some(local.clone()))),
                served: None,
                subscribed_at: vec![PEER],
                ..RelayInbox::default()
            },
            alice_invite(&local),
        )
        .await,
        filed_as(DmRequestStatus::Pending),
        "a local sender's card need not name a host they are subscribed at"
    );
}

/// A host past its daily requests is refused `limited` whichever principal it names, without
/// fetching an identity; an invite the policy delivers is still recorded.
#[tokio::test]
async fn a_host_past_its_daily_requests_is_limited() {
    use crate::relay::inbox::MAX_REQUESTS_PER_HOST_PER_DAY;
    use rim_core::relay::model::DmRequestStatus;
    let card = sender_card(1, &[PEER_ORIGIN]);
    let past = |inbox: RelayInbox| RelayInbox {
        by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY,
        served: None,
        ..inbox
    };
    assert_eq!(
        file_invite(past(RelayInbox::default()), alice_invite(&card)).await,
        refused_as("limited"),
        "unknown sender"
    );
    let held = RelayInbox {
        held: Some((UserKind::Remote, Some(card.clone()))),
        ..RelayInbox::default()
    };
    assert_eq!(
        file_invite(past(held.clone()), alice_invite(&card)).await,
        refused_as("limited"),
        "held sender"
    );
    assert_eq!(
        file_invite(
            past(RelayInbox {
                contact: true,
                ..held
            }),
            alice_invite(&card)
        )
        .await,
        filed_as(DmRequestStatus::Accepted),
        "a contact"
    );
    assert_eq!(
        file_invite(
            RelayInbox {
                by_host_today: MAX_REQUESTS_PER_HOST_PER_DAY - 1,
                ..RelayInbox::default()
            },
            alice_invite(&card)
        )
        .await,
        filed_as(DmRequestStatus::Pending),
        "below the limit"
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
        removed_items: 1,
        host_server_id: host.to_string(),
        workspace_id: Uuid::from_u128(workspace),
    }
}

/// A queue over its cap drops the overflow from the oldest items and names the spaces it
/// dropped from in one notice.
#[tokio::test]
async fn a_queue_over_its_cap_drops_the_overflow_and_names_the_spaces() {
    let recorder = metrics_util::debugging::DebuggingRecorder::new();
    let snapshot = recorder.snapshotter();
    let _guard = metrics::set_default_local_recorder(&recorder);
    let drops = Arc::new(Mutex::new(Vec::new()));
    let (svc, bob) = queueing_relay(
        peers_resolving(peer_keys(&host_signer(40), 0), None),
        QUEUE_CAP + 300,
        vec![dropped_space(PEER, 5), dropped_space("rims1_other", 6)],
        drops.clone(),
    );

    let notices = deliver_one(&svc).await;

    assert_eq!(*drops.lock().expect("lock"), vec![300]);
    let values = snapshot.snapshot().into_vec();
    for (name, count) in [
        ("rim_relay_enqueued_items_total", 1),
        ("rim_relay_removed_items_total", 2),
    ] {
        assert!(
            values
                .iter()
                .any(|(key, _, _, value)| key.key().name() == name
                    && *value == metrics_util::debugging::DebugValue::Counter(count)),
            "missing {name}"
        );
    }

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

/// An envelope whose payload is over a host event's 512 KiB is refused as invalid and not
/// queued, whatever its signature.
#[tokio::test]
async fn an_envelope_over_the_host_event_size_is_refused() {
    let signer = host_signer(40);
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let padding = "x".repeat(rim_s2s::ServerStatement::HostEvent.max_payload_len());
    let event = serde_json::json!({ "type": "MessageCreated", "padding": padding }).to_string();
    let statement = sign_host_event(
        &signer,
        &HostEventFields {
            host: PEER,
            key_seq: 0,
            workspace_id: &workspace_id,
            stream: "e",
            seq: None,
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    let mut relay = MockRelayRepository::new();
    relay
        .expect_received_through()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay
        .expect_advance_received_through()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    relay.expect_insert_envelope().never();
    relay.expect_insert_item().never();
    let svc = relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            ..empty_repos()
        },
        peers_resolving(peer_keys(&signer, 0), None),
        MockS2sClient::new(),
    );

    let response = svc
        .deliver(
            PEER.to_string(),
            vec![DeliverItem {
                id: "1".to_string(),
                envelope: SignedJson {
                    payload: statement.payload,
                    signature: statement.signature,
                },
                recipients: vec![BOB.to_string()],
                device_id: None,
            }],
        )
        .await
        .expect("answered");

    assert_eq!(response.refused.len(), 1);
    assert_eq!(response.refused[0].reason, "invalid_envelope");
    assert_eq!(response.acked_through, "1");
}

// ── Items without a conversation ─────────────────────────────────────────────────────────

/// An item from `PEER` for Bob with id `id` on `stream` of workspace 5, carrying `event`.
fn stream_item(id: u32, stream: &str, event: serde_json::Value) -> DeliverItem {
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let event = event.to_string();
    let statement = sign_host_event(
        &host_signer(40),
        &HostEventFields {
            host: PEER,
            key_seq: 0,
            workspace_id: &workspace_id,
            stream,
            seq: (stream != "e").then_some(u64::from(id)),
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    DeliverItem {
        id: id.to_string(),
        envelope: SignedJson {
            payload: statement.payload,
            signature: statement.signature,
        },
        recipients: vec![BOB.to_string()],
        device_id: None,
    }
}

/// A relay holding Bob, subscribed to workspace 5 at `PEER` or not, who accepted a conversation
/// `PEER` homes or not; `queued` records the size of each item it queues.
fn subscription_relay(
    subscribed: bool,
    accepted_at_host: bool,
    queued: Arc<Mutex<Vec<i32>>>,
) -> DefaultRelayService {
    peer_limited_relay(
        subscribed,
        accepted_at_host,
        queued,
        QUEUED_BYTES_PER_PEER_PER_DAY,
    )
}

/// [`subscription_relay`] queueing `peer_bytes` from each peer per day.
fn peer_limited_relay(
    subscribed: bool,
    accepted_at_host: bool,
    queued: Arc<Mutex<Vec<i32>>>,
    peer_bytes: u64,
) -> DefaultRelayService {
    let bob = UserId::new();
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((bob, UserKind::Local))) }));
    let mut subscriptions = MockSubscriptionRepository::new();
    subscriptions
        .expect_is_subscribed()
        .returning(move |_, host, workspace, _| {
            let found = subscribed && host == PEER && workspace == Uuid::from_u128(5);
            Box::pin(async move { Ok(found) })
        });
    let mut relay = MockRelayRepository::new();
    relay
        .expect_received_through()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay
        .expect_advance_received_through()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    relay
        .expect_accepted_at_host()
        .returning(move |user, host| {
            let found = accepted_at_host && user == bob && host == PEER;
            Box::pin(async move { Ok(found) })
        });
    relay
        .expect_insert_envelope()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay.expect_insert_item().returning(move |item| {
        queued.lock().expect("lock").push(item.size_bytes);
        Box::pin(async { Ok(1) })
    });
    relay
        .expect_queued_bytes()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay_with_peer_bytes(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            subscription_repo: Arc::new(subscriptions),
            ..empty_repos()
        },
        peers_resolving(peer_keys(&host_signer(40), 0), None),
        MockS2sClient::new(),
        peer_bytes,
    )
}

/// The refusal reasons, with the principal each names, of one delivery of `items`.
async fn refusals(svc: &DefaultRelayService, items: Vec<DeliverItem>) -> Vec<(String, String)> {
    svc.deliver(PEER.to_string(), items)
        .await
        .expect("answered")
        .refused
        .into_iter()
        .map(|r| (r.reason, r.principal_id.unwrap_or_default()))
        .collect()
}

/// The items a workspace sends without a conversation, and those of the direct space.
fn items_without_a_conversation() -> Vec<(&'static str, DeliverItem)> {
    let bob_stream = format!("u:{BOB}");
    let alice_stream = format!("u:{ALICE}");
    vec![
        (
            "a workspace event",
            stream_item(
                1,
                "w",
                serde_json::json!({ "type": "WorkspacePluginChanged", "plugin_id": "p", "change": "added" }),
            ),
        ),
        (
            "presence",
            stream_item(
                2,
                "e",
                serde_json::json!({ "type": "PresenceChanged", "status": "online" }),
            ),
        ),
        (
            "a directory event about Bob",
            stream_item(
                3,
                &bob_stream,
                serde_json::json!({ "type": "MemberRoleChanged", "role": "guest" }),
            ),
        ),
        (
            "an event on another person's stream",
            stream_item(
                4,
                &alice_stream,
                serde_json::json!({ "type": "MemberLeft" }),
            ),
        ),
    ]
}

/// With a subscription to its workspace, every item without a conversation is queued.
#[tokio::test]
async fn items_without_a_conversation_are_queued_for_a_subscriber() {
    for (kind, item) in items_without_a_conversation() {
        let queued = Arc::default();
        let svc = subscription_relay(true, false, Arc::clone(&queued));
        assert_eq!(refusals(&svc, vec![item]).await, vec![], "{kind}");
        assert_eq!(queued.lock().expect("lock").len(), 1, "{kind}");
    }
}

/// Without a subscription, an item without a conversation is refused as `not_subscribed` for
/// its recipient, and nothing is queued.
#[tokio::test]
async fn items_without_a_conversation_are_refused_without_a_subscription() {
    for (kind, item) in items_without_a_conversation() {
        let queued = Arc::default();
        let svc = subscription_relay(false, false, Arc::clone(&queued));
        assert_eq!(
            refusals(&svc, vec![item]).await,
            vec![("not_subscribed".to_string(), BOB.to_string())],
            "{kind}"
        );
        assert!(queued.lock().expect("lock").is_empty(), "{kind}");
    }
}

/// Once Bob accepted a conversation the host homes, the items its direct space sends without
/// a conversation (presence on `e`, directory events on his own `u:` stream) are queued; a
/// workspace event and another person's stream are still refused.
#[tokio::test]
async fn direct_space_items_are_queued_once_a_conversation_there_is_accepted() {
    for (kind, item) in items_without_a_conversation() {
        let queued = Arc::default();
        let svc = subscription_relay(false, true, Arc::clone(&queued));
        let refused = refusals(&svc, vec![item]).await;
        let direct = kind == "presence" || kind == "a directory event about Bob";
        if direct {
            assert_eq!(refused, vec![], "{kind}");
            assert_eq!(queued.lock().expect("lock").len(), 1, "{kind}");
        } else {
            assert_eq!(
                refused,
                vec![("not_subscribed".to_string(), BOB.to_string())],
                "{kind}"
            );
            assert!(queued.lock().expect("lock").is_empty(), "{kind}");
        }
    }
}

/// A peer's items are queued up to its daily bytes across deliveries, then refused as
/// `limited` per recipient; items of a subscribed workspace are neither counted nor refused.
#[tokio::test]
async fn a_peer_past_its_daily_bytes_is_refused_as_limited() {
    let presence = |id| {
        stream_item(
            id,
            "e",
            serde_json::json!({ "type": "PresenceChanged", "status": "online" }),
        )
    };
    let size = {
        let item = presence(1);
        u64::try_from(item.envelope.payload.len() + item.envelope.signature.len()).expect("size")
    };
    let queued = Arc::new(Mutex::new(Vec::new()));
    let svc = peer_limited_relay(false, true, Arc::clone(&queued), 2 * size + size / 2);
    assert_eq!(refusals(&svc, vec![presence(1), presence(2)]).await, vec![]);
    assert_eq!(
        refusals(&svc, vec![presence(3)]).await,
        vec![("limited".to_string(), BOB.to_string())],
        "a third item takes the peer past its bytes"
    );
    assert_eq!(queued.lock().expect("lock").len(), 2);

    let subscribed = Arc::new(Mutex::new(Vec::new()));
    let svc = peer_limited_relay(true, false, Arc::clone(&subscribed), size);
    assert_eq!(
        refusals(&svc, (1..=3).map(presence).collect()).await,
        vec![],
        "a subscriber's items are not counted"
    );
    assert_eq!(subscribed.lock().expect("lock").len(), 3);
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
        .expect_count_contacts()
        .returning(|_| Box::pin(async { Ok(0) }));
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

/// A caller holding the most contacts adds no new one, but may still change one they hold.
#[tokio::test]
async fn a_caller_holding_the_most_contacts_adds_no_new_one() {
    let caller = UserId::new();
    let held = TestIdentity::new(71).principal_id();
    let new = TestIdentity::new(72).principal_id();
    let mut relay = MockRelayRepository::new();
    relay
        .expect_count_contacts()
        .returning(|_| Box::pin(async { Ok(crate::relay::MAX_CONTACTS_PER_USER) }));
    let listed = held.clone();
    relay.expect_is_contact().returning(move |_, principal| {
        let contact = principal == listed;
        Box::pin(async move { Ok(contact) })
    });
    let stored = held.clone();
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
                crate::probation_fixtures::account(
                    caller,
                    Utc::now(),
                    Some(&TestIdentity::new(70).principal_id()),
                ),
            ])),
            ..empty_repos()
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    );
    let relays = vec![PEER_ORIGIN.to_string()];

    let err = svc
        .add_contact(Actor::new(caller), new, relays.clone())
        .await
        .expect_err("past the cap");
    assert!(
        matches!(&err, ServiceError::Validation(m)
            if m == "a user holds at most 1000 contacts; remove one first"),
        "{err:?}"
    );
    let contact = svc
        .add_contact(Actor::new(caller), held.clone(), relays)
        .await
        .expect("a contact already held");
    assert_eq!(contact.principal_id, held);
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
            session_repo: Arc::new(rim_core::auth::repository::MockSessionRepository::new()),
            relay_abuse_repo: Arc::new(blocks(if facts.server_blocked { &[BOB] } else { &[] })),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
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

/// A host with a one-hour probation, read at `now`, where `ada` knows the holder of
/// identity 61 (a mutual contact or an accepted request), holds the holder of identity 65 as a
/// contact on her own, and shares a workspace with the holder of identity 62; `accounts` are
/// the users it knows.
fn probation_host(
    ada: UserId,
    accounts: Vec<rim_core::user::model::User>,
    now: chrono::DateTime<Utc>,
) -> DefaultHostService {
    knowing_host(
        ada,
        crate::probation_fixtures::users(accounts),
        now,
        MockPeerService::new(),
    )
}

/// [`probation_host`] over the accounts of `users`, pinning other servers through `peers`.
fn knowing_host(
    ada: UserId,
    users: MockUserRepository,
    now: chrono::DateTime<Utc>,
    peers: MockPeerService,
) -> DefaultHostService {
    let known = TestIdentity::new(61).principal_id();
    let one_sided = TestIdentity::new(65).principal_id();
    let coworker = TestIdentity::new(62).principal_id();
    let mut relay = MockRelayRepository::new();
    relay.expect_knows().returning(move |user, principal| {
        let knows = user == ada && principal == known;
        Box::pin(async move { Ok(knows) })
    });
    relay.expect_is_contact().returning(move |user, principal| {
        let contact = user == ada && principal == one_sided;
        Box::pin(async move { Ok(contact) })
    });
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(|_| Box::pin(async { Ok(None) }));
    // Users it knows by id only: no DM settings or relays to check.
    host.expect_user_kind()
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
            user_repo: Arc::new(users),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            session_repo: Arc::new(rim_core::auth::repository::MockSessionRepository::new()),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        },
        None,
        identity(41),
        Arc::new(peers),
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
    let (ada, known, coworker, stranger, one_sided) = (
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
        UserId::new(),
    );
    let stranger_principal = TestIdentity::new(63).principal_id();
    let accounts = vec![
        account(ada, created, Some(&TestIdentity::new(60).principal_id())),
        account(known, created, Some(&TestIdentity::new(61).principal_id())),
        account(
            coworker,
            created,
            Some(&TestIdentity::new(62).principal_id()),
        ),
        account(stranger, created, Some(&stranger_principal)),
        account(
            one_sided,
            created,
            Some(&TestIdentity::new(65).principal_id()),
        ),
    ];
    let refusal = "a new account can send a DM request to a stranger in 40 minutes";
    let refused = |result: Result<(), ServiceError>| match result {
        Err(ServiceError::Validation(message)) => assert_eq!(message, refusal),
        other => panic!("expected the probation refusal, got {other:?}"),
    };

    let host = probation_host(ada, accounts.clone(), in_probation);
    let direct = host.direct_space_id();
    host.ensure_may_request(Actor::new(ada), direct, vec![ada, known, coworker], false)
        .await
        .expect("people she knows and coworkers are no strangers");
    refused(
        host.ensure_may_request(Actor::new(ada), direct, vec![known, stranger], false)
            .await,
    );
    refused(
        host.ensure_may_request(Actor::new(ada), direct, vec![one_sided], false)
            .await,
    );
    for seed in [64, 65] {
        refused(
            host.ensure_remote_member(
                Actor::new(ada),
                rim_services::host::dto::RemoteMemberRef {
                    principal_id: TestIdentity::new(seed).principal_id(),
                    relay_urls: vec!["https://relay.example".to_string()],
                },
            )
            .await
            .map(|_| ()),
        );
    }

    let later = probation_host(ada, accounts.clone(), created + Duration::hours(1));
    later
        .ensure_may_request(
            Actor::new(ada),
            later.direct_space_id(),
            vec![stranger],
            false,
        )
        .await
        .expect("probation is over");
    let off = probation_host(ada, accounts, in_probation)
        .with_limits(rim_services::limits::InstanceLimits::default());
    off.ensure_may_request(
        Actor::new(ada),
        off.direct_space_id(),
        vec![stranger],
        false,
    )
    .await
    .expect("no probation on this instance");
}

#[tokio::test]
async fn an_instance_operator_sends_a_dm_request_to_a_stranger_however_new_the_account() {
    use crate::probation_fixtures::{account, users_and_operators};
    let created = Utc::now();
    let (ada, stranger) = (UserId::new(), UserId::new());
    let accounts = vec![
        account(ada, created, Some(&TestIdentity::new(60).principal_id())),
        account(
            stranger,
            created,
            Some(&TestIdentity::new(63).principal_id()),
        ),
    ];
    let host = knowing_host(
        ada,
        users_and_operators(accounts, vec![ada]),
        created + Duration::minutes(20),
        MockPeerService::new(),
    );
    host.ensure_may_request(
        Actor::new(ada),
        host.direct_space_id(),
        vec![stranger],
        false,
    )
    .await
    .expect("an instance operator is never on probation");
}

#[tokio::test]
async fn only_people_a_user_knows_skip_the_daily_invites_to_other_servers() {
    let ada = UserId::new();
    let mut peers = MockPeerService::new();
    peers
        .expect_pin()
        .returning(|_, _| Box::pin(async { Err(ServiceError::Unavailable) }));
    let host = knowing_host(
        ada,
        crate::probation_fixtures::users(Vec::new()),
        Utc::now(),
        peers,
    )
    .with_limits(rim_services::limits::InstanceLimits::default())
    .with_quotas(Quotas {
        dm_requests_per_day: 1,
        ..Quotas::default()
    });
    let invite = |seed: u8| {
        host.ensure_remote_member(
            Actor::new(ada),
            rim_services::host::dto::RemoteMemberRef {
                principal_id: TestIdentity::new(seed).principal_id(),
                relay_urls: vec!["https://relay.example".to_string()],
            },
        )
    };
    let limited =
        |result: &Result<_, ServiceError>| matches!(result, Err(ServiceError::RateLimited { .. }));

    // The day's one invite goes to a stranger, whose relay does not answer.
    assert!(
        !limited(&invite(64).await),
        "the first invite is within the quota"
    );
    // A contact Ada added on her own is counted like a stranger.
    assert!(
        limited(&invite(65).await),
        "a one-sided contact is over the quota"
    );
    // Someone she knows is not counted.
    assert!(
        !limited(&invite(61).await),
        "someone she knows is no invite"
    );
}

#[tokio::test]
async fn outbound_none_refuses_a_person_on_another_server_before_reaching_their_relay() {
    let ada = UserId::new();
    let host = probation_host(ada, Vec::new(), Utc::now()).with_dm_policy(DmOperatorPolicy {
        outbound: rim_core::dm_policy::model::DmOutbound::None,
        ..Default::default()
    });
    let err = host
        .ensure_remote_member(
            Actor::new(ada),
            rim_services::host::dto::RemoteMemberRef {
                principal_id: TestIdentity::new(64).principal_id(),
                relay_urls: vec!["https://relay.example".to_string()],
            },
        )
        .await
        .expect_err("refused");
    assert!(matches!(err, ServiceError::Forbidden(_)), "{err:?}");
}

// ── DM request limits when a conversation is started ─────────────────────────────────────

/// Requests per sender per day in the limit tests.
const DAILY: u32 = 3;

/// What a host knows of Ada's DM requests: how many she filed in the last day, each
/// recipient's pending requests, who holds her as a contact, who shares a workspace with her
/// (and how they take DMs from its members), and whether her DM with the recipient exists.
#[derive(Default, Clone)]
struct Inbox {
    filed_today: i64,
    pending: Vec<(UserId, i64)>,
    contacts: Vec<UserId>,
    coworkers: Vec<(UserId, rim_core::dm_policy::model::MemberDm)>,
    existing_dm: bool,
}

/// A host at `now` with a daily limit of `DAILY` requests per sender, where Ada's oldest
/// request of the window was filed 20 hours ago.
fn limits_host(ada: UserId, inbox: Inbox, now: chrono::DateTime<Utc>) -> DefaultHostService {
    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    use rim_core::dm_policy::model::{OutsideDm, OutsideFloor, SharedWorkspace, UserWorkspaceDm};

    let mut host = MockHostRepository::new();
    host.expect_user_kind()
        .returning(|_| Box::pin(async { Ok(Some(UserKind::Local)) }));
    let accounts = vec![crate::probation_fixtures::account(
        ada,
        now - Duration::days(30),
        Some(&TestIdentity::new(70).principal_id()),
    )];
    let mut relay = MockRelayRepository::new();
    let contacts = inbox.contacts.clone();
    relay.expect_is_contact().returning(move |user, _| {
        let known = contacts.contains(&user);
        Box::pin(async move { Ok(known) })
    });
    relay
        .expect_accepted_from()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let pending = inbox.pending.clone();
    relay
        .expect_count_pending_requests()
        .returning(move |user| {
            let n = pending
                .iter()
                .find(|(u, _)| *u == user)
                .map_or(0, |(_, n)| *n);
            Box::pin(async move { Ok(n) })
        });
    let filed = inbox.filed_today;
    relay
        .expect_count_requests_from()
        .returning(move |_, since| {
            assert_eq!(since, now - Duration::days(1));
            Box::pin(async move { Ok(filed) })
        });
    relay
        .expect_oldest_request_from()
        .returning(move |_, _| Box::pin(async move { Ok(Some(now - Duration::hours(20))) }));
    let mut dm = MockDmPolicyRepository::new();
    let coworkers = inbox.coworkers.clone();
    dm.expect_shared_workspaces().returning(move |user, _| {
        let shared = coworkers
            .iter()
            .filter(|(u, _)| *u == user)
            .map(|(_, members)| SharedWorkspace {
                workspace: UserWorkspaceDm {
                    server_id: ServerId::from(Uuid::from_u128(7)),
                    name: "Acme".to_string(),
                    outside_floor: OutsideFloor::None,
                    member_default: *members,
                    choice: None,
                },
                sender_role: ServerMemberRole::Member,
            })
            .collect();
        Box::pin(async move { Ok(shared) })
    });
    dm.expect_user_workspaces()
        .returning(|_| Box::pin(async { Ok(Vec::new()) }));
    dm.expect_outside()
        .returning(|_| Box::pin(async { Ok(OutsideDm::Requests) }));
    let mut conversations = MockConversationRepository::new();
    let existing = inbox.existing_dm;
    conversations
        .expect_find_direct_between_users()
        .returning(move |server_id, user, _| {
            let found = existing.then(|| Conversation {
                id: ConversationId::new(),
                conversation_type: ConversationType::Direct,
                title: None,
                topic: None,
                created_at: now,
                last_activity_at: now,
                server_id,
                is_private: false,
                archived_at: None,
                created_by: Some(user),
                slow_mode_seconds: 0,
                encrypted_at: None,
            });
            Box::pin(async move { Ok(found) })
        });
    DefaultHostService::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(relay),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(crate::probation_fixtures::users(accounts)),
            conversation_repo: Arc::new(conversations),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
            dm_policy_repo: Arc::new(dm),
            session_repo: Arc::new(rim_core::auth::repository::MockSessionRepository::new()),
        },
        None,
        identity(42),
        Arc::new(MockPeerService::new()),
        Arc::new(MockS2sClient::new()),
        ServerId::new(),
        120,
    )
    .with_quotas(Quotas {
        dm_requests_per_day: DAILY,
        ..Quotas::default()
    })
    .with_clock(crate::clock::Clock::fixed(now))
}

/// Ada starting a conversation with `others` in the direct space, a DM when `dm`.
async fn start_direct(
    inbox: Inbox,
    ada: UserId,
    others: Vec<UserId>,
    dm: bool,
) -> Result<(), ServiceError> {
    let host = limits_host(ada, inbox, Utc::now());
    let direct = host.direct_space_id();
    host.ensure_may_request(Actor::new(ada), direct, others, dm)
        .await
}

#[tokio::test]
async fn a_dm_request_past_the_senders_daily_limit_is_refused_until_a_slot_opens() {
    let (ada, stranger) = (UserId::new(), UserId::new());
    let within = Inbox {
        filed_today: i64::from(DAILY) - 1,
        ..Inbox::default()
    };
    start_direct(within, ada, vec![stranger], true)
        .await
        .expect("within the limit");
    let over = Inbox {
        filed_today: i64::from(DAILY),
        ..Inbox::default()
    };
    match start_direct(over, ada, vec![stranger], true).await {
        Err(ServiceError::RateLimited {
            retry_after_seconds,
        }) => assert_eq!(retry_after_seconds, 4 * 60 * 60, "the oldest leaves in 4 h"),
        other => panic!("expected RateLimited, got {other:?}"),
    }
}

#[tokio::test]
async fn a_full_request_inbox_refuses_new_requests() {
    let (ada, stranger) = (UserId::new(), UserId::new());
    let full = Inbox {
        pending: vec![(stranger, crate::relay::inbox::MAX_PENDING_PER_USER)],
        ..Inbox::default()
    };
    match start_direct(full, ada, vec![stranger], true).await {
        Err(ServiceError::Forbidden(message)) => {
            assert_eq!(message, "their request inbox is full")
        }
        other => panic!("expected Forbidden, got {other:?}"),
    }
}

#[tokio::test]
async fn a_group_counts_each_request_it_files() {
    let ada = UserId::new();
    let strangers: Vec<UserId> = (0..3).map(|_| UserId::new()).collect();
    let inbox = Inbox {
        filed_today: 1,
        ..Inbox::default()
    };
    start_direct(inbox.clone(), ada, strangers[..2].to_vec(), false)
        .await
        .expect("two more fit");
    assert!(matches!(
        start_direct(inbox, ada, strangers, false).await,
        Err(ServiceError::RateLimited { .. })
    ));
}

#[tokio::test]
async fn contacts_coworkers_and_a_reopened_dm_meet_no_request_limit() {
    use rim_core::dm_policy::model::MemberDm;
    let (ada, contact, coworker, stranger) =
        (UserId::new(), UserId::new(), UserId::new(), UserId::new());
    let over = Inbox {
        filed_today: i64::from(DAILY),
        pending: vec![
            (contact, crate::relay::inbox::MAX_PENDING_PER_USER),
            (coworker, crate::relay::inbox::MAX_PENDING_PER_USER),
        ],
        contacts: vec![contact],
        coworkers: vec![(coworker, MemberDm::Direct)],
        ..Inbox::default()
    };
    start_direct(over.clone(), ada, vec![contact, coworker], false)
        .await
        .expect("delivered directly, no request");
    start_direct(
        Inbox {
            existing_dm: true,
            ..over.clone()
        },
        ada,
        vec![stranger],
        true,
    )
    .await
    .expect("an existing DM is reopened");
    assert!(start_direct(over, ada, vec![stranger], true).await.is_err());
}

#[tokio::test]
async fn a_workspace_dm_taken_as_a_request_meets_the_limits_and_groups_there_do_not() {
    use rim_core::dm_policy::model::MemberDm;
    let (ada, direct, requests) = (UserId::new(), UserId::new(), UserId::new());
    let inbox = Inbox {
        filed_today: i64::from(DAILY),
        coworkers: vec![(direct, MemberDm::Direct), (requests, MemberDm::Requests)],
        ..Inbox::default()
    };
    let host = limits_host(ada, inbox, Utc::now());
    let acme = ServerId::from(Uuid::from_u128(7));
    host.ensure_may_request(Actor::new(ada), acme, vec![direct], true)
        .await
        .expect("a DM the member takes directly");
    assert!(matches!(
        host.ensure_may_request(Actor::new(ada), acme, vec![requests], true)
            .await,
        Err(ServiceError::RateLimited { .. })
    ));
    host.ensure_may_request(Actor::new(ada), acme, vec![requests, direct], false)
        .await
        .expect("a workspace group files no request");
}

// ── Accepted conversations ───────────────────────────────────────────────────────────────

/// What the request inbox did to Bob's rows.
#[derive(Debug, Clone, PartialEq)]
enum RowCall {
    Refreshed(Option<chrono::DateTime<Utc>>, Option<Uuid>),
    Deleted(String, Uuid),
}

/// The conversation the accepted-row tests deliver for.
const CONVERSATION: Uuid = Uuid::from_u128(9);
/// The id `PEER` gives Bob.
const BOB_AT_HOST: Uuid = Uuid::from_u128(77);

/// Bob's row for `CONVERSATION` at `PEER`, accepted and expiring at `expires_at`, with the id
/// the host gives Bob when `bound`.
fn accepted_row(
    user_id: UserId,
    expires_at: chrono::DateTime<Utc>,
    bound: bool,
) -> rim_core::relay::model::DmRequest {
    use rim_core::relay::model::{DmRequest, DmRequestId, DmRequestStatus};
    DmRequest {
        id: DmRequestId::new(),
        user_id,
        from_principal_id: ALICE.to_string(),
        card_payload: String::new(),
        card_signature: String::new(),
        host_server_id: PEER.to_string(),
        conversation_id: CONVERSATION,
        status: DmRequestStatus::Accepted,
        created_at: Utc::now(),
        decided_at: Some(Utc::now()),
        expires_at,
        host_user_id: bound.then_some(BOB_AT_HOST),
    }
}

/// Bob's relay, where `row` (made for Bob's user id) is his row for every conversation;
/// `calls` records what it does to rows.
fn accepted_relay(
    row: impl Fn(UserId) -> Option<rim_core::relay::model::DmRequest> + Send + Sync + 'static,
    calls: Arc<Mutex<Vec<RowCall>>>,
) -> DefaultRelayService {
    let bob = UserId::new();
    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((bob, UserKind::Local))) }));
    let mut subscriptions = MockSubscriptionRepository::new();
    subscriptions
        .expect_is_subscribed()
        .returning(|_, _, _, _| Box::pin(async { Ok(false) }));
    let mut relay = MockRelayRepository::new();
    relay
        .expect_received_through()
        .returning(|_| Box::pin(async { Ok(0) }));
    relay
        .expect_advance_received_through()
        .returning(|_, _| Box::pin(async { Ok(()) }));
    relay.expect_find_request().returning(move |_, _, _| {
        let found = row(bob);
        Box::pin(async move { Ok(found) })
    });
    let log = Arc::clone(&calls);
    relay
        .expect_refresh_request()
        .returning(move |_, expires_at, host_user_id| {
            log.lock()
                .expect("lock")
                .push(RowCall::Refreshed(expires_at, host_user_id));
            Box::pin(async { Ok(()) })
        });
    let log = Arc::clone(&calls);
    relay
        .expect_delete_accepted_request()
        .returning(move |_, host, conversation_id| {
            log.lock()
                .expect("lock")
                .push(RowCall::Deleted(host.to_string(), conversation_id));
            Box::pin(async { Ok(1) })
        });
    relay
        .expect_insert_envelope()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_insert_item()
        .returning(|_| Box::pin(async { Ok(1) }));
    relay
        .expect_queued_bytes()
        .returning(|_| Box::pin(async { Ok(0) }));
    let signer = host_signer(40);
    relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay),
            host_repo: Arc::new(host),
            subscription_repo: Arc::new(subscriptions),
            ..empty_repos()
        },
        peers_resolving(peer_keys(&signer, 0), None),
        MockS2sClient::new(),
    )
}

/// An event of `CONVERSATION` from `PEER` for Bob on `stream`.
fn conversation_item(stream: &str, event: serde_json::Value) -> DeliverItem {
    let workspace_id = Uuid::from_u128(5).to_string();
    let issued_at = timestamp(Utc::now());
    let event = event.to_string();
    let statement = sign_host_event(
        &host_signer(40),
        &HostEventFields {
            host: PEER,
            key_seq: 0,
            workspace_id: &workspace_id,
            stream,
            seq: Some(2),
            event_json: &event,
            issued_at: &issued_at,
        },
    );
    DeliverItem {
        id: "2".to_string(),
        envelope: SignedJson {
            payload: statement.payload,
            signature: statement.signature,
        },
        recipients: vec![BOB.to_string()],
        device_id: None,
    }
}

/// What Bob's relay, holding `row`, does with `item` from `PEER`: the refusal reasons and the
/// calls on rows.
async fn deliver_accepted(
    row: impl Fn(UserId) -> Option<rim_core::relay::model::DmRequest> + Send + Sync + 'static,
    item: DeliverItem,
) -> (Vec<String>, Vec<RowCall>) {
    let calls = Arc::default();
    let svc = accepted_relay(row, Arc::clone(&calls));
    let response = svc
        .deliver(PEER.to_string(), vec![item])
        .await
        .expect("delivered");
    let refused = response.refused.into_iter().map(|r| r.reason).collect();
    let calls = calls.lock().expect("lock").clone();
    (refused, calls)
}

#[tokio::test]
async fn an_invite_from_a_contact_files_an_accepted_row_without_a_card_that_expires() {
    use rim_core::relay::model::DmRequestStatus;
    let before = Utc::now();
    let inbox = RelayInbox {
        contact: true,
        ..RelayInbox::default()
    };
    let rows = Arc::clone(&inbox.rows);
    assert_eq!(
        file_invite(inbox, alice_invite(&sender_card(1, &[PEER_ORIGIN]))).await,
        filed_as(DmRequestStatus::Accepted)
    );
    let rows = rows.lock().expect("lock").clone();
    let [row] = rows.as_slice() else {
        panic!("one row filed: {rows:?}");
    };
    assert_eq!(
        (row.card_payload.as_str(), row.card_signature.as_str()),
        ("", ""),
        "a row accepted as it is filed keeps no card"
    );
    assert!(
        row.expires_at >= before + Duration::days(400)
            && row.expires_at <= Utc::now() + Duration::days(400),
        "an accepted row expires 400 days out, not with the relay: {}",
        row.expires_at
    );
}

#[tokio::test]
async fn an_invite_past_the_accepted_cap_for_its_host_is_refused_as_limited() {
    use rim_core::relay::model::DmRequestStatus;

    use crate::relay::inbox::MAX_ACCEPTED_PER_HOST;

    let holding = |accepted| RelayInbox {
        contact: true,
        accepted,
        ..RelayInbox::default()
    };
    let invite = alice_invite(&sender_card(1, &[PEER_ORIGIN]));
    assert_eq!(
        file_invite(holding(MAX_ACCEPTED_PER_HOST - 1), invite.clone()).await,
        filed_as(DmRequestStatus::Accepted)
    );
    assert_eq!(
        file_invite(holding(MAX_ACCEPTED_PER_HOST), invite).await,
        refused_as("limited")
    );
}

#[tokio::test]
async fn an_item_moves_a_stale_accepted_row_out_and_a_fresh_one_is_left_alone() {
    let message = || {
        conversation_item(
            &format!("c:{CONVERSATION}"),
            serde_json::json!({ "type": "NewMessage", "conversation_id": CONVERSATION }),
        )
    };
    let before = Utc::now();
    let stale = Utc::now() + Duration::days(300);
    let (refused, calls) =
        deliver_accepted(move |bob| Some(accepted_row(bob, stale, true)), message()).await;
    assert!(refused.is_empty());
    let [RowCall::Refreshed(Some(expires_at), None)] = calls.as_slice() else {
        panic!("the row's expiry moves: {calls:?}");
    };
    assert!(*expires_at >= before + Duration::days(400));

    let fresh = Utc::now() + Duration::days(400);
    assert_eq!(
        deliver_accepted(move |bob| Some(accepted_row(bob, fresh, true)), message()).await,
        (Vec::new(), Vec::new()),
        "a row moved out within the day is not written again"
    );
}

#[tokio::test]
async fn an_event_on_the_users_own_stream_names_the_id_the_host_gives_them() {
    let welcome = |stream: String| {
        conversation_item(
            &stream,
            serde_json::json!({
                "type": "MlsWelcomeAvailable",
                "conversation_id": CONVERSATION,
                "user_id": BOB_AT_HOST,
                "device_id": Uuid::from_u128(1),
            }),
        )
    };
    let fresh = Utc::now() + Duration::days(400);
    assert_eq!(
        deliver_accepted(
            move |bob| Some(accepted_row(bob, fresh, false)),
            welcome(format!("u:{BOB}"))
        )
        .await,
        (
            Vec::new(),
            vec![RowCall::Refreshed(None, Some(BOB_AT_HOST))]
        )
    );
    assert_eq!(
        deliver_accepted(
            move |bob| Some(accepted_row(bob, fresh, false)),
            welcome(format!("u:{ALICE}"))
        )
        .await,
        (Vec::new(), Vec::new()),
        "another user's stream names nobody for Bob"
    );
}

#[tokio::test]
async fn the_hosts_removal_of_the_user_deletes_their_accepted_row() {
    let removed = |user_id: Uuid| {
        conversation_item(
            &format!("c:{CONVERSATION}"),
            serde_json::json!({
                "type": "MemberRemovedFromConversation",
                "conversation_id": CONVERSATION,
                "user_id": user_id,
            }),
        )
    };
    let fresh = Utc::now() + Duration::days(400);
    assert_eq!(
        deliver_accepted(
            move |bob| Some(accepted_row(bob, fresh, true)),
            removed(BOB_AT_HOST)
        )
        .await,
        (
            Vec::new(),
            vec![RowCall::Deleted(PEER.to_string(), CONVERSATION)]
        ),
        "Bob still gets the event that removes him"
    );
    assert_eq!(
        deliver_accepted(
            move |bob| Some(accepted_row(bob, fresh, true)),
            removed(Uuid::from_u128(78))
        )
        .await,
        (Vec::new(), Vec::new()),
        "another member's removal keeps Bob's row"
    );
    assert_eq!(
        deliver_accepted(
            move |bob| Some(accepted_row(bob, fresh, false)),
            removed(BOB_AT_HOST)
        )
        .await,
        (Vec::new(), Vec::new()),
        "a row that never learnt Bob's id at the host is left to expire"
    );
}

/// A writer for a server whose direct space is `direct` and whose relay deletes accepted rows
/// into `deleted`.
fn removal_writer(
    direct: ServerId,
    deleted: Arc<Mutex<Vec<(UserId, String, Uuid)>>>,
) -> (crate::host::outbox::HostEventWriter, String) {
    use crate::host::outbox::{HostEventWriter, PeerBacklogs, RefusalCache};

    let shared = identity(30);
    let own_id = shared.current().server_id().to_string();
    let mut host = MockHostRepository::new();
    host.expect_remote_members()
        .returning(|_| Box::pin(async { Ok(Vec::new()) }));
    let mut relay = MockRelayRepository::new();
    relay
        .expect_delete_accepted_request()
        .returning(move |user_id, host, conversation_id| {
            deleted
                .lock()
                .expect("lock")
                .push((user_id, host.to_string(), conversation_id));
            Box::pin(async { Ok(1) })
        });
    let mut relay_side = MockRelayRepository::new();
    relay_side
        .expect_pending_holders()
        .returning(|_| Box::pin(async { Ok(Vec::new()) }));
    let relay_service = relay_service(
        RelayRepos {
            relay_repo: Arc::new(relay_side),
            ..empty_repos()
        },
        MockPeerService::new(),
        MockS2sClient::new(),
    );
    let writer = HostEventWriter::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(relay),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            session_repo: Arc::new(rim_core::auth::repository::MockSessionRepository::new()),
            relay_abuse_repo: Arc::new(blocks(&[])),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        },
        None,
        shared,
        direct,
        Arc::new(RefusalCache::default()),
        Arc::new(PeerBacklogs::new(Duration::hours(24))),
        Arc::new(tokio::sync::Notify::new()),
    )
    .with_relay(Arc::new(relay_service));
    (writer, own_id)
}

#[tokio::test]
async fn a_local_member_removed_from_a_conversation_here_loses_its_accepted_row() {
    use rim_core::conversation::model::ConversationId;
    use rim_services::events::WorkspaceEvent;

    let direct = ServerId::from(Uuid::from_u128(5));
    let deleted = Arc::default();
    let (writer, own_id) = removal_writer(direct, Arc::clone(&deleted));
    let (bob, conversation) = (UserId::new(), Uuid::from_u128(9));
    writer
        .write(&WorkspaceEvent::MemberRemovedFromConversation {
            workspace_id: direct,
            conversation_id: ConversationId::from(conversation),
            user_id: bob,
            member_user_ids: vec![bob],
        })
        .await
        .expect("written");
    assert_eq!(
        deleted.lock().expect("lock").as_slice(),
        &[(bob, own_id, conversation)]
    );
}

/// A writer for a server whose direct space is `direct` and holds `conversation`, created by
/// Ada with a card; Bob, from the relay `PEER`, is a remote member of the direct space and, when
/// `member`, of the conversation. `outbox` records the relay and item of each row added.
fn reinvite_writer(
    direct: ServerId,
    conversation: rim_core::conversation::model::Conversation,
    member: bool,
    outbox: Arc<Mutex<Vec<(String, String)>>>,
) -> crate::host::outbox::HostEventWriter {
    use rim_core::conversation_member::model::Membership;
    use rim_core::host::model::RemoteMember;
    use rim_core::user::model::User;

    use crate::host::outbox::{HostEventWriter, PeerBacklogs, RefusalCache};

    let (ada, bob) = (conversation.created_by.expect("creator"), UserId::new());
    let mut host = MockHostRepository::new();
    host.expect_find_user().returning(move |principal| {
        let found = (principal == BOB).then_some((bob, UserKind::Remote));
        Box::pin(async move { Ok(found) })
    });
    host.expect_remote_members().returning(move |_| {
        Box::pin(async move {
            Ok(vec![RemoteMember {
                user_id: bob,
                principal_id: BOB.to_string(),
                role: ServerMemberRole::Guest,
                home_relay: Some(PEER.to_string()),
                card_payload: None,
            }])
        })
    });
    host.expect_next_stream_seq()
        .returning(|_, _| Box::pin(async { Ok(1) }));
    host.expect_outbox_backlog().returning(|_| {
        Box::pin(async {
            Ok(rim_core::host::model::OutboxBacklog {
                rows: 0,
                bytes: 0,
                failing_since: None,
            })
        })
    });
    host.expect_insert_outbox()
        .returning(move |relay, item, _| {
            outbox
                .lock()
                .expect("lock")
                .push((relay.to_string(), item.to_string()));
            Box::pin(async { Ok(1) })
        });
    let stored = conversation.clone();
    let mut conversations = MockConversationRepository::new();
    conversations.expect_get_by_id().returning(move |id| {
        let found = (id == stored.id).then(|| stored.clone());
        Box::pin(async move { Ok(found) })
    });
    let mut members = MockConversationMemberRepository::new();
    members.expect_membership().returning(move |_, user_id| {
        let found = (member && user_id == bob).then_some(Membership::Active);
        Box::pin(async move { Ok(found) })
    });
    members
        .expect_list_member_ids()
        .returning(move |_| Box::pin(async move { Ok(vec![ada, bob]) }));
    let user = |id: UserId, principal: &str| User {
        id,
        username: principal.to_string(),
        password_hash: None,
        created_at: Utc::now(),
        updated_at: Utc::now(),
        last_login_at: None,
        display_name: None,
        avatar_url: None,
        status_text: None,
        bio: None,
        email: None,
        timezone: None,
        principal_id: Some(principal.to_string()),
        deletion_scheduled_for: None,
        handle: rim_core::user::model::UserHandle::Local,
    };
    let mut users = MockUserRepository::new();
    let creator = user(ada, ALICE);
    users
        .expect_get_by_id()
        .returning(move |_| Ok(Some(creator.clone())));
    let all = vec![user(ada, ALICE), user(bob, BOB)];
    users
        .expect_get_by_ids()
        .returning(move |_| Ok(all.clone()));
    let mut keys = MockKeyIdentityRepository::new();
    keys.expect_get_profile_card().returning(move |user_id| {
        Box::pin(async move {
            Ok(Some(StoredProfileCard {
                user_id,
                version: 1,
                payload: "{}".to_string(),
                signature: "ed25519:c".to_string(),
                updated_at: Utc::now(),
            }))
        })
    });
    HostEventWriter::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(MockRelayRepository::new()),
            key_identity_repo: Arc::new(keys),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(users),
            conversation_repo: Arc::new(conversations),
            conversation_member_repo: Arc::new(members),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            session_repo: Arc::new(rim_core::auth::repository::MockSessionRepository::new()),
            relay_abuse_repo: Arc::new(blocks(&[])),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        },
        None,
        identity(30),
        direct,
        Arc::new(RefusalCache::default()),
        Arc::new(PeerBacklogs::new(Duration::hours(24))),
        Arc::new(tokio::sync::Notify::new()),
    )
}

/// A group of the space `server_id`, created by someone.
fn group_in(server_id: ServerId) -> rim_core::conversation::model::Conversation {
    use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
    Conversation {
        id: ConversationId::from(Uuid::from_u128(9)),
        conversation_type: ConversationType::Group,
        title: None,
        topic: None,
        created_at: Utc::now(),
        last_activity_at: Utc::now(),
        server_id,
        is_private: false,
        archived_at: None,
        created_by: Some(UserId::new()),
        slow_mode_seconds: 0,
        encrypted_at: Some(Utc::now()),
    }
}

#[tokio::test]
async fn a_member_whose_relay_refused_a_conversation_gets_its_invite_again() {
    let direct = ServerId::from(Uuid::from_u128(5));
    let outbox = Arc::default();
    let writer = reinvite_writer(direct, group_in(direct), true, Arc::clone(&outbox));
    assert_eq!(
        writer
            .reinvite(Uuid::from_u128(9), BOB)
            .await
            .expect("sent"),
        1
    );
    let rows = outbox.lock().expect("lock").clone();
    let [(relay, item)] = rows.as_slice() else {
        panic!("one row: {rows:?}");
    };
    assert_eq!(relay, PEER);
    let item: serde_json::Value = serde_json::from_str(item).expect("json");
    assert_eq!(item["recipients"], serde_json::json!([BOB]));
    let envelope = rim_identity::SignedStatement {
        payload: item["envelope"]["payload"]
            .as_str()
            .expect("payload")
            .to_string(),
        signature: item["envelope"]["signature"]
            .as_str()
            .expect("signature")
            .to_string(),
    };
    let event = rim_s2s::host::read_host_event(&envelope).expect("host event");
    assert_eq!(event.stream, format!("u:{BOB}"));
    assert_eq!(event.event["type"], "DmInvite");
    assert_eq!(
        event.event["conversation_id"],
        Uuid::from_u128(9).to_string()
    );
    assert_eq!(event.event["from_principal_id"], ALICE);
    assert_eq!(
        event.event["member_principal_ids"],
        serde_json::json!([ALICE, BOB])
    );
}

#[tokio::test]
async fn no_invite_is_sent_again_to_a_non_member_or_for_a_conversation_elsewhere() {
    let direct = ServerId::from(Uuid::from_u128(5));
    for (conversation, member, principal) in [
        (group_in(direct), false, BOB),
        (group_in(direct), true, "rimp1_carol"),
        (group_in(ServerId::from(Uuid::from_u128(6))), true, BOB),
    ] {
        let outbox = Arc::default();
        let writer = reinvite_writer(direct, conversation, member, Arc::clone(&outbox));
        assert_eq!(
            writer
                .reinvite(Uuid::from_u128(9), principal)
                .await
                .expect("answered"),
            0
        );
        assert!(outbox.lock().expect("lock").is_empty());
    }
    let outbox = Arc::default();
    let writer = reinvite_writer(direct, group_in(direct), true, Arc::clone(&outbox));
    assert_eq!(
        writer
            .reinvite(Uuid::from_u128(10), BOB)
            .await
            .expect("answered"),
        0,
        "a conversation gone"
    );
}
