#![cfg(feature = "db-tests")]

//! Relay queues, the request inbox, host event streams, the outbox and the device request
//! window against a real database: per-stream `seq` under concurrency, outbox order, inbound
//! idempotency, deletion only after every device acked, expired requests left undecided,
//! cascades, and the migration's refusing `down`.

mod support;

use std::collections::HashSet;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::host::model::{NewRemoteUser, SeqDecision, UserKind};
use rim_core::host::repository::HostRepository;
use rim_core::key_identity::model::{DeviceKey, NewDeviceKey};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{
    DmRequestStatus, NewDmRequest, NewRelayEnvelope, NewRelayItem, RelayQueueStats,
};
use rim_core::relay::repository::RelayRepository;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_db::{DbHandle, begin_transaction};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261010_002421_add_relay_delivery";
const HOST: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

async fn device(db: &DbHandle, user_id: UserId, n: u8) -> DeviceKey {
    SeaOrmKeyIdentityRepository::new(db.clone())
        .add_device(NewDeviceKey {
            user_id,
            device_id: Uuid::from_u128(u128::from(n)),
            public_key: format!("ed25519:device{n}"),
            key_hash: vec![n; 32],
            name: format!("Device {n}"),
            cert_payload: "{}".to_string(),
            cert_signature: "ed25519:x".to_string(),
            key_seq: 0,
        })
        .await
        .unwrap()
}

fn envelope(seq: Option<i64>, workspace_id: Uuid) -> NewRelayEnvelope {
    NewRelayEnvelope {
        host_server_id: HOST.to_string(),
        workspace_id,
        stream: "c:x".to_string(),
        seq,
        payload: format!("{{\"seq\":{seq:?}}}"),
        signature: "ed25519:s".to_string(),
    }
}

fn item(user_id: UserId, envelope_id: i64) -> NewRelayItem {
    NewRelayItem {
        user_id,
        envelope_id,
        device_key_id: None,
        request_id: None,
        size_bytes: 100,
        expires_at: Utc::now() + Duration::days(30),
    }
}

async fn count(db: &DbHandle, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_string(DbBackend::Postgres, sql))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap()
}

async fn table_exists(db: &sea_orm::DatabaseConnection, table: &str) -> bool {
    db.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT to_regclass($1) IS NOT NULL AS present",
        [format!("public.{table}").into()],
    ))
    .await
    .unwrap()
    .unwrap()
    .try_get::<bool>("", "present")
    .unwrap()
}

#[tokio::test]
async fn stream_seqs_have_no_gaps_or_repeats_under_concurrency() {
    let ctx = support::TestCtx::new().await;
    let server = ctx.create_server("hosted").await;
    let conn = support::db_conn().await;
    let mut tasks = Vec::new();
    for _ in 0..24 {
        let conn = conn.clone();
        tasks.push(tokio::spawn(async move {
            let scope = begin_transaction(&conn).await.unwrap();
            let repo = SeaOrmHostRepository::new(scope.handle());
            let seq = repo.next_stream_seq(server.id, "c:one").await.unwrap();
            repo.insert_outbox("rims1_relay", &format!("{{\"seq\":{seq}}}"), 10)
                .await
                .unwrap();
            scope.commit().await.unwrap();
            seq
        }));
    }
    let mut seqs = Vec::new();
    for task in tasks {
        seqs.push(task.await.unwrap());
    }
    seqs.sort_unstable();
    assert_eq!(seqs, (1..=24).collect::<Vec<i64>>());

    let repo = SeaOrmHostRepository::new(ctx.db.clone());
    assert_eq!(repo.next_stream_seq(server.id, "w").await.unwrap(), 1);
    let heads = repo
        .stream_heads(
            server.id,
            &["c:one".to_string(), "w".to_string(), "u:x".to_string()],
        )
        .await
        .unwrap();
    let heads: HashSet<(String, i64)> = heads.into_iter().collect();
    assert_eq!(
        heads,
        HashSet::from([("c:one".to_string(), 24), ("w".to_string(), 1)])
    );

    // A rolled-back transaction gives its seq back.
    let scope = begin_transaction(&conn).await.unwrap();
    let in_txn = SeaOrmHostRepository::new(scope.handle());
    assert_eq!(in_txn.next_stream_seq(server.id, "w").await.unwrap(), 2);
    drop(in_txn);
    drop(scope);
    assert_eq!(repo.next_stream_seq(server.id, "w").await.unwrap(), 2);
}

#[tokio::test]
async fn outbox_rows_come_in_order_per_peer_within_the_byte_cap() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmHostRepository::new(ctx.db.clone());
    let mut a = Vec::new();
    for i in 0..5 {
        a.push(
            repo.insert_outbox("rims1_a", &format!("a{i}"), 100)
                .await
                .unwrap(),
        );
        repo.insert_outbox("rims1_b", &format!("b{i}"), 100)
            .await
            .unwrap();
    }
    let rows = repo
        .outbox_after("rims1_a", 0, 256, 1_000_000)
        .await
        .unwrap();
    assert_eq!(rows.iter().map(|r| r.id).collect::<Vec<_>>(), a);
    assert!(rows.iter().all(|r| r.item.starts_with('a')));
    let after = repo
        .outbox_after("rims1_a", a[1], 2, 1_000_000)
        .await
        .unwrap();
    assert_eq!(after.iter().map(|r| r.id).collect::<Vec<_>>(), a[2..4]);
    let capped = repo.outbox_after("rims1_a", 0, 256, 250).await.unwrap();
    assert_eq!(capped.len(), 2, "the byte cap stops the page");
    let one = repo.outbox_after("rims1_a", 0, 256, 10).await.unwrap();
    assert_eq!(one.len(), 1, "the first row always goes");

    let now = Utc::now();
    let due: HashSet<String> = repo
        .outbox_peers_due(now)
        .await
        .unwrap()
        .into_iter()
        .collect();
    assert_eq!(
        due,
        HashSet::from(["rims1_a".to_string(), "rims1_b".to_string()])
    );
    repo.record_delivered("rims1_a", a[4]).await.unwrap();
    repo.record_failure("rims1_b", 3, now + Duration::minutes(5))
        .await
        .unwrap();
    assert!(repo.outbox_peers_due(now).await.unwrap().is_empty());
    assert_eq!(repo.delivery("rims1_b").await.unwrap().failures, 3);
    repo.record_delivered("rims1_a", a[2]).await.unwrap();
    assert_eq!(
        repo.delivery("rims1_a").await.unwrap().delivered_through,
        a[4],
        "delivered_through never moves back"
    );
    assert_eq!(
        repo.delete_outbox_before(Utc::now() + Duration::seconds(1))
            .await
            .unwrap(),
        10
    );
}

#[tokio::test]
async fn received_through_only_moves_up_and_envelopes_are_stored_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    assert_eq!(repo.received_through(HOST).await.unwrap(), 0);
    repo.advance_received_through(HOST, 5).await.unwrap();
    repo.advance_received_through(HOST, 3).await.unwrap();
    assert_eq!(repo.received_through(HOST).await.unwrap(), 5);
    assert_eq!(repo.inbound_peers().await.unwrap(), vec![HOST.to_string()]);

    let workspace = Uuid::now_v7();
    let first = repo
        .insert_envelope(envelope(Some(1), workspace))
        .await
        .unwrap();
    let again = repo
        .insert_envelope(envelope(Some(1), workspace))
        .await
        .unwrap();
    assert_eq!(first, again);
    let second = repo
        .insert_envelope(envelope(Some(2), workspace))
        .await
        .unwrap();
    assert_ne!(first, second);
    let e1 = repo
        .insert_envelope(envelope(None, workspace))
        .await
        .unwrap();
    let e2 = repo
        .insert_envelope(envelope(None, workspace))
        .await
        .unwrap();
    assert_ne!(e1, e2, "ephemeral events have no seq to deduplicate on");
}

#[tokio::test]
async fn items_go_only_after_every_device_acked() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let d1 = device(&ctx.db, user.id, 1).await;
    let d2 = device(&ctx.db, user.id, 2).await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let workspace = Uuid::now_v7();
    let env = repo
        .insert_envelope(envelope(Some(1), workspace))
        .await
        .unwrap();
    let i1 = repo.insert_item(item(user.id, env)).await.unwrap();
    let env2 = repo
        .insert_envelope(envelope(Some(2), workspace))
        .await
        .unwrap();
    let i2 = repo.insert_item(item(user.id, env2)).await.unwrap();

    assert_eq!(repo.device_cursor(d1.id).await.unwrap(), 0);
    let page = repo.list_items(user.id, d1.id, 0, 10).await.unwrap();
    assert_eq!(page.iter().map(|i| i.id).collect::<Vec<_>>(), vec![i1, i2]);

    repo.ack(d1.id, i2).await.unwrap();
    repo.ack(d1.id, i1).await.unwrap();
    assert_eq!(
        repo.device_cursor(d1.id).await.unwrap(),
        i2,
        "cursors never move back"
    );
    assert_eq!(
        repo.delete_acked_items().await.unwrap(),
        0,
        "d2 has not read them"
    );
    repo.ack(d2.id, i1).await.unwrap();
    assert_eq!(repo.delete_acked_items().await.unwrap(), 1);
    assert_eq!(
        repo.list_items(user.id, d1.id, 0, 10).await.unwrap().len(),
        1
    );

    // A device added after an item starts past it and does not hold it.
    let d3 = device(&ctx.db, user.id, 3).await;
    assert_eq!(repo.device_cursor(d3.id).await.unwrap(), i2);
    repo.ack(d2.id, i2).await.unwrap();
    assert_eq!(repo.delete_acked_items().await.unwrap(), 1);

    // A revoked device holds nothing back.
    let env3 = repo
        .insert_envelope(envelope(Some(3), workspace))
        .await
        .unwrap();
    let i3 = repo.insert_item(item(user.id, env3)).await.unwrap();
    repo.ack(d1.id, i3).await.unwrap();
    repo.ack(d3.id, i3).await.unwrap();
    assert_eq!(repo.delete_acked_items().await.unwrap(), 0);
    SeaOrmKeyIdentityRepository::new(ctx.db.clone())
        .revoke_device(d2.id, "{}", "ed25519:r")
        .await
        .unwrap();
    assert_eq!(repo.delete_acked_items().await.unwrap(), 1);

    // An item for one device is that device's alone.
    let env4 = repo
        .insert_envelope(envelope(Some(4), workspace))
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        device_key_id: Some(d1.id),
        ..item(user.id, env4)
    })
    .await
    .unwrap();
    assert_eq!(
        repo.list_items(user.id, d1.id, i3, 10).await.unwrap().len(),
        1
    );
    assert!(
        repo.list_items(user.id, d3.id, i3, 10)
            .await
            .unwrap()
            .is_empty()
    );

    assert_eq!(
        repo.delete_orphan_envelopes().await.unwrap(),
        0,
        "young envelopes stay"
    );
    assert_eq!(
        repo.delete_expired_items(Utc::now() + Duration::days(31))
            .await
            .unwrap(),
        1
    );
}

#[tokio::test]
async fn the_oldest_items_go_over_the_cap_and_held_ones_stay() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let (w1, w2) = (Uuid::now_v7(), Uuid::now_v7());
    let request = repo
        .insert_request(NewDmRequest {
            status: DmRequestStatus::Pending,
            user_id: user.id,
            from_principal_id: "rimp1_alice".to_string(),
            card_payload: "{}".to_string(),
            card_signature: "ed25519:c".to_string(),
            host_server_id: HOST.to_string(),
            conversation_id: Uuid::now_v7(),
            expires_at: Utc::now() + Duration::days(30),
        })
        .await
        .unwrap();
    let held_env = repo.insert_envelope(envelope(Some(9), w1)).await.unwrap();
    repo.insert_item(NewRelayItem {
        request_id: Some(request.id),
        ..item(user.id, held_env)
    })
    .await
    .unwrap();
    for (seq, w) in [(1, w1), (2, w2), (3, w2)] {
        let env = repo.insert_envelope(envelope(Some(seq), w)).await.unwrap();
        repo.insert_item(item(user.id, env)).await.unwrap();
    }
    assert_eq!(repo.queued_bytes(user.id).await.unwrap(), 300);
    assert_eq!(repo.user_held_bytes(user.id).await.unwrap(), 100);
    assert_eq!(repo.request_held_bytes(request.id).await.unwrap(), 100);
    assert_eq!(
        repo.queue_stats().await.unwrap(),
        RelayQueueStats {
            queued_items: 3,
            queued_bytes: 300,
            held_items: 1,
            held_bytes: 100,
        }
    );
    let dropped = repo.drop_oldest(user.id, 150).await.unwrap();
    assert_eq!(
        dropped.iter().map(|space| space.removed_items).sum::<u64>(),
        2
    );
    let spaces: HashSet<Uuid> = dropped.iter().map(|d| d.workspace_id).collect();
    assert_eq!(spaces, HashSet::from([w1, w2]));
    assert_eq!(repo.queued_bytes(user.id).await.unwrap(), 100);
    assert_eq!(
        repo.queue_stats().await.unwrap(),
        RelayQueueStats {
            queued_items: 1,
            queued_bytes: 100,
            held_items: 1,
            held_bytes: 100,
        }
    );
    assert_eq!(
        repo.count_held(request.id).await.unwrap(),
        1,
        "held items stay"
    );
}

#[tokio::test]
async fn held_bytes_over_the_cap_leave_the_live_queue_alone() {
    const CAP: i64 = 1000;
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let w = Uuid::now_v7();
    for n in 0..3 {
        let request = repo
            .insert_request(NewDmRequest {
                status: DmRequestStatus::Pending,
                user_id: user.id,
                from_principal_id: format!("rimp1_stranger{n}"),
                card_payload: "{}".to_string(),
                card_signature: "ed25519:c".to_string(),
                host_server_id: HOST.to_string(),
                conversation_id: Uuid::now_v7(),
                expires_at: Utc::now() + Duration::days(30),
            })
            .await
            .unwrap();
        let env = repo.insert_envelope(envelope(None, w)).await.unwrap();
        repo.insert_item(NewRelayItem {
            request_id: Some(request.id),
            size_bytes: 600,
            ..item(user.id, env)
        })
        .await
        .unwrap();
    }
    for seq in 1..=4 {
        let env = repo.insert_envelope(envelope(Some(seq), w)).await.unwrap();
        repo.insert_item(item(user.id, env)).await.unwrap();
    }

    // The queue's cap is checked against its own bytes, so 1800 held bytes over a cap of
    // 1000 drop none of the 400 queued.
    let over = repo.queued_bytes(user.id).await.unwrap() - CAP;
    assert!(over <= 0, "held bytes count toward the queue: {over} over");
    assert_eq!(repo.user_held_bytes(user.id).await.unwrap(), 1800);
    let d = device(&ctx.db, user.id, 1).await;
    assert_eq!(
        repo.list_items(user.id, d.id, 0, 10).await.unwrap().len(),
        4,
        "every queued item is still there"
    );
}

#[tokio::test]
async fn requests_move_from_pending_once_and_hold_their_items() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let conversation = Uuid::now_v7();
    let new = |status, conversation_id| NewDmRequest {
        status,
        user_id: user.id,
        from_principal_id: "rimp1_alice".to_string(),
        card_payload: "{\"relays\":[\"https://a.example\"]}".to_string(),
        card_signature: "ed25519:c".to_string(),
        host_server_id: HOST.to_string(),
        conversation_id,
        expires_at: Utc::now() + Duration::days(30),
    };
    let request = repo
        .insert_request(new(DmRequestStatus::Pending, conversation))
        .await
        .unwrap();
    assert_eq!(request.status, DmRequestStatus::Pending);
    assert!(
        !repo.accepted_at_host(user.id, HOST).await.unwrap(),
        "a pending request is no accepted conversation"
    );
    let err = repo
        .insert_request(new(DmRequestStatus::Pending, conversation))
        .await
        .unwrap_err();
    assert_eq!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    );
    let accepted_at_once = repo
        .insert_request(NewDmRequest {
            expires_at: Utc::now() + Duration::days(36500),
            ..new(DmRequestStatus::Accepted, Uuid::now_v7())
        })
        .await
        .unwrap();
    assert_eq!(
        accepted_at_once.decided_at,
        Some(accepted_at_once.created_at)
    );
    assert!(
        repo.accepted_at_host(user.id, HOST).await.unwrap(),
        "an accepted conversation the host homes"
    );
    assert!(!repo.accepted_at_host(user.id, "rims1_other").await.unwrap());
    assert_eq!(repo.list_requests(user.id, None).await.unwrap().len(), 1);
    assert_eq!(
        repo.count_requests_from("rimp1_alice", Utc::now() - Duration::days(1))
            .await
            .unwrap(),
        1,
        "conversations from contacts are not requests"
    );
    assert_eq!(
        repo.count_requests_via_host(HOST, Utc::now() - Duration::days(1))
            .await
            .unwrap(),
        1,
        "a host's conversations from contacts are not requests"
    );
    assert_eq!(
        repo.count_requests_via_host("rims1_other", Utc::now() - Duration::days(1))
            .await
            .unwrap(),
        0
    );
    assert_eq!(repo.count_pending_requests(user.id).await.unwrap(), 1);
    let refused = Uuid::now_v7();
    repo.insert_request(new(DmRequestStatus::Ignored, refused))
        .await
        .unwrap();
    assert_eq!(
        repo.list_requests(user.id, None).await.unwrap().len(),
        1,
        "a refused conversation is no request"
    );
    let since = Utc::now() - Duration::days(1);
    assert_eq!(
        repo.count_requests_from("rimp1_alice", since)
            .await
            .unwrap(),
        1
    );
    assert!(
        repo.conversation_hidden(user.id, refused).await.unwrap(),
        "a refused conversation stays hidden"
    );
    assert_eq!(
        repo.oldest_request_from("rimp1_alice", since)
            .await
            .unwrap(),
        Some(request.created_at)
    );
    assert_eq!(
        repo.oldest_request_from("rimp1_carol", since)
            .await
            .unwrap(),
        None
    );

    let env = repo
        .insert_envelope(envelope(Some(1), Uuid::now_v7()))
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        request_id: Some(request.id),
        ..item(user.id, env)
    })
    .await
    .unwrap();
    assert_eq!(repo.held_items(request.id).await.unwrap().len(), 1);
    let d = device(&ctx.db, user.id, 1).await;
    assert!(
        repo.list_items(user.id, d.id, 0, 10)
            .await
            .unwrap()
            .is_empty(),
        "held items are not read"
    );

    let decided = repo
        .decide_request(
            request.id,
            DmRequestStatus::Accepted,
            Utc::now() + Duration::days(36500),
        )
        .await
        .unwrap()
        .expect("pending");
    assert_eq!(decided.status, DmRequestStatus::Accepted);
    assert!(
        repo.decide_request(request.id, DmRequestStatus::Ignored, Utc::now())
            .await
            .unwrap()
            .is_none(),
        "a decided request stays decided"
    );
    assert_eq!(repo.release_held(request.id).await.unwrap(), 1);
    assert_eq!(
        repo.list_items(user.id, d.id, 0, 10).await.unwrap().len(),
        1
    );
    assert!(repo.accepted_from(user.id, "rimp1_alice").await.unwrap());

    // Ignoring a sender's pending requests deletes what they hold.
    let other = repo
        .insert_request(NewDmRequest {
            from_principal_id: "rimp1_spam".to_string(),
            ..new(DmRequestStatus::Pending, Uuid::now_v7())
        })
        .await
        .unwrap();
    let env = repo
        .insert_envelope(envelope(Some(2), Uuid::now_v7()))
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        request_id: Some(other.id),
        ..item(user.id, env)
    })
    .await
    .unwrap();
    assert_eq!(
        repo.ignore_pending_from(user.id, "rimp1_spam", Utc::now() + Duration::days(30))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.count_held(other.id).await.unwrap(),
        1,
        "the invite stays with the request"
    );
    let env = repo
        .insert_envelope(envelope(Some(3), Uuid::now_v7()))
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        request_id: Some(other.id),
        ..item(user.id, env)
    })
    .await
    .unwrap();
    let first = repo.held_items(other.id).await.unwrap()[0].id;
    assert_eq!(repo.delete_held(other.id).await.unwrap(), 1);
    assert_eq!(repo.held_items(other.id).await.unwrap()[0].id, first);
    assert_eq!(
        repo.user_held_bytes(user.id).await.unwrap(),
        100,
        "the invite an ignored request keeps counts as held"
    );
    assert_eq!(
        repo.delete_expired_requests(Utc::now() + Duration::days(31))
            .await
            .unwrap(),
        2,
        "the ignored requests expire, the refused conversation's too; the accepted ones stay"
    );
}

#[tokio::test]
async fn an_expired_pending_request_cannot_be_decided() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let new = |expires_at| NewDmRequest {
        status: DmRequestStatus::Pending,
        user_id: user.id,
        from_principal_id: "rimp1_alice".to_string(),
        card_payload: "{}".to_string(),
        card_signature: "ed25519:c".to_string(),
        host_server_id: HOST.to_string(),
        conversation_id: Uuid::now_v7(),
        expires_at,
    };
    let expired = repo
        .insert_request(new(Utc::now() - Duration::minutes(1)))
        .await
        .unwrap();
    for status in [DmRequestStatus::Accepted, DmRequestStatus::Ignored] {
        assert!(
            repo.decide_request(expired.id, status, Utc::now() + Duration::days(30))
                .await
                .unwrap()
                .is_none(),
            "an expired request stays undecided: {status:?}"
        );
    }
    let stored = repo
        .get_request(user.id, expired.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(stored.status, DmRequestStatus::Pending);
    assert_eq!(stored.decided_at, None);

    let live = repo
        .insert_request(new(Utc::now() + Duration::days(30)))
        .await
        .unwrap();
    let decided = repo
        .decide_request(
            live.id,
            DmRequestStatus::Accepted,
            Utc::now() + Duration::days(36500),
        )
        .await
        .unwrap()
        .expect("a live request is decided");
    assert_eq!(decided.status, DmRequestStatus::Accepted);
}

#[tokio::test]
async fn contacts_blocks_and_hosts_of_a_user() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let contact = repo
        .upsert_contact(
            user.id,
            "rimp1_alice",
            vec!["https://a.example".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(contact.relay_urls, vec!["https://a.example"]);
    let contact = repo
        .upsert_contact(
            user.id,
            "rimp1_alice",
            vec!["https://b.example".to_string()],
        )
        .await
        .unwrap();
    assert_eq!(contact.relay_urls, vec!["https://b.example"]);
    assert!(repo.is_contact(user.id, "rimp1_alice").await.unwrap());
    assert!(repo.delete_contact(user.id, "rimp1_alice").await.unwrap());
    assert!(!repo.delete_contact(user.id, "rimp1_alice").await.unwrap());

    assert!(repo.block(user.id, "rimp1_spam").await.unwrap());
    assert!(!repo.block(user.id, "rimp1_spam").await.unwrap());
    assert!(repo.is_blocked(user.id, "rimp1_spam").await.unwrap());
    assert!(repo.unblock(user.id, "rimp1_spam").await.unwrap());

    let env = repo
        .insert_envelope(envelope(Some(1), Uuid::now_v7()))
        .await
        .unwrap();
    repo.insert_item(item(user.id, env)).await.unwrap();
    assert_eq!(
        repo.hosts_of_user(user.id).await.unwrap(),
        vec![HOST.to_string()]
    );
    repo.store_identity("rimp1_alice", HOST, "{}")
        .await
        .unwrap();
    let cached = repo.cached_identity("rimp1_alice").await.unwrap().unwrap();
    assert_eq!(cached.served_by, HOST);
    repo.clear_user(user.id).await.unwrap();
    assert!(repo.hosts_of_user(user.id).await.unwrap().is_empty());
}

#[tokio::test]
async fn people_know_each_other_through_mutual_contacts_or_an_accepted_request() {
    let ctx = support::TestCtx::new().await;
    let ada = ctx.create_key_only_user("ada", "rimp1_ada").await;
    let bob = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let urls = || vec!["https://a.example".to_string()];
    let request = |user_id: UserId, from: &str, status| NewDmRequest {
        status,
        user_id,
        from_principal_id: from.to_string(),
        card_payload: "{}".to_string(),
        card_signature: "ed25519:c".to_string(),
        host_server_id: HOST.to_string(),
        conversation_id: Uuid::now_v7(),
        expires_at: Utc::now() + Duration::days(30),
    };

    // A contact Ada added on her own: neither knows the other.
    repo.upsert_contact(ada.id, "rimp1_bob", urls())
        .await
        .unwrap();
    assert_eq!(repo.count_contacts(ada.id).await.unwrap(), 1);
    assert!(!repo.knows(ada.id, "rimp1_bob").await.unwrap());
    assert!(!repo.knows(bob.id, "rimp1_ada").await.unwrap());

    // Once Bob lists Ada too, they know each other both ways.
    repo.upsert_contact(bob.id, "rimp1_ada", urls())
        .await
        .unwrap();
    assert!(repo.knows(ada.id, "rimp1_bob").await.unwrap());
    assert!(repo.knows(bob.id, "rimp1_ada").await.unwrap());
    repo.delete_contact(ada.id, "rimp1_bob").await.unwrap();
    assert!(
        !repo.knows(bob.id, "rimp1_ada").await.unwrap(),
        "Bob's list alone is one-sided too"
    );
    repo.delete_contact(bob.id, "rimp1_ada").await.unwrap();

    // A request Bob holds from Ada counts once he accepted it, for both of them.
    let pending = repo
        .insert_request(request(bob.id, "rimp1_ada", DmRequestStatus::Pending))
        .await
        .unwrap();
    assert!(!repo.knows(ada.id, "rimp1_bob").await.unwrap());
    assert!(!repo.knows(bob.id, "rimp1_ada").await.unwrap());
    repo.decide_request(
        pending.id,
        DmRequestStatus::Accepted,
        Utc::now() + Duration::days(30),
    )
    .await
    .unwrap();
    assert!(repo.knows(ada.id, "rimp1_bob").await.unwrap());
    assert!(repo.knows(bob.id, "rimp1_ada").await.unwrap());

    // Someone on another server is known through a request accepted from them only.
    repo.upsert_contact(ada.id, "rimp1_carol", urls())
        .await
        .unwrap();
    assert!(!repo.knows(ada.id, "rimp1_carol").await.unwrap());
    repo.insert_request(request(ada.id, "rimp1_carol", DmRequestStatus::Accepted))
        .await
        .unwrap();
    assert!(repo.knows(ada.id, "rimp1_carol").await.unwrap());
    assert_eq!(repo.count_contacts(ada.id).await.unwrap(), 1);
}

#[tokio::test]
async fn deleting_users_devices_and_requests_cascades() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let d = device(&ctx.db, user.id, 1).await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let env = repo
        .insert_envelope(envelope(Some(1), Uuid::now_v7()))
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        device_key_id: Some(d.id),
        ..item(user.id, env)
    })
    .await
    .unwrap();
    repo.ack(d.id, 1).await.unwrap();
    repo.upsert_contact(
        user.id,
        "rimp1_alice",
        vec!["https://a.example".to_string()],
    )
    .await
    .unwrap();
    repo.block(user.id, "rimp1_spam").await.unwrap();
    SeaOrmHostRepository::new(ctx.db.clone())
        .check_request_seq(d.id, 10, 300, Utc::now() + Duration::minutes(10))
        .await
        .unwrap();

    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM device_key WHERE id = '{}'",
            Uuid::from(d.id)
        ))
        .await
        .unwrap();
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_item").await,
        0
    );
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_device_cursor").await,
        0
    );
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM device_request_seen").await,
        0
    );

    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM \"user\" WHERE id = '{}'",
            Uuid::from(user.id)
        ))
        .await
        .unwrap();
    for table in ["relay_contact", "relay_block", "relay_dm_request"] {
        assert_eq!(
            count(&ctx.db, &format!("SELECT count(*) AS n FROM {table}")).await,
            0,
            "{table}"
        );
    }
}

#[tokio::test]
async fn the_request_seq_window_refuses_replays_and_old_seqs() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let d = device(&ctx.db, user.id, 1).await;
    let repo = SeaOrmHostRepository::new(ctx.db.clone());
    let until = Utc::now() + Duration::minutes(10);
    let window = 300_000_000;
    let base = 1_791_504_000_000_000;
    assert_eq!(
        repo.check_request_seq(d.id, base, window, until)
            .await
            .unwrap(),
        SeqDecision::Accepted
    );
    assert_eq!(
        repo.check_request_seq(d.id, base, window, until)
            .await
            .unwrap(),
        SeqDecision::Replay { highest: base }
    );
    assert_eq!(
        repo.check_request_seq(d.id, base - 1_000, window, until)
            .await
            .unwrap(),
        SeqDecision::Accepted,
        "reordered requests inside the window pass"
    );
    assert_eq!(
        repo.check_request_seq(d.id, base - window, window, until)
            .await
            .unwrap(),
        SeqDecision::Replay { highest: base }
    );
    assert_eq!(
        repo.check_request_seq(d.id, base + 5, window, until)
            .await
            .unwrap(),
        SeqDecision::Accepted
    );
    // Seen seqs inside the window are kept past their time; old ones go.
    assert_eq!(
        repo.purge_request_seen(Utc::now() + Duration::hours(1), window)
            .await
            .unwrap(),
        0
    );
    repo.check_request_seq(d.id, base + 2 * window, window, until)
        .await
        .unwrap();
    assert_eq!(
        repo.purge_request_seen(Utc::now() + Duration::hours(1), window)
            .await
            .unwrap(),
        3
    );
    assert_eq!(
        repo.check_request_seq(d.id, base + 5, window, until)
            .await
            .unwrap(),
        SeqDecision::Replay {
            highest: base + 2 * window
        },
        "a purged seq is below the window"
    );
}

#[tokio::test]
async fn the_direct_space_is_unique_and_unlisted_and_remote_users_are_members() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmHostRepository::new(ctx.db.clone());
    let direct = repo.ensure_direct_space("Direct messages").await.unwrap();
    assert_eq!(
        repo.ensure_direct_space("Direct messages").await.unwrap(),
        direct
    );
    assert_eq!(repo.direct_space().await.unwrap(), Some(direct));

    let local = ctx.create_key_only_user("alice", "rimp1_alice").await;
    assert!(repo.join_as_guest(direct, local.id).await.unwrap());
    assert!(!repo.join_as_guest(direct, local.id).await.unwrap());
    let servers = SeaOrmServerRepository::new(ctx.db.clone());
    assert!(
        servers
            .get_servers_for_user(local.id)
            .await
            .unwrap()
            .is_empty()
    );

    let remote = repo
        .create_remote_user(NewRemoteUser {
            principal_id: "rimp1_bob".to_string(),
            identity_key: "ed25519:bob".to_string(),
            username: "r_bob".to_string(),
            display_name: "Bob".to_string(),
            home_relay: Some("rims1_relay".to_string()),
        })
        .await
        .unwrap();
    assert_eq!(
        repo.find_user("rimp1_bob").await.unwrap(),
        Some((remote, UserKind::Remote))
    );
    repo.join_as_guest(direct, remote).await.unwrap();
    let members = repo.remote_members(direct).await.unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].principal_id, "rimp1_bob");
    assert!(members[0].role.is_guest());
    assert_eq!(members[0].home_relay.as_deref(), Some("rims1_relay"));

    repo.make_local(remote, "bob", "Bob", Some("bob@example.com"))
        .await
        .unwrap();
    assert_eq!(repo.user_kind(remote).await.unwrap(), Some(UserKind::Local));
    assert!(repo.remote_members(direct).await.unwrap().is_empty());
    let users = SeaOrmUserRepository::new(ctx.db.clone());
    let bob = users.get_by_id(remote).await.unwrap().expect("bob");
    assert_eq!(bob.email.as_deref(), Some("bob@example.com"));
    assert!(
        repo.make_local(remote, "bob2", "Bob", None).await.is_err(),
        "only remote users"
    );
}

#[tokio::test]
async fn the_migration_refuses_down_while_remote_users_or_items_exist() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    let steps = u32::try_from(names.len() - index).expect("steps fit u32");

    let repo = SeaOrmHostRepository::new(ctx.db.clone());
    let remote = repo
        .create_remote_user(NewRemoteUser {
            principal_id: "rimp1_bob".to_string(),
            identity_key: "ed25519:bob".to_string(),
            username: "r_bob".to_string(),
            display_name: "Bob".to_string(),
            home_relay: None,
        })
        .await
        .unwrap();
    assert!(
        Migrator::down(&conn, Some(steps)).await.is_err(),
        "a remote user blocks the down migration"
    );
    // Each migration commits on its own: the later ones rolled back before the refusal.
    Migrator::up(&conn, None).await.expect("up again");
    assert!(table_exists(&conn, "relay_item").await);
    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM \"user\" WHERE id = '{}'",
            Uuid::from(remote)
        ))
        .await
        .unwrap();

    Migrator::down(&conn, Some(steps)).await.expect("down");
    for table in [
        "relay_item",
        "relay_envelope",
        "s2s_outbox",
        "host_event_stream",
    ] {
        assert!(!table_exists(&conn, table).await, "{table} after down");
    }
    Migrator::up(&conn, None).await.expect("up");
    for table in [
        "relay_item",
        "relay_envelope",
        "s2s_outbox",
        "host_event_stream",
    ] {
        assert!(table_exists(&conn, table).await, "{table} after up");
    }
    let err = conn
        .execute_unprepared(
            "INSERT INTO server (id, name, kind) VALUES (gen_random_uuid(), 'x', 'other')",
        )
        .await;
    assert!(err.is_err(), "server.kind is checked");
    conn.execute_unprepared(
        "INSERT INTO server (id, name, kind) VALUES (gen_random_uuid(), 'a', 'direct')",
    )
    .await
    .unwrap();
    let err = conn
        .execute_unprepared(
            "INSERT INTO server (id, name, kind) VALUES (gen_random_uuid(), 'b', 'direct')",
        )
        .await;
    assert!(err.is_err(), "one direct space");
}

/// What a user sees of one conversation: notifications listed, the bell's count, digest rows,
/// whether the digest picks them and the unread count.
async fn visible_signals(
    db: &DbHandle,
    user_id: UserId,
    server_id: rim_core::server::model::ServerId,
    conversation_id: rim_core::conversation::model::ConversationId,
) -> (usize, u64, usize, bool, usize) {
    use rim_core::conversation_read_state::repository::ConversationReadStateRepository;
    use rim_core::notification::repository::{ListUserNotificationsParams, NotificationRepository};
    use rim_core::repository::pagination::PageDir;
    use rim_infra_db::sea_orm_impl::conversation_read_state::SeaOrmConversationReadStateRepository;
    use rim_infra_db::sea_orm_impl::notification::SeaOrmNotificationRepository;

    let notifications = SeaOrmNotificationRepository::new(db.clone());
    let listed = notifications
        .list_notifications(ListUserNotificationsParams {
            user_id,
            server_id,
            unread_only: false,
            limit: 50,
            dir: PageDir::After,
            cursor: None,
        })
        .await
        .unwrap()
        .items
        .len();
    let bell = notifications
        .unread_count(user_id, server_id)
        .await
        .unwrap();
    let digest_rows = notifications
        .list_pending_digest(user_id, server_id, 50)
        .await
        .unwrap()
        .len();
    let later = Utc::now() + Duration::hours(1);
    let digest_candidate = notifications
        .list_digest_candidates(later, later)
        .await
        .unwrap()
        .iter()
        .any(|c| c.user_id == user_id);
    let unread = SeaOrmConversationReadStateRepository::new(db.clone())
        .get_unread_counts(user_id, &[conversation_id])
        .await
        .unwrap()
        .len();
    (listed, bell, digest_rows, digest_candidate, unread)
}

#[tokio::test]
async fn a_hidden_conversation_stays_out_of_notifications_and_counts() {
    use rim_core::conversation::model::{ConversationType, NewConversation};
    use rim_core::conversation::repository::ConversationRepository;
    use rim_core::notification::model::{
        DigestFrequency, NewUserNotification, NotificationReason, UserNotificationSettings,
    };
    use rim_core::notification::repository::NotificationRepository;
    use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
    use rim_infra_db::sea_orm_impl::notification::SeaOrmNotificationRepository;

    let ctx = support::TestCtx::new().await;
    let direct = SeaOrmHostRepository::new(ctx.db.clone())
        .ensure_direct_space("Direct messages")
        .await
        .unwrap();
    let alice = ctx.create_key_only_user("alice", "rimp1_alice").await;
    let bob = ctx.create_user("bob").await;
    let conversation = SeaOrmConversationRepository::new(ctx.db.clone())
        .create_conversation(NewConversation {
            server_id: direct,
            conversation_type: ConversationType::Direct,
            title: None,
            is_private: true,
            created_by: Some(alice.id),
            encrypted: false,
        })
        .await
        .unwrap();
    let message = ctx
        .create_message(conversation.id, alice.id, "hello <@bob>")
        .await;
    let notifications = SeaOrmNotificationRepository::new(ctx.db.clone());
    notifications
        .upsert_settings(UserNotificationSettings {
            digest: DigestFrequency::Hourly,
            ..UserNotificationSettings::defaults(bob.id, direct, Utc::now())
        })
        .await
        .unwrap();
    notifications
        .create_notifications(
            &[NewUserNotification {
                user_id: bob.id,
                server_id: direct,
                conversation_id: conversation.id,
                message_id: message.id,
                reason: NotificationReason::Direct,
            }],
            Utc::now(),
        )
        .await
        .unwrap();
    let shown = (1, 1, 1, true, 1);
    let hidden = (0, 0, 0, false, 0);
    let signals = || visible_signals(&ctx.db, bob.id, direct, conversation.id);
    assert_eq!(signals().await, shown);

    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let request = repo
        .insert_request(NewDmRequest {
            status: DmRequestStatus::Pending,
            user_id: bob.id,
            from_principal_id: "rimp1_alice".to_string(),
            card_payload: "{}".to_string(),
            card_signature: "ed25519:c".to_string(),
            host_server_id: HOST.to_string(),
            conversation_id: conversation.id.into(),
            expires_at: Utc::now() + Duration::days(30),
        })
        .await
        .unwrap();
    assert!(
        repo.conversation_hidden(bob.id, conversation.id.into())
            .await
            .unwrap()
    );
    assert_eq!(
        signals().await,
        hidden,
        "a pending request hides everything"
    );
    repo.decide_request(
        request.id,
        DmRequestStatus::Accepted,
        Utc::now() + Duration::days(36500),
    )
    .await
    .unwrap();
    assert_eq!(signals().await, shown, "accepting brings everything back");

    repo.block(bob.id, "rimp1_alice").await.unwrap();
    assert_eq!(
        signals().await,
        hidden,
        "a blocked creator's conversation is hidden"
    );
    repo.unblock(bob.id, "rimp1_alice").await.unwrap();
    assert_eq!(signals().await, shown);
    assert!(
        !repo
            .conversation_hidden(alice.id, conversation.id.into())
            .await
            .unwrap(),
        "only the request's holder is affected"
    );
}
