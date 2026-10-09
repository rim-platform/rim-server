#![cfg(feature = "db-tests")]

//! End-to-end encryption storage: key packages, the one-Commit-per-epoch locking, encrypted
//! envelopes in the change feed, Welcomes, re-add requests, retention, cascades and the
//! migration's `down` guard.
//!
//! Destructive for the shared test database, like every db-test: run single-threaded.

mod support;

use std::collections::HashSet;

use chrono::{Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use migration::{Migrator, MigratorTrait};
use rim_core::conversation::model::{Conversation, ConversationId, ConversationType};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::e2ee::model::{
    MlsRetention, NewEncryptedMessage, NewMlsCommit, NewMlsGroup, NewMlsKeyPackage,
    NewMlsReaddRequest, NewMlsWelcome,
};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::key_identity::model::{DeviceKeyId, NewDeviceKey};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::message::model::{MessageEncryption, MessageId};
use rim_core::message::repository::MessageRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::Server;
use rim_core::user::model::{User, UserId};
use rim_infra_db::begin_transaction;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::e2ee::SeaOrmE2eeRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;

const MIGRATION_NAME: &str = "m20261009_182741_add_e2ee_mls";
const SUITE: i32 = 79;

struct Member {
    user: User,
    device: DeviceKeyId,
    device_id: Uuid,
}

struct Fixture {
    ctx: support::TestCtx,
    repo: SeaOrmE2eeRepository,
    server: Server,
    conv: Conversation,
    alice: Member,
    bob: Member,
}

async fn add_device(ctx: &support::TestCtx, user: &User, tag: &str) -> (DeviceKeyId, Uuid) {
    let keys = SeaOrmKeyIdentityRepository::new(ctx.db.clone());
    let device_id = Uuid::now_v7();
    let public_key = format!("ed25519:{tag}");
    let mut key_hash = public_key.as_bytes().to_vec();
    key_hash.resize(32, 0);
    let device = keys
        .add_device(NewDeviceKey {
            user_id: user.id,
            device_id,
            public_key,
            key_hash,
            name: "Laptop".to_string(),
            cert_payload: "{}".to_string(),
            cert_signature: "sig".to_string(),
            key_seq: 0,
        })
        .await
        .expect("add device");
    (device.id, device_id)
}

async fn member(ctx: &support::TestCtx, name: &str) -> Member {
    let user = ctx
        .create_key_only_user(name, &format!("rimp1_{name}"))
        .await;
    let (device, device_id) = add_device(ctx, &user, name).await;
    SeaOrmE2eeRepository::new(ctx.db.clone())
        .touch_key_package_upload(device)
        .await
        .expect("touch");
    Member {
        user,
        device,
        device_id,
    }
}

async fn join(ctx: &support::TestCtx, conv: ConversationId, user: UserId) {
    SeaOrmConversationMemberRepository::new(ctx.db.clone())
        .add_member(conv, user)
        .await
        .expect("add member");
}

async fn fixture() -> Fixture {
    let ctx = support::TestCtx::new().await;
    let server = ctx.create_server("s").await;
    let conv = ctx
        .create_conversation(server.id, ConversationType::Direct)
        .await;
    let alice = member(&ctx, "alice").await;
    let bob = member(&ctx, "bob").await;
    join(&ctx, conv.id, alice.user.id).await;
    join(&ctx, conv.id, bob.user.id).await;
    let repo = SeaOrmE2eeRepository::new(ctx.db.clone());
    Fixture {
        ctx,
        repo,
        server,
        conv,
        alice,
        bob,
    }
}

fn key_package(owner: &Member, tag: u8, last_resort: bool, days: i64) -> NewMlsKeyPackage {
    NewMlsKeyPackage {
        user_id: owner.user.id,
        device_key_id: owner.device,
        cipher_suite: SUITE,
        key_package_ref: vec![tag; 32],
        key_package: vec![tag; 64],
        last_resort,
        not_after: Utc::now() + Duration::days(days),
    }
}

fn group_id(conv: ConversationId, incarnation: u32) -> Vec<u8> {
    let mut id = b"rim1".to_vec();
    id.extend_from_slice(Uuid::from(conv).as_bytes());
    id.extend_from_slice(&incarnation.to_be_bytes());
    id
}

async fn create_group(f: &Fixture) {
    f.repo
        .insert_group(NewMlsGroup {
            conversation_id: f.conv.id,
            incarnation: 1,
            group_id: group_id(f.conv.id, 1),
            cipher_suite: SUITE,
            epoch: 1,
            created_by_device_key_id: Some(f.alice.device),
        })
        .await
        .expect("group");
    f.repo
        .add_group_devices(
            f.conv.id,
            1,
            0,
            &[
                (f.alice.device, f.alice.user.id),
                (f.bob.device, f.bob.user.id),
            ],
        )
        .await
        .expect("devices");
}

fn envelope(epoch: i64, device: DeviceKeyId, ciphertext: &[u8], key_id: i32) -> MessageEncryption {
    MessageEncryption {
        incarnation: 1,
        epoch,
        sender_device_key_id: Some(device),
        ciphertext: Some(ciphertext.to_vec()),
        franking_commitment: vec![1; 32],
        franking_receipt: vec![2; 32],
        franking_key_id: key_id,
    }
}

async fn insert_message(
    repo: &SeaOrmE2eeRepository,
    f: &Fixture,
    epoch: i64,
    ciphertext: &[u8],
) -> rim_core::message::model::Message {
    let key = repo.current_franking_key(vec![9; 32]).await.expect("key");
    repo.insert_encrypted_message(NewEncryptedMessage {
        id: MessageId::new(),
        conversation_id: f.conv.id,
        sent_by: f.alice.user.id,
        thread_id: None,
        forwarded_from_message_id: None,
        sent_at: Utc::now(),
        envelope: envelope(epoch, f.alice.device, ciphertext, key.id),
    })
    .await
    .expect("insert message")
}

fn commit(f: &Fixture, epoch: i64, body: u8) -> NewMlsCommit {
    NewMlsCommit {
        conversation_id: f.conv.id,
        incarnation: 1,
        epoch,
        sender_user_id: f.alice.user.id,
        sender_device_key_id: f.alice.device,
        message: vec![body; 16],
    }
}

#[tokio::test]
async fn key_packages_are_claimed_oldest_first_then_the_last_resort() {
    let f = fixture().await;
    for tag in 1..=3 {
        f.repo
            .insert_key_package(key_package(&f.bob, tag, false, 30))
            .await
            .unwrap();
    }
    f.repo
        .insert_key_package(key_package(&f.bob, 9, true, 30))
        .await
        .unwrap();
    let expired = f
        .repo
        .insert_key_package(key_package(&f.bob, 8, false, 30))
        .await
        .unwrap();
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE mls_key_package SET not_after = now() - interval '1 minute' WHERE id = $1",
            [Uuid::from(expired).into()],
        ))
        .await
        .unwrap();

    assert_eq!(
        f.repo
            .count_available_key_packages(f.bob.device, SUITE)
            .await
            .unwrap(),
        3
    );
    let status = f.repo.key_package_status(f.bob.device).await.unwrap();
    assert_eq!(status.len(), 1);
    assert_eq!(status[0].available, 3);
    assert!(status[0].last_resort_not_after.is_some());
    assert!(status[0].oldest_not_after.is_some());

    let duplicate = f
        .repo
        .insert_key_package(key_package(&f.alice, 1, false, 30))
        .await
        .unwrap_err();
    assert_eq!(duplicate.kind(), RepoErrorKind::Conflict);

    let mut claimed = Vec::new();
    for _ in 0..3 {
        let package = f
            .repo
            .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
            .await
            .unwrap()
            .expect("one-time package");
        assert!(!package.last_resort);
        claimed.push(package.key_package_ref[0]);
    }
    assert_eq!(claimed, vec![1, 2, 3], "oldest first");
    assert!(
        f.repo
            .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
            .await
            .unwrap()
            .is_none(),
        "expired packages are never handed out"
    );
    let last_resort = f
        .repo
        .claim_last_resort_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
        .await
        .unwrap()
        .expect("last resort");
    assert!(last_resort.last_resort);
    // The last-resort package stays and can be handed out again.
    assert!(
        f.repo
            .claim_last_resort_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
            .await
            .unwrap()
            .is_some()
    );

    let found = f
        .repo
        .find_key_packages_by_refs(&[vec![1; 32], vec![9; 32], vec![7; 32]])
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    let one_time = found.iter().find(|p| !p.last_resort).expect("one-time");
    assert_eq!(one_time.claimed_by_device_key_id, Some(f.alice.device));
    assert_eq!(one_time.claimed_for_conversation_id, Some(f.conv.id));
    assert!(one_time.claimed_at.is_some());

    let ids: Vec<_> = found.iter().map(|p| p.id).collect();
    assert_eq!(
        f.repo.consume_key_packages(&ids).await.unwrap(),
        1,
        "the last-resort package is not consumed"
    );
}

#[tokio::test]
async fn a_device_has_one_last_resort_package_per_suite() {
    let f = fixture().await;
    f.repo
        .insert_key_package(key_package(&f.bob, 1, true, 30))
        .await
        .unwrap();
    let second = f
        .repo
        .insert_key_package(key_package(&f.bob, 2, true, 30))
        .await
        .unwrap_err();
    assert_eq!(second.kind(), RepoErrorKind::Conflict);
    // Another suite, and another device, each have their own.
    let mut other_suite = key_package(&f.bob, 3, true, 30);
    other_suite.cipher_suite = 1;
    f.repo.insert_key_package(other_suite).await.unwrap();
    f.repo
        .insert_key_package(key_package(&f.alice, 4, true, 30))
        .await
        .unwrap();
    // Replacing is delete then insert.
    assert_eq!(
        f.repo
            .delete_last_resort_key_package(f.bob.device, SUITE)
            .await
            .unwrap(),
        1
    );
    f.repo
        .insert_key_package(key_package(&f.bob, 2, true, 30))
        .await
        .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_claims_never_hand_out_a_package_twice() {
    let f = fixture().await;
    for tag in 1..=10 {
        f.repo
            .insert_key_package(key_package(&f.bob, tag, false, 30))
            .await
            .unwrap();
    }
    let conn = support::db_conn().await;
    let mut tasks = Vec::new();
    for _ in 0..16 {
        let conn = conn.clone();
        let (device, claimer, conv) = (f.bob.device, f.alice.device, f.conv.id);
        tasks.push(tokio::spawn(async move {
            let scope = begin_transaction(&conn).await.unwrap();
            let repo = SeaOrmE2eeRepository::new(scope.handle());
            let claimed = repo
                .claim_one_time_key_package(device, SUITE, claimer, conv)
                .await
                .unwrap();
            // Hold the row lock a moment so the claims overlap.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            scope.commit().await.unwrap();
            claimed.map(|p| p.id)
        }));
    }
    let mut ids = Vec::new();
    for task in tasks {
        ids.extend(task.await.unwrap());
    }
    assert_eq!(ids.len(), 10, "every package is handed out");
    assert_eq!(
        ids.iter().collect::<HashSet<_>>().len(),
        10,
        "and none twice"
    );
}

/// The service's Commit transaction, reduced to its storage steps: lock the group, check the
/// epoch, insert the Commit, advance. `Err(current)` is `STALE_EPOCH`.
async fn try_commit(
    conn: &sea_orm::DatabaseConnection,
    commit: NewMlsCommit,
    hold: std::time::Duration,
) -> Result<i64, i64> {
    let scope = begin_transaction(conn).await.unwrap();
    let repo = SeaOrmE2eeRepository::new(scope.handle());
    let group = repo
        .lock_current_group(commit.conversation_id, GroupLock::Update)
        .await
        .unwrap()
        .expect("group");
    if group.epoch != commit.epoch {
        return Err(group.epoch);
    }
    let (conv, incarnation) = (commit.conversation_id, commit.incarnation);
    match repo.insert_commit(commit).await {
        Ok(_) => {}
        Err(e) if e.kind() == RepoErrorKind::Conflict => return Err(group.epoch),
        Err(e) => panic!("{e:?}"),
    }
    let epoch = repo.advance_epoch(conv, incarnation).await.unwrap();
    tokio::time::sleep(hold).await;
    scope.commit().await.unwrap();
    Ok(epoch)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn two_commits_for_one_epoch_one_wins_the_other_is_stale() {
    let f = fixture().await;
    create_group(&f).await;
    let conn = support::db_conn().await;

    let first = {
        let conn = conn.clone();
        let commit = commit(&f, 1, 0xa);
        tokio::spawn(async move {
            try_commit(&conn, commit, std::time::Duration::from_millis(300)).await
        })
    };
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    let second = try_commit(&conn, commit(&f, 1, 0xb), std::time::Duration::ZERO).await;
    let first = first.await.unwrap();

    assert_eq!(first, Ok(2));
    assert_eq!(second, Err(2), "the loser sees the new epoch");
    let commits = f.repo.list_commits(f.conv.id, 1, None, 10).await.unwrap();
    assert_eq!(commits.len(), 1);
    assert_eq!(commits[0].message, vec![0xa; 16]);
    assert_eq!(commits[0].sender_device_id, Some(f.alice.device_id));
    assert_eq!(
        commits[0].sender_principal_id.as_deref(),
        Some("rimp1_alice")
    );
}

#[tokio::test]
async fn the_unique_index_backs_up_the_lock() {
    let f = fixture().await;
    create_group(&f).await;
    f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    let again = f.repo.insert_commit(commit(&f, 1, 2)).await.unwrap_err();
    assert_eq!(again.kind(), RepoErrorKind::Conflict);

    f.repo.insert_commit(commit(&f, 2, 3)).await.unwrap();
    f.repo.insert_commit(commit(&f, 3, 4)).await.unwrap();
    let after = f
        .repo
        .list_commits(f.conv.id, 1, Some(1), 10)
        .await
        .unwrap();
    assert_eq!(
        after.iter().map(|c| c.epoch).collect::<Vec<_>>(),
        vec![2, 3]
    );
    let page = f.repo.list_commits(f.conv.id, 1, None, 2).await.unwrap();
    assert_eq!(page.iter().map(|c| c.epoch).collect::<Vec<_>>(), vec![1, 2]);
    let earlier = Utc::now() - Duration::minutes(1);
    assert!(
        f.repo
            .commit_accepted_since(f.conv.id, 1, earlier)
            .await
            .unwrap()
    );
    assert!(
        !f.repo
            .commit_accepted_since(f.conv.id, 1, Utc::now() + Duration::minutes(1))
            .await
            .unwrap()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_message_racing_a_commit_lands_in_epoch_order() {
    let f = fixture().await;
    create_group(&f).await;
    let conn = support::db_conn().await;

    // A message in epoch 1 holds the group row FOR SHARE, uncommitted.
    let message_scope = begin_transaction(&conn).await.unwrap();
    let message_repo = SeaOrmE2eeRepository::new(message_scope.handle());
    let group = message_repo
        .lock_current_group(f.conv.id, GroupLock::Share)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(group.epoch, 1);
    let message = insert_message(&message_repo, &f, 1, b"in epoch 1").await;

    // The Commit ending epoch 1 waits for it.
    let committer = {
        let conn = conn.clone();
        let commit = commit(&f, 1, 7);
        tokio::spawn(async move { try_commit(&conn, commit, std::time::Duration::ZERO).await })
    };
    tokio::time::sleep(std::time::Duration::from_millis(300)).await;
    assert!(!committer.is_finished(), "the Commit waits for the message");
    message_scope.commit().await.unwrap();
    assert_eq!(committer.await.unwrap(), Ok(2));

    // A message still naming epoch 1 now finds the group at epoch 2.
    let late = f
        .repo
        .lock_current_group(f.conv.id, GroupLock::Share)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(late.epoch, 2);

    let feed = SeaOrmMessageRepository::new(f.ctx.db.clone())
        .list_message_changes(f.conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].message.id, message.id);
    let commits = f.repo.list_commits(f.conv.id, 1, None, 10).await.unwrap();
    assert!(
        commits[0].created_at >= feed[0].message.sent_at,
        "the Commit committed after the message"
    );
}

#[tokio::test]
async fn an_edit_bumps_the_version_and_a_delete_clears_the_ciphertext() {
    let f = fixture().await;
    create_group(&f).await;
    let messages = SeaOrmMessageRepository::new(f.ctx.db.clone());
    let sent = insert_message(&f.repo, &f, 1, b"first").await;
    let encryption = sent.encryption.clone().expect("encrypted");
    assert_eq!(sent.text, None);
    assert_eq!(encryption.ciphertext.as_deref(), Some(&b"first"[..]));
    assert_eq!(encryption.epoch, 1);

    let replaced = f
        .repo
        .replace_envelope(sent.id, envelope(2, f.bob.device, b"edited", 1))
        .await
        .unwrap();
    let encryption = replaced.encryption.expect("encrypted");
    assert_eq!(encryption.ciphertext.as_deref(), Some(&b"edited"[..]));
    assert_eq!(encryption.epoch, 2);
    assert_eq!(encryption.sender_device_key_id, Some(f.bob.device));

    let feed = messages
        .list_message_changes(f.conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].version, 2);
    assert!(feed[0].edited_at.is_some(), "an envelope change is an edit");

    messages.soft_delete_message(sent.id).await.unwrap();
    let feed = messages
        .list_message_changes(f.conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(feed[0].version, 3);
    assert!(feed[0].message.is_deleted);
    let encryption = feed[0].message.encryption.clone().expect("envelope kept");
    assert_eq!(encryption.ciphertext, None, "the ciphertext is gone");
    assert_eq!(encryption.franking_receipt, vec![2; 32], "franking is kept");

    // An encrypted row never carries text.
    let err = f
        .ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE message SET text = 'leak' WHERE id = $1",
            [Uuid::from(sent.id).into()],
        ))
        .await;
    assert!(err.is_err());

    // A plaintext row cannot have its envelope replaced.
    let plain = f
        .ctx
        .create_message(f.conv.id, f.alice.user.id, "plain")
        .await;
    let err = f
        .repo
        .replace_envelope(plain.id, envelope(2, f.bob.device, b"x", 1))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::NotFound);

    // A repeated id is a conflict.
    let key = f.repo.current_franking_key(vec![0; 32]).await.unwrap();
    assert_eq!(key.id, 1);
    assert_eq!(key.secret, vec![9; 32], "the first key stays");
    let dup = f
        .repo
        .insert_encrypted_message(NewEncryptedMessage {
            id: sent.id,
            conversation_id: f.conv.id,
            sent_by: f.alice.user.id,
            thread_id: None,
            forwarded_from_message_id: None,
            sent_at: Utc::now(),
            envelope: envelope(2, f.alice.device, b"again", key.id),
        })
        .await
        .unwrap_err();
    assert_eq!(dup.kind(), RepoErrorKind::Conflict);
}

#[tokio::test]
async fn groups_devices_and_readiness() {
    let f = fixture().await;
    assert!(f.repo.current_group(f.conv.id).await.unwrap().is_none());
    create_group(&f).await;
    let group = f.repo.current_group(f.conv.id).await.unwrap().unwrap();
    assert_eq!(group.epoch, 1);
    assert_eq!(f.repo.advance_epoch(f.conv.id, 1).await.unwrap(), 2);

    // One current group per conversation.
    let second = f
        .repo
        .insert_group(NewMlsGroup {
            conversation_id: f.conv.id,
            incarnation: 2,
            group_id: group_id(f.conv.id, 2),
            cipher_suite: SUITE,
            epoch: 1,
            created_by_device_key_id: None,
        })
        .await
        .unwrap_err();
    assert_eq!(second.kind(), RepoErrorKind::Conflict);
    f.repo.retire_group(f.conv.id, 1).await.unwrap();
    f.repo
        .insert_group(NewMlsGroup {
            conversation_id: f.conv.id,
            incarnation: 2,
            group_id: group_id(f.conv.id, 2),
            cipher_suite: SUITE,
            epoch: 1,
            created_by_device_key_id: None,
        })
        .await
        .unwrap();
    assert_eq!(
        f.repo
            .current_group(f.conv.id)
            .await
            .unwrap()
            .unwrap()
            .incarnation,
        2
    );
    assert!(f.repo.group_devices(f.conv.id, 2).await.unwrap().is_empty());
    let old = f.repo.group_devices(f.conv.id, 1).await.unwrap();
    assert_eq!(old.len(), 2);

    // Desired devices: unrevoked, active, of current members.
    let active_since = Utc::now() - Duration::days(60);
    let (laptop, _) = add_device(&f.ctx, &f.bob.user, "bob-laptop").await;
    let desired = f
        .repo
        .desired_devices(f.conv.id, active_since, SUITE)
        .await
        .unwrap();
    assert_eq!(
        desired.len(),
        2,
        "a device that never uploaded is not desired"
    );
    f.repo.touch_key_package_upload(laptop).await.unwrap();
    f.repo
        .insert_key_package(key_package(&f.bob, 5, true, 30))
        .await
        .unwrap();
    let desired = f
        .repo
        .desired_devices(f.conv.id, active_since, SUITE)
        .await
        .unwrap();
    assert_eq!(desired.len(), 3);
    let bob_phone = desired
        .iter()
        .find(|d| d.device.device_key_id == f.bob.device)
        .unwrap();
    assert!(bob_phone.has_key_package);
    assert_eq!(bob_phone.device.principal_id, "rimp1_bob");
    assert!(
        !desired
            .iter()
            .find(|d| d.device.device_key_id == laptop)
            .unwrap()
            .has_key_package
    );

    let carol = f.ctx.create_user("carol").await;
    let ready = f
        .repo
        .ready_users(&[f.alice.user.id, carol.id], active_since)
        .await
        .unwrap();
    assert_eq!(ready, vec![f.alice.user.id]);
    assert!(
        f.repo
            .ready_users(&[f.alice.user.id], Utc::now() + Duration::minutes(1))
            .await
            .unwrap()
            .is_empty(),
        "an upload before the window does not count"
    );

    let members = f
        .repo
        .members_by_principal(
            f.conv.id,
            &["rimp1_bob".to_string(), "rimp1_nobody".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(members, vec![("rimp1_bob".to_string(), f.bob.user.id)]);
    assert!(f.repo.mark_conversation_encrypted(f.conv.id).await.unwrap());
    assert!(!f.repo.mark_conversation_encrypted(f.conv.id).await.unwrap());

    let found = f
        .repo
        .find_device("rimp1_alice", f.alice.device_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(found.device_key_id, f.alice.device);
    assert!(found.last_key_package_upload_at.is_some());
    assert!(f.repo.lock_device(f.alice.device).await.unwrap());
    assert!(!f.repo.lock_device(DeviceKeyId::new()).await.unwrap());
}

#[tokio::test]
async fn principals_are_visible_only_to_workspace_peers() {
    let f = fixture().await;
    let members = rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository::new(
        f.ctx.db.clone(),
    );
    use rim_core::server_member::repository::ServerMemberRepository;
    members
        .add_member(
            f.server.id,
            f.alice.user.id,
            rim_core::server_member::model::ServerMemberRole::Member,
        )
        .await
        .unwrap();
    members
        .add_member(
            f.server.id,
            f.bob.user.id,
            rim_core::server_member::model::ServerMemberRole::Member,
        )
        .await
        .unwrap();
    let stranger = member(&f.ctx, "stranger").await;
    let ids = [
        "rimp1_bob".to_string(),
        "rimp1_stranger".to_string(),
        "rimp1_alice".to_string(),
    ];
    let mut visible = f
        .repo
        .principals_visible_to(f.alice.user.id, &ids)
        .await
        .unwrap();
    visible.sort_by(|a, b| a.0.cmp(&b.0));
    assert_eq!(
        visible.iter().map(|(p, _)| p.as_str()).collect::<Vec<_>>(),
        vec!["rimp1_alice", "rimp1_bob"]
    );
    let own = f
        .repo
        .principals_visible_to(stranger.user.id, &ids)
        .await
        .unwrap();
    assert_eq!(own, vec![("rimp1_stranger".to_string(), stranger.user.id)]);
}

#[tokio::test]
async fn welcomes_wait_for_their_devices() {
    let f = fixture().await;
    create_group(&f).await;
    let commit = f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    let welcome = f
        .repo
        .insert_welcome(NewMlsWelcome {
            commit_id: commit.id,
            conversation_id: f.conv.id,
            incarnation: 1,
            epoch: 2,
            message: vec![5; 8],
            expires_at: Utc::now() + Duration::days(30),
            recipients: vec![(f.bob.device, f.bob.user.id)],
        })
        .await
        .unwrap();
    let pending = f.repo.pending_welcomes(f.bob.device).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, welcome);
    assert_eq!(pending[0].server_id, f.server.id);
    assert_eq!(pending[0].epoch, 2);
    assert!(
        f.repo
            .pending_welcomes(f.alice.device)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        f.repo
            .ack_welcomes(f.alice.device, &[welcome])
            .await
            .unwrap(),
        0
    );
    assert_eq!(
        f.repo.ack_welcomes(f.bob.device, &[welcome]).await.unwrap(),
        1
    );
    assert_eq!(
        f.repo.ack_welcomes(f.bob.device, &[welcome]).await.unwrap(),
        0
    );
    assert!(
        f.repo
            .pending_welcomes(f.bob.device)
            .await
            .unwrap()
            .is_empty()
    );

    // A member who left no longer gets the conversation's Welcomes.
    let again = f
        .repo
        .insert_welcome(NewMlsWelcome {
            commit_id: commit.id,
            conversation_id: f.conv.id,
            incarnation: 1,
            epoch: 2,
            message: vec![6; 8],
            expires_at: Utc::now() + Duration::days(30),
            recipients: vec![(f.bob.device, f.bob.user.id)],
        })
        .await
        .unwrap();
    assert_eq!(
        f.repo.pending_welcomes(f.bob.device).await.unwrap()[0].id,
        again
    );
    SeaOrmConversationMemberRepository::new(f.ctx.db.clone())
        .remove_member(f.conv.id, f.bob.user.id)
        .await
        .unwrap();
    assert!(
        f.repo
            .pending_welcomes(f.bob.device)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn readd_requests_and_retention() {
    let f = fixture().await;
    create_group(&f).await;
    let request = f
        .repo
        .insert_readd_request(NewMlsReaddRequest {
            conversation_id: f.conv.id,
            incarnation: 1,
            device_key_id: f.bob.device,
            payload: "{}".to_string(),
            signature: "sig".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(request.device.device_id, f.bob.device_id);
    assert_eq!(request.device.principal_id, "rimp1_bob");
    let since = Utc::now() - Duration::days(7);
    assert_eq!(
        f.repo
            .open_readd_requests(f.conv.id, 1, since)
            .await
            .unwrap()
            .len(),
        1
    );
    assert_eq!(
        f.repo
            .fulfill_readd_requests(f.conv.id, 1, &[f.bob.device], 3)
            .await
            .unwrap(),
        1
    );
    assert!(
        f.repo
            .open_readd_requests(f.conv.id, 1, since)
            .await
            .unwrap()
            .is_empty()
    );

    // Old commits, claimed one-time packages and expired packages go; fresh ones stay.
    f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    f.repo.insert_commit(commit(&f, 2, 2)).await.unwrap();
    f.ctx
        .db
        .execute_unprepared(
            "UPDATE mls_commit SET created_at = now() - interval '91 days' WHERE epoch = 1",
        )
        .await
        .unwrap();
    f.repo
        .insert_key_package(key_package(&f.bob, 1, false, 30))
        .await
        .unwrap();
    f.repo
        .insert_key_package(key_package(&f.bob, 2, false, 30))
        .await
        .unwrap();
    f.repo
        .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
        .await
        .unwrap()
        .unwrap();
    f.ctx
        .db
        .execute_unprepared(
            "UPDATE mls_key_package SET claimed_at = now() - interval '8 days' \
             WHERE claimed_at IS NOT NULL",
        )
        .await
        .unwrap();

    let purged = f
        .repo
        .purge(MlsRetention {
            commits: Duration::days(90),
            claimed_key_packages: Duration::days(7),
            readd_requests: Duration::days(7),
            franked_content: Duration::days(90),
        })
        .await
        .unwrap();
    assert_eq!(purged.commits, 1);
    assert_eq!(purged.key_packages, 1);
    assert_eq!(purged.readd_requests, 1, "fulfilled requests go");
    assert_eq!(
        f.repo
            .count_available_key_packages(f.bob.device, SUITE)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        f.repo
            .list_commits(f.conv.id, 1, None, 10)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn deleting_a_conversation_or_device_cascades() {
    let f = fixture().await;
    create_group(&f).await;
    f.repo
        .insert_key_package(key_package(&f.bob, 1, false, 30))
        .await
        .unwrap();
    f.repo
        .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id)
        .await
        .unwrap();
    let commit = f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    f.repo
        .insert_welcome(NewMlsWelcome {
            commit_id: commit.id,
            conversation_id: f.conv.id,
            incarnation: 1,
            epoch: 2,
            message: vec![1],
            expires_at: Utc::now() + Duration::days(30),
            recipients: vec![(f.bob.device, f.bob.user.id)],
        })
        .await
        .unwrap();
    insert_message(&f.repo, &f, 1, b"bye").await;
    assert_eq!(
        f.repo
            .conversations_with_group_device(f.alice.device)
            .await
            .unwrap(),
        vec![f.conv.id]
    );
    assert_eq!(
        f.repo
            .grouped_conversations_of_user(f.bob.user.id)
            .await
            .unwrap(),
        vec![f.conv.id]
    );

    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM device_key WHERE id = $1",
            [Uuid::from(f.alice.device).into()],
        ))
        .await
        .unwrap();
    let left = f.repo.group_devices(f.conv.id, 1).await.unwrap();
    assert_eq!(left.len(), 1, "the device's leaf goes with it");

    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM conversation WHERE id = $1",
            [Uuid::from(f.conv.id).into()],
        ))
        .await
        .unwrap();
    for table in [
        "mls_group",
        "mls_group_device",
        "mls_commit",
        "mls_welcome",
        "mls_welcome_recipient",
    ] {
        let row = f
            .ctx
            .db
            .query_one_raw(Statement::from_string(
                DbBackend::Postgres,
                format!("SELECT count(*) AS n FROM {table}"),
            ))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(row.try_get::<i64>("", "n").unwrap(), 0, "{table}");
    }
    let package = f
        .repo
        .find_key_packages_by_refs(&[vec![1; 32]])
        .await
        .unwrap();
    assert_eq!(
        package[0].claimed_for_conversation_id, None,
        "a claim forgets a deleted conversation"
    );
}

#[tokio::test]
async fn migration_down_is_refused_while_an_encrypted_message_exists() {
    let f = fixture().await;
    let conn = support::db_conn().await;
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");

    create_group(&f).await;
    let message = insert_message(&f.repo, &f, 1, b"secret").await;
    let err = Migrator::down(&conn, Some(steps)).await.unwrap_err();
    assert!(
        err.to_string().contains("encrypted messages exist"),
        "{err}"
    );

    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "DELETE FROM message WHERE id = $1",
            [Uuid::from(message.id).into()],
        ))
        .await
        .unwrap();
    Migrator::down(&conn, Some(steps))
        .await
        .expect("roll back add_e2ee_mls");
    // The previous change feed trigger is back: a text edit still stamps edited_at.
    let plain = Uuid::now_v7();
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "INSERT INTO message (id, conversation_id, sent_by, text, is_deleted, has_attachments) \
         VALUES ($1, $2, $3, 'before', false, false)",
        [
            plain.into(),
            Uuid::from(f.conv.id).into(),
            Uuid::from(f.alice.user.id).into(),
        ],
    ))
    .await
    .unwrap();
    let messages = SeaOrmMessageRepository::new(f.ctx.db.clone());
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "UPDATE message SET text = 'after' WHERE id = $1",
        [plain.into()],
    ))
    .await
    .unwrap();
    let edited: Option<chrono::DateTime<Utc>> = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT edited_at FROM message WHERE id = $1",
            [plain.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "edited_at")
        .unwrap();
    assert!(edited.is_some());

    Migrator::up(&conn, None)
        .await
        .expect("re-apply add_e2ee_mls");
    let feed = messages
        .list_message_changes(f.conv.id, 0, 10)
        .await
        .unwrap();
    assert_eq!(feed.len(), 1);
    assert_eq!(feed[0].message.encryption, None);
}

#[tokio::test]
async fn search_never_returns_encrypted_rows() {
    let f = fixture().await;
    create_group(&f).await;
    insert_message(&f.repo, &f, 1, b"ciphertext").await;
    let plain = f
        .ctx
        .create_message(f.conv.id, f.alice.user.id, "plain words")
        .await;
    let hits = SeaOrmMessageRepository::new(f.ctx.db.clone())
        .search_messages_fts(rim_core::message::repository::MessageSearchParams {
            conversation_ids: vec![f.conv.id],
            tsquery: String::new(),
            author_id: Some(f.alice.user.id),
            has_link: false,
            has_attachment: false,
            after: None,
            before: None,
            limit: 10,
            cursor: None,
            dir: rim_core::repository::pagination::PageDir::After,
        })
        .await
        .unwrap();
    assert_eq!(
        hits.iter().map(|h| h.message.id).collect::<Vec<_>>(),
        vec![plain.id],
        "an operator-only search matches rows without text, but never encrypted ones"
    );
}
