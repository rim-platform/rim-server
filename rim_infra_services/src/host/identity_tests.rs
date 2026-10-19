//! Which relays a host takes a remote user's identity from: pushes from relays that speak for
//! the user and refusals of all others, the relay a refresh goes to, and the device activity
//! and home relay each source may change.

use std::sync::{Arc, Mutex};

use chrono::{Duration, Utc};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::dm_policy::repository::MockDmPolicyRepository;
use rim_core::e2ee::repository::MockE2eeRepository;
use rim_core::host::model::UserKind;
use rim_core::host::repository::{MockDelegationRepository, MockHostRepository};
use rim_core::key_identity::model::{
    DeviceKey, DeviceKeyId, KeyLogEntry, KeyLogEntryType, StoredProfileCard,
};
use rim_core::key_identity::repository::MockKeyIdentityRepository;
use rim_core::relay::model::CachedIdentity;
use rim_core::relay::repository::{MockRelayAbuseRepository, MockRelayRepository};
use rim_core::server::model::ServerId;
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::MockUserRepository;
use rim_identity::KeyState;
use rim_identity::testing::{TestIdentity, signing_key};
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerIdentity,
    ServerKeyState, ServerSigner, SharedIdentity, sign_descriptor, sign_inception, timestamp,
};
use rim_services::Actor;
use rim_services::host::dto::{IdentityPush, RemoteMemberRef};
use rim_services::host::service::HostService;
use rim_services::relay::dto::IdentityDocument;
use rim_services::s2s::dto::{PeerDto, PeerKeysDto, PeerLookup, S2sResponse};
use rim_services::s2s::service::{MockPeerService, MockS2sClient};
use uuid::Uuid;

use crate::host::{DefaultHostService, HostRepos};
use crate::relay::identity::testing::{document, signed};

/// The relay the user's first card lists (`https://relay.example`, as in `document`).
const RELAY: &str = "rims1_card_relay";
const RELAY_ORIGIN: &str = "https://relay.example";
/// A pinned peer the user's first card does not list.
const STRANGER: &str = "rims1_stranger";
const STRANGER_ORIGIN: &str = "https://stranger.example";

fn server_identity() -> SharedIdentity {
    let signer = ServerSigner::from_seed([42; 32]);
    let recovery = ServerSigner::from_seed([142; 32]);
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
        state: state.clone(),
        log: vec![inception],
        descriptor,
        descriptor_content: content,
    })
}

fn server_at(origin: &str) -> Option<&'static str> {
    match origin {
        RELAY_ORIGIN => Some(RELAY),
        STRANGER_ORIGIN => Some(STRANGER),
        _ => None,
    }
}

fn origin_of_server(server_id: &str) -> Option<&'static str> {
    match server_id {
        RELAY => Some(RELAY_ORIGIN),
        STRANGER => Some(STRANGER_ORIGIN),
        _ => None,
    }
}

/// What the host holds about the remote user before the identity arrives.
#[derive(Clone, Copy)]
struct Facts {
    /// The stored card's version and the one relay it lists.
    stored_card: Option<(u32, &'static str)>,
    home_relay: Option<&'static str>,
    /// The relay holding an active delegation of one of the user's devices.
    delegated_relay: Option<&'static str>,
    /// The relay the cached copy names as its source, cached two hours ago.
    cached_from: Option<&'static str>,
}

const FACTS: Facts = Facts {
    stored_card: Some((1, RELAY_ORIGIN)),
    home_relay: Some(RELAY),
    delegated_relay: None,
    cached_from: None,
};

/// What the host wrote.
#[derive(Default)]
struct Writes {
    mls_active: Mutex<Vec<bool>>,
    /// The relays recorded as the source of the cached copy.
    cached_from: Mutex<Vec<String>>,
    /// The home relays passed to `update_remote_user`.
    home_relay: Mutex<Vec<Option<String>>>,
    /// The relays the host fetched the identity from.
    fetched_from: Mutex<Vec<String>>,
}

impl Writes {
    fn mls_active(&self) -> Vec<bool> {
        self.mls_active.lock().expect("lock").clone()
    }

    fn cached_from(&self) -> Vec<String> {
        self.cached_from.lock().expect("lock").clone()
    }

    fn home_relay(&self) -> Vec<Option<String>> {
        self.home_relay.lock().expect("lock").clone()
    }

    fn fetched_from(&self) -> Vec<String> {
        self.fetched_from.lock().expect("lock").clone()
    }
}

struct Alice {
    id: TestIdentity,
    state: KeyState,
}

impl Alice {
    fn new() -> Self {
        let id = TestIdentity::new(1);
        let state = KeyState::from_inception(&id.inception()).expect("inception");
        Self { id, state }
    }

    /// Her document with card version 1 listing `RELAY_ORIGIN` and her device's activity.
    fn document(&self, mls_active: bool) -> IdentityDocument {
        let mut doc = document(&self.id, &self.state);
        doc.devices[0].mls_active = mls_active;
        doc
    }

    /// Her document with a card of `version` listing `relays`.
    fn with_card(&self, mls_active: bool, version: u64, relays: &[&str]) -> IdentityDocument {
        let mut doc = self.document(mls_active);
        let card = self.id.profile_card(
            &self.state,
            version,
            "Alice",
            relays,
            &[(Uuid::from_u128(1), &signing_key(20))],
        );
        doc.card = Some(signed(&card));
        doc
    }

    /// Her document without a card.
    fn without_card(&self, mls_active: bool) -> IdentityDocument {
        let mut doc = self.document(mls_active);
        doc.card = None;
        doc
    }
}

/// A host holding Alice as a remote user per `facts`, whose relays serve `served`.
fn host(facts: Facts, served: &IdentityDocument) -> (DefaultHostService, Arc<Writes>) {
    let user = UserId::new();
    let writes = Arc::new(Writes::default());
    let key_log = served.key_log.clone();

    let mut host = MockHostRepository::new();
    host.expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((user, UserKind::Remote))) }));
    host.expect_home_relay().returning(move |_| {
        let home = facts.home_relay.map(str::to_string);
        Box::pin(async move { Ok(home) })
    });
    host.expect_peer_at_origin().returning(|origin| {
        let server = server_at(origin).map(str::to_string);
        Box::pin(async move { Ok(server) })
    });
    host.expect_join_as_guest()
        .returning(|_, _| Box::pin(async { Ok(false) }));
    let w = Arc::clone(&writes);
    host.expect_update_remote_user()
        .returning(move |_, _, home| {
            w.home_relay.lock().expect("lock").push(home);
            Box::pin(async { Ok(()) })
        });
    let w = Arc::clone(&writes);
    host.expect_set_device_mls_active()
        .returning(move |_, active| {
            w.mls_active.lock().expect("lock").push(active);
            Box::pin(async { Ok(()) })
        });

    let mut keys = MockKeyIdentityRepository::new();
    keys.expect_get_profile_card().returning(move |user_id| {
        let card = facts.stored_card.map(|(version, relay)| StoredProfileCard {
            user_id,
            version,
            payload: format!(r#"{{"relays":["{relay}"]}}"#),
            signature: "ed25519:c".to_string(),
            updated_at: Utc::now(),
        });
        Box::pin(async move { Ok(card) })
    });
    keys.expect_lock_user()
        .returning(|_| Box::pin(async { Ok(true) }));
    keys.expect_list_key_log().returning(move |user_id| {
        let entries = key_log
            .iter()
            .zip(0..)
            .map(|(entry, seq)| KeyLogEntry {
                user_id,
                seq,
                entry_type: KeyLogEntryType::Inception,
                payload: entry.payload.clone(),
                signature: entry.signature.clone(),
                created_at: Utc::now(),
            })
            .collect();
        Box::pin(async move { Ok(entries) })
    });
    keys.expect_get_device().returning(|user_id, device_id| {
        Box::pin(async move {
            Ok(Some(DeviceKey {
                id: DeviceKeyId::new(),
                user_id,
                device_id,
                public_key: "ed25519:x".to_string(),
                name: "Laptop".to_string(),
                cert_payload: "{}".to_string(),
                cert_signature: "ed25519:x".to_string(),
                key_seq: 0,
                added_at: Utc::now(),
                last_used_at: None,
                revoked_at: None,
                revocation_payload: None,
                revocation_signature: None,
            }))
        })
    });
    keys.expect_upsert_profile_card()
        .returning(|_, _, _, _| Box::pin(async { Ok(true) }));

    let mut delegations = MockDelegationRepository::new();
    delegations
        .expect_delegated_workspaces()
        .returning(move |_, relay, _| {
            let held = facts.delegated_relay == Some(relay);
            Box::pin(async move { Ok(if held { vec![ServerId::new()] } else { vec![] }) })
        });

    let mut relay = MockRelayRepository::new();
    let w = Arc::clone(&writes);
    relay
        .expect_store_identity()
        .returning(move |_, served_by, _| {
            w.cached_from
                .lock()
                .expect("lock")
                .push(served_by.to_string());
            Box::pin(async { Ok(()) })
        });
    let cached_text = serde_json::to_string(served).expect("json");
    relay
        .expect_cached_identity()
        .returning(move |principal_id| {
            let cached = facts.cached_from.map(|from| CachedIdentity {
                principal_id: principal_id.to_string(),
                served_by: from.to_string(),
                identity: cached_text.clone(),
                fetched_at: Utc::now() - Duration::hours(2),
            });
            Box::pin(async move { Ok(cached) })
        });
    relay
        .expect_is_contact()
        .returning(|_, _| Box::pin(async { Ok(true) }));

    let mut peers = MockPeerService::new();
    peers.expect_pin().returning(|origin, _| {
        let pinned = server_at(origin).map(|server_id| PeerDto {
            server_id: server_id.to_string(),
            origin: origin.to_string(),
            status: "active".to_string(),
            head_seq: 0,
            descriptor_version: 1,
            descriptor_payload: "{}".to_string(),
            fetched_at: Utc::now(),
        });
        Box::pin(async move { pinned.ok_or(rim_services::ServiceError::Unavailable) })
    });
    peers.expect_resolve().returning(|server_id, _, _| {
        let lookup = match origin_of_server(server_id) {
            Some(origin) => PeerLookup::Ready(PeerKeysDto {
                server_id: server_id.to_string(),
                origin: origin.to_string(),
                head_seq: 0,
                head_key: "ed25519:k".to_string(),
                previous_key: None,
                s2s_endpoint: None,
            }),
            None => PeerLookup::Unknown,
        };
        Box::pin(async move { Ok(lookup) })
    });

    let mut client = MockS2sClient::new();
    let body = serde_json::to_vec(served).expect("json");
    let w = Arc::clone(&writes);
    client.expect_send().returning(move |request| {
        w.fetched_from
            .lock()
            .expect("lock")
            .push(request.peer_server_id.clone());
        let response = S2sResponse {
            status: 200,
            content_type: "application/json".to_string(),
            body: body.clone(),
            key_seq: 0,
            headers: Vec::new(),
        };
        Box::pin(async move { Ok(response) })
    });

    let service = DefaultHostService::new(
        HostRepos {
            host_repo: Arc::new(host),
            relay_repo: Arc::new(relay),
            key_identity_repo: Arc::new(keys),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(delegations),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        },
        None,
        server_identity(),
        Arc::new(peers),
        Arc::new(client),
        ServerId::new(),
        120,
    );
    (service, writes)
}

async fn push(facts: Facts, from: &str, doc: IdentityDocument) -> (IdentityPush, Arc<Writes>) {
    let (svc, writes) = host(facts, &doc);
    let origin = origin_of_server(from).expect("a known relay").to_string();
    let outcome = svc
        .apply_identity(from.to_string(), origin, doc)
        .await
        .expect("push");
    (outcome, writes)
}

#[tokio::test]
async fn a_relay_that_does_not_speak_for_the_user_changes_nothing() {
    let alice = Alice::new();
    let (outcome, writes) = push(FACTS, STRANGER, alice.document(false)).await;
    assert_eq!(outcome, IdentityPush::NotDelegated);
    assert!(writes.mls_active().is_empty());
    assert!(writes.cached_from().is_empty());
    assert!(writes.home_relay().is_empty());
}

#[tokio::test]
async fn the_relay_the_stored_card_lists_applies_device_activity() {
    let alice = Alice::new();
    let (outcome, writes) = push(FACTS, RELAY, alice.document(false)).await;
    assert_eq!(outcome, IdentityPush::Applied(true));
    assert_eq!(writes.mls_active(), vec![false]);
    assert_eq!(writes.cached_from(), vec![RELAY.to_string()]);
}

#[tokio::test]
async fn a_relay_only_a_newer_pushed_card_lists_may_push() {
    let alice = Alice::new();
    let (outcome, writes) = push(
        FACTS,
        STRANGER,
        alice.with_card(true, 2, &[STRANGER_ORIGIN]),
    )
    .await;
    assert_eq!(outcome, IdentityPush::Applied(true));
    assert_eq!(writes.mls_active(), vec![true]);
    assert_eq!(writes.cached_from(), vec![STRANGER.to_string()]);
    assert_eq!(writes.home_relay(), vec![Some(STRANGER.to_string())]);

    // A card of the stored version does not move the user to another relay.
    let (outcome, writes) = push(
        FACTS,
        STRANGER,
        alice.with_card(false, 1, &[STRANGER_ORIGIN]),
    )
    .await;
    assert_eq!(outcome, IdentityPush::NotDelegated);
    assert!(writes.mls_active().is_empty());
}

#[tokio::test]
async fn without_a_card_only_the_home_relay_may_push() {
    let alice = Alice::new();
    let cardless = Facts {
        stored_card: None,
        ..FACTS
    };
    let (outcome, writes) = push(cardless, STRANGER, alice.without_card(false)).await;
    assert_eq!(outcome, IdentityPush::NotDelegated);
    assert!(writes.home_relay().is_empty());
    assert!(writes.mls_active().is_empty());

    let (outcome, writes) = push(cardless, RELAY, alice.without_card(false)).await;
    assert_eq!(outcome, IdentityPush::Applied(true));
    assert_eq!(writes.home_relay(), vec![Some(RELAY.to_string())]);
    assert_eq!(writes.mls_active(), vec![false]);
}

#[tokio::test]
async fn a_relay_holding_a_delegation_may_push_and_the_delegated_home_relay_stays() {
    let alice = Alice::new();
    let delegated = Facts {
        delegated_relay: Some(STRANGER),
        home_relay: Some(STRANGER),
        ..FACTS
    };
    let (outcome, writes) = push(delegated, STRANGER, alice.document(false)).await;
    assert_eq!(outcome, IdentityPush::Applied(true));
    assert_eq!(writes.mls_active(), vec![false]);
    // The card lists another relay, but the delegated home relay is kept.
    assert_eq!(writes.home_relay(), vec![None]);
}

#[tokio::test]
async fn a_refresh_goes_to_the_card_relay_not_to_the_cached_source() {
    let alice = Alice::new();
    let doc = alice.document(true);
    let facts = Facts {
        cached_from: Some(STRANGER),
        home_relay: Some(STRANGER),
        ..FACTS
    };
    let (svc, writes) = host(facts, &doc);
    svc.refresh_remote(vec![doc.principal_id.clone()])
        .await
        .expect("refresh");
    assert_eq!(writes.fetched_from(), vec![RELAY.to_string()]);
    assert_eq!(writes.cached_from(), vec![RELAY.to_string()]);
}

#[tokio::test]
async fn a_presenting_relay_that_does_not_speak_for_the_user_is_not_stored() {
    let alice = Alice::new();
    let doc = alice.document(false);
    let (svc, writes) = host(FACTS, &doc);
    let stored = svc
        .refresh_identity(doc.principal_id.clone(), Some(STRANGER.to_string()))
        .await
        .expect("refresh");
    assert!(!stored);
    assert_eq!(writes.fetched_from(), vec![STRANGER.to_string()]);
    assert!(writes.mls_active().is_empty());
    assert!(writes.cached_from().is_empty());

    let stored = svc
        .refresh_identity(doc.principal_id.clone(), Some(RELAY.to_string()))
        .await
        .expect("refresh");
    assert!(stored);
    assert_eq!(writes.mls_active(), vec![false]);
}

#[tokio::test]
async fn a_relay_a_local_user_names_does_not_update_a_known_person_it_does_not_speak_for() {
    let alice = Alice::new();
    let doc = alice.document(false);
    let (svc, writes) = host(FACTS, &doc);
    let resolved = svc
        .ensure_remote_member(
            Actor::new(UserId::new()),
            RemoteMemberRef {
                principal_id: doc.principal_id.clone(),
                relay_urls: vec![STRANGER_ORIGIN.to_string()],
            },
        )
        .await
        .expect("the stored copy is used");
    assert!(resolved.remote);
    assert_eq!(writes.fetched_from(), vec![STRANGER.to_string()]);
    assert!(writes.mls_active().is_empty());
    assert!(writes.cached_from().is_empty());
}
