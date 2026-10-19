#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};
use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::model::{
    KeyChallengeId, KeyChallengePurpose, KeyLogEntryType, NewDeviceKey, NewKeyLogEntry,
};
use rim_core::key_identity::repository::{KeyChallengeRepository, KeyIdentityRepository};
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::key_identity::{
    SeaOrmKeyChallengeRepository, SeaOrmKeyIdentityRepository,
};
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use uuid::Uuid;

fn entry(seq: u32, entry_type: KeyLogEntryType) -> NewKeyLogEntry {
    NewKeyLogEntry {
        seq,
        entry_type,
        payload: format!(r#"{{"seq":{seq}}}"#),
        signature: format!("sig-{seq}"),
    }
}

/// A stand-in for the SHA-256 the service computes: distinct short keys give distinct values.
fn test_key_hash(public_key: &str) -> Vec<u8> {
    let mut hash = public_key.as_bytes().to_vec();
    hash.resize(32, 0);
    hash
}

fn device(user_id: UserId, device_id: Uuid, public_key: &str) -> NewDeviceKey {
    NewDeviceKey {
        user_id,
        device_id,
        public_key: public_key.to_string(),
        key_hash: test_key_hash(public_key),
        name: "Laptop".to_string(),
        cert_payload: "{}".to_string(),
        cert_signature: "cert-sig".to_string(),
        key_seq: 0,
    }
}

#[tokio::test]
async fn enroll_sets_principal_and_stores_the_log_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_user("bob").await;

    assert!(repo.lock_user(alice.id).await.unwrap());
    assert!(!repo.lock_user(UserId::new()).await.unwrap());

    repo.enroll(
        alice.id,
        "rimp1_alice",
        "identity-key-0",
        vec![entry(0, KeyLogEntryType::Inception)],
    )
    .await
    .unwrap();

    let stored = users.get_by_id(alice.id).await.unwrap().unwrap();
    assert_eq!(stored.principal_id.as_deref(), Some("rimp1_alice"));
    assert_eq!(
        repo.find_user_by_principal_id("rimp1_alice").await.unwrap(),
        Some(alice.id)
    );
    let log = repo.list_key_log(alice.id).await.unwrap();
    assert_eq!(log.len(), 1);
    assert_eq!(log[0].entry_type, KeyLogEntryType::Inception);

    // The same account again.
    let err = repo
        .enroll(
            alice.id,
            "rimp1_other",
            "identity-key-x",
            vec![entry(0, KeyLogEntryType::Inception)],
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    // Another account, the same principal.
    let err = repo
        .enroll(
            bob.id,
            "rimp1_alice",
            "identity-key-0",
            vec![entry(0, KeyLogEntryType::Inception)],
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
}

#[tokio::test]
async fn rotation_appends_in_order_and_rejects_a_taken_seq() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    repo.enroll(
        alice.id,
        "rimp1_alice",
        "identity-key-0",
        vec![entry(0, KeyLogEntryType::Inception)],
    )
    .await
    .unwrap();

    repo.append_rotation(
        alice.id,
        entry(1, KeyLogEntryType::Rotate),
        "identity-key-1",
    )
    .await
    .unwrap();
    let err = repo
        .append_rotation(
            alice.id,
            entry(1, KeyLogEntryType::Rotate),
            "identity-key-x",
        )
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    let seqs: Vec<u32> = repo
        .list_key_log(alice.id)
        .await
        .unwrap()
        .iter()
        .map(|e| e.seq)
        .collect();
    assert_eq!(seqs, vec![0, 1]);
}

#[tokio::test]
async fn devices_are_unique_per_user_and_revocation_is_kept_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_user("bob").await;
    let laptop = Uuid::now_v7();

    let stored = repo
        .add_device(device(alice.id, laptop, "device-key-a"))
        .await
        .unwrap();
    assert_eq!(stored.device_id, laptop);
    assert!(stored.revoked_at.is_none());

    for dup in [
        device(alice.id, laptop, "device-key-b"),
        device(alice.id, Uuid::now_v7(), "device-key-a"),
    ] {
        let err = repo.add_device(dup).await.unwrap_err();
        assert_eq!(err.kind(), RepoErrorKind::Conflict);
    }
    // Another user may hold the same device id and key.
    repo.add_device(device(bob.id, laptop, "device-key-a"))
        .await
        .unwrap();

    repo.touch_device(stored.id).await.unwrap();
    assert!(
        repo.revoke_device(stored.id, "rev-1", "sig-1")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .revoke_device(stored.id, "rev-2", "sig-2")
            .await
            .unwrap()
    );
    let again = repo.get_device(alice.id, laptop).await.unwrap().unwrap();
    assert!(again.revoked_at.is_some());
    assert!(again.last_used_at.is_some());
    assert_eq!(again.revocation_payload.as_deref(), Some("rev-1"));

    // A revoked device id is never reused.
    let err = repo
        .add_device(device(alice.id, laptop, "device-key-c"))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    let phone = repo
        .add_device(device(alice.id, Uuid::now_v7(), "device-key-d"))
        .await
        .unwrap();
    assert_eq!(repo.revoke_all_devices(alice.id).await.unwrap(), 1);
    let phone = repo
        .get_device(alice.id, phone.device_id)
        .await
        .unwrap()
        .unwrap();
    assert!(phone.revoked_at.is_some() && phone.revocation_payload.is_none());
    assert_eq!(repo.list_devices(alice.id).await.unwrap().len(), 2);
    // Bob's device is untouched.
    assert!(
        repo.get_device(bob.id, laptop)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
}

#[tokio::test]
async fn profile_card_keeps_the_newest_version() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;

    assert!(repo.get_profile_card(alice.id).await.unwrap().is_none());
    assert!(
        repo.upsert_profile_card(alice.id, 2, "card-2", "s2")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .upsert_profile_card(alice.id, 2, "card-2b", "s")
            .await
            .unwrap()
    );
    assert!(
        !repo
            .upsert_profile_card(alice.id, 1, "card-1", "s1")
            .await
            .unwrap()
    );
    assert_eq!(
        repo.get_profile_card(alice.id)
            .await
            .unwrap()
            .unwrap()
            .payload,
        "card-2"
    );
    assert!(
        repo.upsert_profile_card(alice.id, u32::MAX >> 1, "card-max", "s")
            .await
            .unwrap()
    );
    let card = repo.get_profile_card(alice.id).await.unwrap().unwrap();
    assert_eq!(card.version, u32::MAX >> 1);
    assert_eq!(card.payload, "card-max");
}

#[tokio::test]
async fn challenge_is_consumed_once_and_only_with_its_nonce() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyChallengeRepository::new(ctx.db.clone());
    let login = KeyChallengePurpose::Login;

    let c = repo
        .create("nonce-1", Utc::now() + Duration::minutes(2), &login)
        .await
        .unwrap();
    assert_eq!(c.purpose, login);
    assert!(!repo.consume(c.id, "wrong", &login).await.unwrap());
    assert!(
        !repo
            .consume(KeyChallengeId::new(), "nonce-1", &login)
            .await
            .unwrap()
    );
    assert!(repo.consume(c.id, "nonce-1", &login).await.unwrap());
    assert!(!repo.consume(c.id, "nonce-1", &login).await.unwrap());

    let expired = repo
        .create("nonce-2", Utc::now() - Duration::seconds(1), &login)
        .await
        .unwrap();
    assert!(!repo.consume(expired.id, "nonce-2", &login).await.unwrap());
    assert_eq!(repo.delete_expired(Utc::now()).await.unwrap(), 1);
}

#[tokio::test]
async fn challenge_is_consumed_only_for_its_purpose_and_relay() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmKeyChallengeRepository::new(ctx.db.clone());
    let login = KeyChallengePurpose::Login;
    let join = |relay: &str| KeyChallengePurpose::Join {
        relay_server_id: relay.to_string(),
    };
    let expires_at = Utc::now() + Duration::minutes(2);

    let joining = repo
        .create("nonce-j", expires_at, &join("rims1_relay"))
        .await
        .unwrap();
    assert_eq!(joining.purpose, join("rims1_relay"));
    assert!(!repo.consume(joining.id, "nonce-j", &login).await.unwrap());
    assert!(
        !repo
            .consume(joining.id, "nonce-j", &join("rims1_other"))
            .await
            .unwrap()
    );
    assert!(
        repo.consume(joining.id, "nonce-j", &join("rims1_relay"))
            .await
            .unwrap()
    );

    let signing_in = repo.create("nonce-l", expires_at, &login).await.unwrap();
    assert!(
        !repo
            .consume(signing_in.id, "nonce-l", &join("rims1_relay"))
            .await
            .unwrap()
    );
    assert!(
        repo.consume(signing_in.id, "nonce-l", &login)
            .await
            .unwrap()
    );
}

#[tokio::test]
async fn revoking_a_device_ends_only_its_sessions() {
    let ctx = support::TestCtx::new().await;
    let keys = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let sessions = SeaOrmSessionRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let laptop = keys
        .add_device(device(alice.id, Uuid::now_v7(), "device-key-a"))
        .await
        .unwrap();

    let new_session = |hash: &str| NewSession {
        user_id: alice.id,
        token_hash: hash.to_string(),
        expires_at: Utc::now() + Duration::hours(1),
        user_agent: None,
        ip: None,
    };
    let bound = sessions.create_session(new_session("bound")).await.unwrap();
    let other = sessions.create_session(new_session("other")).await.unwrap();
    sessions.bind_device(bound.id, laptop.id).await.unwrap();

    assert_eq!(sessions.revoke_all_for_device(laptop.id).await.unwrap(), 1);
    assert!(
        sessions
            .get_by_id(bound.id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_some()
    );
    assert!(
        sessions
            .get_by_id(other.id)
            .await
            .unwrap()
            .unwrap()
            .revoked_at
            .is_none()
    );
    assert_eq!(sessions.revoke_all_for_device(laptop.id).await.unwrap(), 0);
}
