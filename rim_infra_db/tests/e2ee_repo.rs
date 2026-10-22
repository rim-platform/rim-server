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
    MlsRetention, MlsWelcomeBacklog, MlsWelcomeCursor, NewEncryptedMessage, NewFrankingRevision,
    NewMlsCommit, NewMlsGroup, NewMlsKeyPackage, NewMlsReaddRequest, NewMlsWelcome,
};
use rim_core::e2ee::repository::{E2eeRepository, GroupLock};
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::{DeviceKeyId, NewDeviceKey};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::message::model::{MessageEncryption, MessageId};
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::model::{MessageAttachmentId, NewMessageAttachment};
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::moderation::model::{MessageReportStatus, NewFrankedReport, NewMessageReport};
use rim_core::moderation::repository::ModerationRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::server::model::Server;
use rim_core::user::model::{User, UserId};
use rim_infra_db::begin_transaction;
use rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository;
use rim_infra_db::sea_orm_impl::e2ee::SeaOrmE2eeRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository;
use rim_infra_db::sea_orm_impl::moderation::SeaOrmModerationRepository;

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
    repo.insert_encrypted_message(NewEncryptedMessage {
        id: MessageId::new(),
        conversation_id: f.conv.id,
        sent_by: f.alice.user.id,
        thread_id: None,
        forwarded_from_message_id: None,
        sent_at: Utc::now(),
        envelope: envelope(epoch, f.alice.device, ciphertext, 1),
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
async fn a_readd_claims_the_newest_package_uploaded_after_its_request() {
    let f = fixture().await;
    for tag in 1..=3 {
        f.repo
            .insert_key_package(key_package(&f.bob, tag, false, 30))
            .await
            .unwrap();
        // Distinct upload times.
        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    }
    let found = f
        .repo
        .find_key_packages_by_refs(&[vec![1; 32]])
        .await
        .unwrap();
    let after_first = found[0].created_at;
    let claim = |after| {
        f.repo
            .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1, after)
    };
    let newest = claim(Some(after_first)).await.unwrap().expect("newest");
    assert_eq!(newest.key_package_ref, vec![3; 32]);
    let next = claim(Some(after_first))
        .await
        .unwrap()
        .expect("next newest");
    assert_eq!(next.key_package_ref, vec![2; 32]);
    assert!(
        claim(Some(after_first)).await.unwrap().is_none(),
        "the package from before the request is never used"
    );
}

#[tokio::test]
async fn key_packages_are_claimed_oldest_first_then_the_last_resort() {
    let f = fixture().await;
    // Valid for 10 more days: too close to expiry to hand out.
    f.repo
        .insert_key_package(key_package(&f.bob, 6, false, 10))
        .await
        .unwrap();
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
            .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1, None)
            .await
            .unwrap()
            .expect("one-time package");
        assert!(!package.last_resort);
        claimed.push(package.key_package_ref[0]);
    }
    assert_eq!(claimed, vec![1, 2, 3], "oldest first");
    assert!(
        f.repo
            .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1, None)
            .await
            .unwrap()
            .is_none(),
        "expired packages and ones close to expiry are never handed out"
    );
    let last_resort = f
        .repo
        .claim_last_resort_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1)
        .await
        .unwrap()
        .expect("last resort");
    assert!(last_resort.last_resort);
    // The last-resort package stays and can be handed out again.
    assert!(
        f.repo
            .claim_last_resort_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 2)
            .await
            .unwrap()
            .is_some()
    );
    let status = f.repo.key_package_status(f.bob.device).await.unwrap();
    assert_eq!(status[0].available, 0);
    assert_eq!(status[0].last_resort_claims, 2);

    let found = f
        .repo
        .find_key_packages_by_refs(&[vec![1; 32], vec![9; 32], vec![7; 32]])
        .await
        .unwrap();
    assert_eq!(found.len(), 2);
    let one_time = found.iter().find(|p| !p.last_resort).expect("one-time");
    assert_eq!(one_time.claimed_by_device_key_id, Some(f.alice.device));
    assert_eq!(one_time.claimed_for_conversation_id, Some(f.conv.id));
    assert_eq!(one_time.claimed_for_incarnation, Some(1));
    assert!(one_time.claimed_at.is_some());
    let last_resort = found.iter().find(|p| p.last_resort).expect("last resort");
    assert_eq!(
        last_resort.claimed_for_incarnation,
        Some(2),
        "the latest claim"
    );

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
                .claim_one_time_key_package(device, SUITE, claimer, conv, 1, None)
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

#[tokio::test]
async fn commit_bytes_are_summed_per_sender_and_window() {
    let f = fixture().await;
    create_group(&f).await;
    let earlier = Utc::now() - Duration::minutes(1);
    assert_eq!(
        f.repo
            .commit_bytes_since(f.conv.id, f.alice.user.id, earlier)
            .await
            .unwrap(),
        0
    );
    f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    f.repo
        .insert_commit(NewMlsCommit {
            message: vec![2; 1000],
            ..commit(&f, 2, 2)
        })
        .await
        .unwrap();
    f.repo
        .insert_commit(NewMlsCommit {
            sender_user_id: f.bob.user.id,
            sender_device_key_id: f.bob.device,
            ..commit(&f, 3, 3)
        })
        .await
        .unwrap();
    assert_eq!(
        f.repo
            .commit_bytes_since(f.conv.id, f.alice.user.id, earlier)
            .await
            .unwrap(),
        1016
    );
    assert_eq!(
        f.repo
            .commit_bytes_since(f.conv.id, f.bob.user.id, earlier)
            .await
            .unwrap(),
        16
    );
    assert_eq!(
        f.repo
            .commit_bytes_since(
                f.conv.id,
                f.alice.user.id,
                Utc::now() + Duration::minutes(1)
            )
            .await
            .unwrap(),
        0
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
    let dup = f
        .repo
        .insert_encrypted_message(NewEncryptedMessage {
            id: sent.id,
            conversation_id: f.conv.id,
            sent_by: f.alice.user.id,
            thread_id: None,
            forwarded_from_message_id: None,
            sent_at: Utc::now(),
            envelope: envelope(2, f.alice.device, b"again", 1),
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

    let found = f.repo.get_device(f.alice.device).await.unwrap().unwrap();
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
    let pending = f
        .repo
        .pending_welcomes(f.bob.device, None, 100)
        .await
        .unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].id, welcome);
    assert_eq!(pending[0].server_id, f.server.id);
    assert_eq!(pending[0].epoch, 2);
    assert!(
        f.repo
            .pending_welcomes(f.alice.device, None, 100)
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
            .pending_welcomes(f.bob.device, None, 100)
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
        f.repo
            .pending_welcomes(f.bob.device, None, 100)
            .await
            .unwrap()[0]
            .id,
        again
    );
    SeaOrmConversationMemberRepository::new(f.ctx.db.clone())
        .remove_member(f.conv.id, f.bob.user.id)
        .await
        .unwrap();
    assert!(
        f.repo
            .pending_welcomes(f.bob.device, None, 100)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn pending_welcomes_are_paged_and_counted_per_device() {
    let f = fixture().await;
    create_group(&f).await;
    let commit = f.repo.insert_commit(commit(&f, 1, 1)).await.unwrap();
    let welcome = |size: usize, days: i64, recipients: Vec<(DeviceKeyId, UserId)>| NewMlsWelcome {
        commit_id: commit.id,
        conversation_id: f.conv.id,
        incarnation: 1,
        epoch: 2,
        message: vec![7; size],
        expires_at: Utc::now() + Duration::days(days),
        recipients,
    };
    let bob = || vec![(f.bob.device, f.bob.user.id)];
    let mut ids = Vec::new();
    for size in 1..=5 {
        ids.push(
            f.repo
                .insert_welcome(welcome(size, 30, bob()))
                .await
                .unwrap(),
        );
    }
    f.repo
        .insert_welcome(welcome(100, -1, bob()))
        .await
        .unwrap();
    f.repo
        .insert_welcome(welcome(9, 30, vec![(f.alice.device, f.alice.user.id)]))
        .await
        .unwrap();

    // Pages follow (created_at, id) without gaps or repeats, and skip the expired Welcome.
    let mut read = Vec::new();
    let mut after = None;
    loop {
        let page = f
            .repo
            .pending_welcomes(f.bob.device, after, 2)
            .await
            .unwrap();
        assert!(page.len() <= 2);
        let Some(last) = page.last() else { break };
        after = Some(MlsWelcomeCursor {
            created_at: last.created_at,
            id: last.id,
        });
        read.extend(page.iter().map(|w| w.id));
    }
    assert_eq!(read, ids);

    let backlog = |devices: Vec<DeviceKeyId>| {
        let repo = &f.repo;
        async move {
            let mut rows = repo.welcome_backlog(&devices).await.unwrap();
            rows.sort_by_key(|b| b.count);
            rows
        }
    };
    let stranger = member(&f.ctx, "stranger").await;
    assert_eq!(
        backlog(vec![f.bob.device, f.alice.device, stranger.device]).await,
        vec![
            MlsWelcomeBacklog {
                device_key_id: f.alice.device,
                count: 1,
                bytes: 9,
            },
            MlsWelcomeBacklog {
                device_key_id: f.bob.device,
                count: 5,
                bytes: 15,
            },
        ]
    );
    assert!(backlog(Vec::new()).await.is_empty());

    // An acknowledged Welcome no longer counts, nor do Welcomes of a conversation left.
    f.repo.ack_welcomes(f.bob.device, &ids[4..]).await.unwrap();
    assert_eq!(
        backlog(vec![f.bob.device]).await,
        vec![MlsWelcomeBacklog {
            device_key_id: f.bob.device,
            count: 4,
            bytes: 10,
        }]
    );
    SeaOrmConversationMemberRepository::new(f.ctx.db.clone())
        .remove_member(f.conv.id, f.bob.user.id)
        .await
        .unwrap();
    assert!(backlog(vec![f.bob.device]).await.is_empty());
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
            nonce: vec![1; 16],
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
        .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1, None)
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
            readd_nonces: Duration::days(1),
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
async fn finding_9_a_readd_nonce_is_accepted_once_per_device_and_incarnation() {
    let f = fixture().await;
    create_group(&f).await;
    let request = |device: DeviceKeyId, nonce: u8| NewMlsReaddRequest {
        conversation_id: f.conv.id,
        incarnation: 1,
        device_key_id: device,
        payload: "{}".to_string(),
        signature: "sig".to_string(),
        nonce: vec![nonce; 16],
    };
    f.repo
        .insert_readd_request(request(f.bob.device, 1))
        .await
        .expect("first");
    // Fulfilled requests are deleted; their nonces are not.
    f.repo
        .fulfill_readd_requests(f.conv.id, 1, &[f.bob.device], 2)
        .await
        .unwrap();
    let retention = MlsRetention {
        commits: Duration::days(90),
        claimed_key_packages: Duration::days(7),
        readd_requests: Duration::days(7),
        readd_nonces: Duration::days(1),
        franked_content: Duration::days(90),
    };
    let purged = f.repo.purge(retention).await.unwrap();
    assert_eq!((purged.readd_requests, purged.readd_nonces), (1, 0));
    let err = f
        .repo
        .insert_readd_request(request(f.bob.device, 1))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
    let open = f
        .repo
        .open_readd_requests(f.conv.id, 1, Utc::now() - Duration::days(7))
        .await
        .unwrap();
    assert!(open.is_empty(), "a refused request stores nothing");

    // Another nonce, or the same nonce from another device, is a new request.
    f.repo
        .insert_readd_request(request(f.bob.device, 2))
        .await
        .expect("new nonce");
    f.repo
        .insert_readd_request(request(f.alice.device, 1))
        .await
        .expect("other device");

    // Nonces go after their retention.
    f.ctx
        .db
        .execute_unprepared(
            "UPDATE mls_readd_nonce SET created_at = now() - interval '2 days' \
             WHERE nonce = decode(repeat('01', 16), 'hex')",
        )
        .await
        .unwrap();
    let purged = f.repo.purge(retention).await.unwrap();
    assert_eq!(purged.readd_nonces, 2);
}

#[tokio::test]
async fn max_message_franking_key_id_reads_the_messages() {
    let f = fixture().await;
    create_group(&f).await;
    assert_eq!(f.repo.max_message_franking_key_id().await.unwrap(), None);

    f.repo
        .insert_encrypted_message(NewEncryptedMessage {
            id: MessageId::new(),
            conversation_id: f.conv.id,
            sent_by: f.alice.user.id,
            thread_id: None,
            forwarded_from_message_id: None,
            sent_at: Utc::now(),
            envelope: envelope(1, f.alice.device, b"hi", 5),
        })
        .await
        .expect("insert");
    assert_eq!(f.repo.max_message_franking_key_id().await.unwrap(), Some(5));
}

#[tokio::test]
async fn franking_revisions_are_numbered_and_go_with_their_message() {
    let f = fixture().await;
    create_group(&f).await;
    let message = insert_message(&f.repo, &f, 1, b"hi").await;
    for commitment in [1u8, 2] {
        f.repo
            .append_franking_revision(NewFrankingRevision {
                message_id: message.id,
                franking_commitment: vec![commitment; 32],
                sent_at_us: 7,
                sender_device_id: f.alice.device_id,
            })
            .await
            .expect("revision");
    }
    let revisions = f.repo.franking_revisions(message.id).await.unwrap();
    assert_eq!(
        revisions
            .iter()
            .map(|r| (r.revision, r.franking_commitment[0], r.sent_at_us))
            .collect::<Vec<_>>(),
        vec![(0, 1, 7), (1, 2, 7)]
    );
    assert_eq!(revisions[0].sender_device_id, f.alice.device_id);

    // A soft delete keeps them; deleting the row takes them with it.
    SeaOrmMessageRepository::new(f.ctx.db.clone())
        .soft_delete_message(message.id)
        .await
        .unwrap();
    assert_eq!(
        f.repo.franking_revisions(message.id).await.unwrap().len(),
        2
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
    assert!(
        f.repo
            .franking_revisions(message.id)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_revision_migration_records_existing_messages_as_revision_0() {
    let f = fixture().await;
    create_group(&f).await;
    let conn = support::db_conn().await;
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == "m20261010_000142_add_message_franking_revision")
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("step count fits u32");
    Migrator::down(&conn, Some(steps))
        .await
        .expect("roll back add_message_franking_revision");
    let message = insert_message(&f.repo, &f, 1, b"before").await;
    Migrator::up(&conn, None)
        .await
        .expect("re-apply add_message_franking_revision");
    let revisions = f.repo.franking_revisions(message.id).await.unwrap();
    assert_eq!(revisions.len(), 1);
    assert_eq!(revisions[0].revision, 0);
    assert_eq!(revisions[0].franking_commitment, vec![1; 32]);
    assert_eq!(revisions[0].sent_at_us, message.sent_at.timestamp_micros());
    assert_eq!(revisions[0].sender_device_id, f.alice.device_id);
}

/// A franked report as the e2ee service stores it.
fn franked_report(f: &Fixture, message: MessageId, reporter: &Member) -> NewMessageReport {
    NewMessageReport {
        server_id: f.server.id,
        conversation_id: f.conv.id,
        message_id: message,
        reporter_user_id: reporter.user.id,
        reason: "abuse".to_string(),
        franked: Some(NewFrankedReport {
            content: b"{\"body\":\"hi\"}".to_vec(),
            sender_device_id: f.alice.device_id,
        }),
    }
}

#[tokio::test]
async fn verified_report_content_is_cleared_90_days_after_resolution() {
    let f = fixture().await;
    create_group(&f).await;
    let reports = SeaOrmModerationRepository::new(f.ctx.db.clone());
    let first = insert_message(&f.repo, &f, 1, b"hi").await;
    let second = insert_message(&f.repo, &f, 1, b"ho").await;
    let unreported = insert_message(&f.repo, &f, 1, b"hm").await;

    let old = reports
        .create_report(franked_report(&f, first.id, &f.bob))
        .await
        .expect("report");
    let recent = reports
        .create_report(franked_report(&f, first.id, &f.alice))
        .await
        .expect("report");
    let open = reports
        .create_report(franked_report(&f, second.id, &f.bob))
        .await
        .expect("report");
    let verified = old.franked.clone().expect("franked");
    assert_eq!(verified.content.as_deref(), Some(&b"{\"body\":\"hi\"}"[..]));
    assert_eq!(verified.sender_device_id, Some(f.alice.device_id));
    for (report, days) in [(&old, 91), (&recent, 89)] {
        reports
            .resolve_report(
                report.id,
                MessageReportStatus::Resolved,
                f.alice.user.id,
                Utc::now() - Duration::days(days),
            )
            .await
            .expect("resolve");
    }

    let purged = f
        .repo
        .purge(MlsRetention {
            commits: Duration::days(90),
            claimed_key_packages: Duration::days(7),
            readd_requests: Duration::days(7),
            readd_nonces: Duration::days(1),
            franked_content: Duration::days(90),
        })
        .await
        .unwrap();
    assert_eq!(purged.franked_contents, 1);
    let cleared = reports
        .get_report(old.id)
        .await
        .unwrap()
        .and_then(|r| r.franked)
        .expect("still verified");
    assert_eq!(cleared.content, None);
    assert_eq!(cleared.sender_device_id, Some(f.alice.device_id));
    for kept in [recent.id, open.id] {
        let franked = reports
            .get_report(kept)
            .await
            .unwrap()
            .and_then(|r| r.franked);
        assert!(franked.and_then(|r| r.content).is_some());
    }

    assert!(
        reports
            .has_verified_report(f.server.id, first.id)
            .await
            .unwrap(),
        "a cleared report stays verified"
    );
    assert!(
        !reports
            .has_verified_report(f.server.id, unreported.id)
            .await
            .unwrap()
    );
    let other = f.ctx.create_server("other").await;
    assert!(
        !reports
            .has_verified_report(other.id, first.id)
            .await
            .unwrap()
    );
}

/// A deleted encrypted message keeps its row and franking columns, so it stays reportable, and
/// the hourly retention run never removes message rows: a verified report of it keeps its
/// message and content until the content's own retention.
#[tokio::test]
async fn finding_26_a_report_of_a_deleted_message_survives_the_retention_run() {
    let f = fixture().await;
    create_group(&f).await;
    let messages = SeaOrmMessageRepository::new(f.ctx.db.clone());
    let reports = SeaOrmModerationRepository::new(f.ctx.db.clone());
    let message = insert_message(&f.repo, &f, 1, b"regret").await;
    messages
        .soft_delete_message(message.id)
        .await
        .expect("delete");
    // Long past every retention the run applies.
    f.ctx
        .db
        .execute_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "UPDATE message SET sent_at = now() - interval '400 days' WHERE id = $1",
            [Uuid::from(message.id).into()],
        ))
        .await
        .unwrap();
    let deleted = messages
        .get_by_id(message.id)
        .await
        .unwrap()
        .expect("the row stays");
    assert!(deleted.is_deleted);
    let envelope = deleted.encryption.expect("franking columns stay");
    assert_eq!(envelope.ciphertext, None);
    assert_eq!(envelope.franking_commitment, vec![1; 32]);

    let report = reports
        .create_report(franked_report(&f, message.id, &f.bob))
        .await
        .expect("report of a deleted message");
    let purged = f
        .repo
        .purge(MlsRetention {
            commits: Duration::days(90),
            claimed_key_packages: Duration::days(7),
            readd_requests: Duration::days(7),
            readd_nonces: Duration::days(1),
            franked_content: Duration::days(90),
        })
        .await
        .unwrap();
    assert_eq!(purged.franked_contents, 0);
    assert!(messages.get_by_id(message.id).await.unwrap().is_some());
    let kept = reports
        .get_report(report.id)
        .await
        .unwrap()
        .and_then(|r| r.franked)
        .expect("still verified");
    assert!(kept.content.is_some());
}

#[tokio::test]
async fn a_stored_blob_is_found_by_hash_within_its_workspace() {
    let f = fixture().await;
    create_group(&f).await;
    let message = insert_message(&f.repo, &f, 1, b"file").await;
    let attachments = SeaOrmMessageAttachmentRepository::new(f.ctx.db.clone());
    let add = |storage_url: &str, hash: u8| NewMessageAttachment {
        id: MessageAttachmentId::new(),
        storage_url: storage_url.to_string(),
        message_id: message.id,
        file_size: 34,
        file_name: "encrypted".to_string(),
        mime_type: "application/octet-stream".to_string(),
        metadata: None,
        blob_sha256: Some(vec![hash; 32]),
    };
    attachments
        .add_attachment(add("pending://upload", 1))
        .await
        .unwrap();
    let stored = attachments
        .add_attachment(add("uploads/blob", 2))
        .await
        .unwrap();

    let found = attachments
        .find_stored_by_blob_sha256(f.server.id, vec![2; 32])
        .await
        .unwrap();
    assert_eq!(found.map(|a| a.id), Some(stored.id));
    assert_eq!(
        attachments
            .find_stored_by_blob_sha256(f.server.id, vec![1; 32])
            .await
            .unwrap(),
        None,
        "a pending upload holds no blob"
    );
    let other = f.ctx.create_server("other").await;
    assert_eq!(
        attachments
            .find_stored_by_blob_sha256(other.id, vec![2; 32])
            .await
            .unwrap(),
        None
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
        .claim_one_time_key_package(f.bob.device, SUITE, f.alice.device, f.conv.id, 1, None)
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
    // The refused call already rolled back every later migration.
    Migrator::down(&conn, Some(1))
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

/// A pending member's devices are not wanted in the group, and they do not count as a member
/// for Commits, Welcomes or group lookups; remote users are told apart from local ones.
#[tokio::test]
async fn pending_members_stay_out_of_the_group() {
    let f = fixture().await;
    let carol = member(&f.ctx, "carol").await;
    SeaOrmConversationMemberRepository::new(f.ctx.db.clone())
        .add_pending_member(f.conv.id, carol.user.id)
        .await
        .unwrap();
    create_group(&f).await;
    let active_since = Utc::now() - Duration::days(60);

    let desired = f
        .repo
        .desired_devices(f.conv.id, active_since, SUITE)
        .await
        .unwrap();
    assert!(desired.iter().all(|d| d.device.user_id != carol.user.id));
    assert_eq!(desired.len(), 2);
    let members = f
        .repo
        .members_by_principal(f.conv.id, &["rimp1_carol".to_string()])
        .await
        .unwrap();
    assert!(members.is_empty());
    assert!(
        f.repo
            .grouped_conversations_of_user(carol.user.id)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        f.repo
            .grouped_conversations_of_user(f.alice.user.id)
            .await
            .unwrap(),
        vec![f.conv.id]
    );

    let remote = rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository::new(f.ctx.db.clone())
        .create_remote_user(rim_core::host::model::NewRemoteUser {
            principal_id: "rimp1_remote".to_string(),
            identity_key: "ed25519:remote".to_string(),
            username: "r_remote".to_string(),
            display_name: "Remote".to_string(),
            home_relay: Some("rims1_relay".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(
        f.repo
            .remote_users(&[f.alice.user.id, remote])
            .await
            .unwrap(),
        vec![remote]
    );
    assert!(f.repo.remote_users(&[]).await.unwrap().is_empty());
}
