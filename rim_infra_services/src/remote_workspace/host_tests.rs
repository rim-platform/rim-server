//! The host's rules for delegations a relay presents: the relay and host it names, the relay
//! key, expiry, the device, membership and bans, the per-relay limit, and the lifetime the host
//! grants.

use std::sync::{Arc, Mutex};

use chrono::{DateTime, Duration, Utc};
use rim_core::auth::repository::{MockInviteRepository, MockSessionRepository};
use rim_core::conversation::repository::MockConversationRepository;
use rim_core::conversation_member::repository::MockConversationMemberRepository;
use rim_core::dm_policy::repository::MockDmPolicyRepository;
use rim_core::e2ee::repository::MockE2eeRepository;
use rim_core::host::model::{DelegationStored, UserKind};
use rim_core::host::repository::{MockDelegationRepository, MockHostRepository};
use rim_core::key_identity::model::{DeviceKey, DeviceKeyId};
use rim_core::key_identity::repository::{MockKeyChallengeRepository, MockKeyIdentityRepository};
use rim_core::moderation::model::ServerBan;
use rim_core::moderation::repository::MockModerationRepository;
use rim_core::relay::repository::{MockRelayAbuseRepository, MockRelayRepository};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::{ServerMember, ServerMemberId, ServerMemberRole};
use rim_core::server_member::repository::MockServerMemberRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::MockUserRepository;
use rim_identity::testing::{device_delegation, signing_key};
use rim_identity::{Delegation, PublicKey, SignedStatement};
use rim_s2s::{
    Endpoints, Limits, Registration, RegistrationMode, Role, ServerDescriptor, ServerIdentity,
    ServerKeyState, ServerRotationSigner, ServerSigner, SharedIdentity, sign_descriptor,
    sign_inception, sign_rotation, timestamp,
};
use rim_services::ServiceError;
use rim_services::relay::dto::SignedJson;
use rim_services::remote_workspace::service::WorkspaceHostService;
use rim_services::s2s::service::{MockPeerService, MockS2sClient};
use uuid::Uuid;

use crate::host::{DefaultHostService, HostRepos};
use crate::remote_workspace::{DefaultWorkspaceHost, WorkspaceHostRepos};

const PRINCIPAL: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";

fn identity(seed: u8) -> (SharedIdentity, Vec<SignedStatement>) {
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
    let log = vec![inception.clone()];
    (
        SharedIdentity::new(ServerIdentity {
            signer,
            state,
            log: vec![inception],
            descriptor,
            descriptor_content: content,
        }),
        log,
    )
}

/// What the host's stores say about the person and the workspace.
#[derive(Clone)]
struct Facts {
    member: bool,
    banned: bool,
    revoked: bool,
    known_device: bool,
    active_for_relay: i64,
    /// The relay already holds an active delegation for the same workspace and device.
    renewing: bool,
    /// The relay's pinned log is followed by a recovery rotation.
    relay_recovered: bool,
}

impl Default for Facts {
    fn default() -> Self {
        Self {
            member: true,
            banned: false,
            revoked: false,
            known_device: true,
            active_for_relay: 0,
            renewing: false,
            relay_recovered: false,
        }
    }
}

struct Setup {
    host: DefaultWorkspaceHost,
    host_id: String,
    relay_id: String,
    relay_key_hash: String,
    workspace: ServerId,
    stored: Arc<Mutex<Vec<DateTime<Utc>>>>,
}

fn setup(facts: Facts) -> Setup {
    let (host_identity, _) = identity(40);
    let host_id = host_identity.current().server_id().to_string();
    let (relay_identity, mut relay_log) = identity(30);
    let relay_id = relay_identity.current().server_id().to_string();
    let relay_key_hash = rim_s2s::key_hash(&relay_identity.current().state.server_key);
    if facts.relay_recovered {
        let state = relay_identity.current().state.clone();
        relay_log.push(sign_rotation(
            &state,
            &ServerSigner::from_seed([130; 32]),
            ServerRotationSigner::Recovery,
            &ServerSigner::from_seed([31; 32]).public_key(),
            &ServerSigner::from_seed([131; 32]).public_key(),
            Utc::now(),
        ));
    }
    let workspace = ServerId::new();
    let user = UserId::new();

    let mut peers = MockPeerService::new();
    let pinned: Vec<SignedJson> = relay_log
        .iter()
        .map(|s| SignedJson {
            payload: s.payload.clone(),
            signature: s.signature.clone(),
        })
        .collect();
    peers.expect_pinned_key_log().returning(move |_| {
        let pinned = pinned.clone();
        Box::pin(async move { Ok(pinned) })
    });

    let mut host_repo = MockHostRepository::new();
    host_repo
        .expect_find_user()
        .returning(move |_| Box::pin(async move { Ok(Some((user, UserKind::Remote))) }));
    host_repo
        .expect_update_remote_user()
        .returning(|_, _, _| Box::pin(async { Ok(()) }));

    let mut keys = MockKeyIdentityRepository::new();
    let f = facts.clone();
    keys.expect_get_device()
        .returning(move |user_id, device_id| {
            let f = f.clone();
            Box::pin(async move {
                Ok(f.known_device.then(|| DeviceKey {
                    id: DeviceKeyId::new(),
                    user_id,
                    device_id,
                    public_key: PublicKey::from(&signing_key(20)).to_string(),
                    name: "Laptop".to_string(),
                    cert_payload: "{}".to_string(),
                    cert_signature: "ed25519:x".to_string(),
                    key_seq: 0,
                    added_at: Utc::now(),
                    last_used_at: None,
                    revoked_at: f.revoked.then(Utc::now),
                    revocation_payload: None,
                    revocation_signature: None,
                }))
            })
        });

    let mut members = MockServerMemberRepository::new();
    let member = facts.member;
    members
        .expect_get_member()
        .returning(move |server_id, user_id| {
            Ok(member.then(|| ServerMember {
                id: ServerMemberId::new(),
                server_id,
                user_id,
                member_role: ServerMemberRole::Member,
                joined_at: Utc::now(),
            }))
        });
    let mut moderation = MockModerationRepository::new();
    let banned = facts.banned;
    moderation
        .expect_get_active_ban()
        .returning(move |server_id, user_id, _| {
            Box::pin(async move {
                Ok(banned.then(|| ServerBan {
                    server_id,
                    user_id,
                    reason: None,
                    banned_by: None,
                    created_at: Utc::now(),
                    expires_at: None,
                }))
            })
        });

    let stored = Arc::new(Mutex::new(Vec::new()));
    let mut delegations = MockDelegationRepository::new();
    let active = facts.active_for_relay;
    delegations
        .expect_count_active_for_relay()
        .returning(move |_, _| Box::pin(async move { Ok(active) }));
    let renewing = facts.renewing;
    delegations
        .expect_is_active()
        .returning(move |_, _, _, _| Box::pin(async move { Ok(renewing) }));
    let record = Arc::clone(&stored);
    delegations.expect_store().returning(move |d, _| {
        record.lock().expect("lock").push(d.expires_at);
        Box::pin(async move {
            Ok(DelegationStored::Stored {
                expires_at: d.expires_at,
            })
        })
    });

    // A host service that knows nobody new: refreshing an identity finds nothing.
    let mut inner_host_repo = MockHostRepository::new();
    inner_host_repo
        .expect_find_user()
        .returning(|_| Box::pin(async { Ok(None) }));
    let inner = Arc::new(DefaultHostService::new(
        HostRepos {
            host_repo: Arc::new(inner_host_repo),
            relay_repo: Arc::new(MockRelayRepository::new()),
            key_identity_repo: Arc::new(MockKeyIdentityRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
            user_repo: Arc::new(MockUserRepository::new()),
            conversation_repo: Arc::new(MockConversationRepository::new()),
            conversation_member_repo: Arc::new(MockConversationMemberRepository::new()),
            server_member_repo: Arc::new(MockServerMemberRepository::new()),
            delegation_repo: Arc::new(MockDelegationRepository::new()),
            relay_abuse_repo: Arc::new(MockRelayAbuseRepository::new()),
            dm_policy_repo: Arc::new(MockDmPolicyRepository::new()),
        },
        None,
        host_identity.clone(),
        Arc::new(MockPeerService::new()),
        Arc::new(MockS2sClient::new()),
        ServerId::new(),
        120,
    ));
    let host = DefaultWorkspaceHost::new(
        WorkspaceHostRepos {
            host_repo: Arc::new(host_repo),
            delegation_repo: Arc::new(delegations),
            key_identity_repo: Arc::new(keys),
            key_challenge_repo: Arc::new(MockKeyChallengeRepository::new()),
            invite_repo: Arc::new(MockInviteRepository::new()),
            moderation_repo: Arc::new(moderation),
            server_member_repo: Arc::new(members),
            session_repo: Arc::new(MockSessionRepository::new()),
            e2ee_repo: Arc::new(MockE2eeRepository::new()),
        },
        host_identity,
        Arc::new(peers),
        inner,
        10_000,
    );
    Setup {
        host,
        host_id,
        relay_id,
        relay_key_hash,
        workspace,
        stored,
    }
}

impl Setup {
    fn delegation(&self, change: impl FnOnce(&mut Delegation)) -> SignedJson {
        let now = Utc::now();
        let mut d = Delegation {
            principal_id: PRINCIPAL.to_string(),
            device_id: Uuid::from_u128(1),
            relay: self.relay_id.clone(),
            relay_key_hash: self.relay_key_hash.clone(),
            host: self.host_id.clone(),
            workspace_id: self.workspace.into(),
            nonce: "cHFyc3R1dnd4eXp7fH1-fw".to_string(),
            issued_at: now - Duration::hours(1),
            expires_at: now + Duration::hours(12),
        };
        change(&mut d);
        let signed = device_delegation(&signing_key(20), &d);
        SignedJson {
            payload: signed.payload,
            signature: signed.signature,
        }
    }

    /// Present one delegation as the relay; returns its status and reason.
    async fn present(&self, delegation: SignedJson) -> (String, Option<String>) {
        let mut results = self
            .host
            .subscribe(self.relay_id.clone(), vec![delegation])
            .await
            .expect("subscribe");
        assert_eq!(results.len(), 1);
        let result = results.remove(0);
        (result.status, result.reason)
    }
}

fn refused(reason: &str) -> (String, Option<String>) {
    ("refused".to_string(), Some(reason.to_string()))
}

#[tokio::test]
async fn a_valid_delegation_is_kept_until_its_expiry() {
    let s = setup(Facts::default());
    let d = s.delegation(|_| {});
    assert_eq!(s.present(d).await, ("active".to_string(), None));
    let stored = s.stored.lock().expect("lock").clone();
    assert_eq!(stored.len(), 1);
    let left = stored[0] - Utc::now();
    assert!(left > Duration::hours(11) && left <= Duration::hours(12));
}

#[tokio::test]
async fn the_host_keeps_one_for_at_most_a_day_from_acceptance() {
    let s = setup(Facts::default());
    // Issued an hour ahead of this host's clock: 24 hours from issue is 25 from now.
    let d = s.delegation(|d| {
        d.issued_at = Utc::now() + Duration::hours(1);
        d.expires_at = d.issued_at + Duration::hours(24);
    });
    assert_eq!(s.present(d).await.0, "active");
    let stored = s.stored.lock().expect("lock")[0];
    assert!(stored - Utc::now() <= Duration::hours(24));
}

#[tokio::test]
async fn another_relay_host_or_relay_key_is_refused() {
    let s = setup(Facts::default());
    let other_relay = s.delegation(|d| d.relay = s.host_id.clone());
    assert_eq!(s.present(other_relay).await, refused("wrong_relay"));
    let other_host = s.delegation(|d| d.host = s.relay_id.clone());
    assert_eq!(s.present(other_host).await, refused("wrong_host"));
    let other_key = s.delegation(|d| d.relay_key_hash = "x".repeat(43));
    assert_eq!(s.present(other_key).await, refused("relay_key"));
    assert!(s.stored.lock().expect("lock").is_empty());
}

#[tokio::test]
async fn a_relay_key_from_before_a_recovery_is_refused() {
    let s = setup(Facts {
        relay_recovered: true,
        ..Facts::default()
    });
    let d = s.delegation(|_| {});
    assert_eq!(s.present(d).await, refused("relay_key"));
}

#[tokio::test]
async fn expired_over_long_and_malformed_delegations_are_refused() {
    let s = setup(Facts::default());
    let expired = s.delegation(|d| {
        d.issued_at = Utc::now() - Duration::hours(10);
        d.expires_at = Utc::now() - Duration::hours(1);
    });
    assert_eq!(s.present(expired).await, refused("expired"));
    let long = s.delegation(|d| d.expires_at = d.issued_at + Duration::hours(25));
    assert_eq!(s.present(long).await, refused("invalid_delegation"));
    let mut forged = s.delegation(|_| {});
    forged.signature = s
        .delegation(|d| d.nonce = "AAAAAAAAAAAAAAAAAAAAAA".into())
        .signature;
    assert_eq!(s.present(forged).await, refused("bad_signature"));
}

#[tokio::test]
async fn members_only_unbanned_and_with_a_live_device() {
    let not_member = setup(Facts {
        member: false,
        ..Facts::default()
    });
    assert_eq!(
        not_member.present(not_member.delegation(|_| {})).await,
        refused("not_member")
    );
    let banned = setup(Facts {
        banned: true,
        ..Facts::default()
    });
    assert_eq!(
        banned.present(banned.delegation(|_| {})).await,
        refused("banned")
    );
    let revoked = setup(Facts {
        revoked: true,
        ..Facts::default()
    });
    assert_eq!(
        revoked.present(revoked.delegation(|_| {})).await,
        refused("revoked_device")
    );
    let unknown = setup(Facts {
        known_device: false,
        ..Facts::default()
    });
    assert_eq!(
        unknown.present(unknown.delegation(|_| {})).await,
        refused("unknown_device")
    );
}

#[tokio::test]
async fn a_relay_at_its_limit_is_refused_and_a_request_holds_64() {
    let s = setup(Facts {
        active_for_relay: 10_000,
        ..Facts::default()
    });
    assert_eq!(s.present(s.delegation(|_| {})).await, refused("limited"));
    let too_many = vec![s.delegation(|_| {}); 65];
    assert!(
        s.host
            .subscribe(s.relay_id.clone(), too_many)
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_renewal_at_the_limit_replaces_its_row_without_counting() {
    let s = setup(Facts {
        active_for_relay: 10_000,
        renewing: true,
        ..Facts::default()
    });
    assert_eq!(
        s.present(s.delegation(|_| {})).await,
        ("active".to_string(), None)
    );
    assert_eq!(s.stored.lock().expect("lock").len(), 1);
}

#[tokio::test]
async fn revocations_brought_directly_are_limited_per_address() {
    let s = setup(Facts::default());
    let junk = SignedJson {
        payload: "{}".to_string(),
        signature: "ed25519:x".to_string(),
    };
    for _ in 0..20 {
        assert!(matches!(
            s.host
                .apply_revocation_directly("198.51.100.7".to_string(), junk.clone())
                .await,
            Err(ServiceError::Validation(_))
        ));
    }
    assert!(matches!(
        s.host
            .apply_revocation_directly("198.51.100.7".to_string(), junk.clone())
            .await,
        Err(ServiceError::RateLimited { .. })
    ));
    assert!(matches!(
        s.host
            .apply_revocation_directly("198.51.100.8".to_string(), junk)
            .await,
        Err(ServiceError::Validation(_))
    ));
}
