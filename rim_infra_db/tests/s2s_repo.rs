#![cfg(feature = "db-tests")]

//! Server identity, pinned peers and request nonces against a real database: conflicts, the
//! conditional updates, cascades, a nonce inserted twice at once, and the migration down and up.

mod support;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::key_identity::model::KeyLogEntryType;
use rim_core::repository::error::RepoErrorKind;
use rim_core::s2s::model::{
    PeerPin, PeerRotationSigner, PeerStatus, ServerDescriptorRecord, ServerKeyLogRecord,
    SignedRecord,
};
use rim_core::s2s::repository::{
    PeerServerRepository, S2sNonceRepository, ServerIdentityRepository,
};
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::s2s::{
    SeaOrmPeerServerRepository, SeaOrmS2sNonceRepository, SeaOrmServerIdentityRepository,
};
use sea_orm::{ConnectionTrait, DbBackend, Statement};

const MIGRATION_NAME: &str = "m20261009_214158_add_server_identity";
const PEER: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

fn record(text: &str) -> SignedRecord {
    SignedRecord {
        payload: format!("{{\"p\":\"{text}\"}}"),
        signature: format!("ed25519:{text}"),
    }
}

fn pin(head_seq: u32) -> PeerPin {
    PeerPin {
        server_id: PEER.to_string(),
        origin: "https://peer.example".to_string(),
        head_seq,
        head_key: format!("ed25519:key{head_seq}"),
        previous_key: (head_seq > 0).then(|| format!("ed25519:key{}", head_seq - 1)),
        rotated_at: (head_seq > 0).then(Utc::now),
        rotation_signer: (head_seq > 0).then_some(PeerRotationSigner::Server),
        descriptor_version: head_seq + 1,
        descriptor: record(&format!("d{head_seq}")),
        log: (0..=head_seq).map(|i| record(&format!("e{i}"))).collect(),
    }
}

#[tokio::test]
async fn key_log_entries_are_unique_per_seq_and_descriptors_only_move_up() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmServerIdentityRepository::new(ctx.db.clone());
    assert!(repo.key_log().await.unwrap().is_empty());
    assert!(repo.descriptor().await.unwrap().is_none());

    let entry = |seq: u32, kind| ServerKeyLogRecord {
        seq,
        entry_type: kind,
        statement: record(&format!("s{seq}")),
    };
    repo.append_key_log_entry(entry(0, KeyLogEntryType::Inception))
        .await
        .unwrap();
    repo.append_key_log_entry(entry(1, KeyLogEntryType::Rotate))
        .await
        .unwrap();
    let err = repo
        .append_key_log_entry(entry(1, KeyLogEntryType::Rotate))
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
    let log = repo.key_log().await.unwrap();
    assert_eq!(
        log,
        vec![
            entry(0, KeyLogEntryType::Inception),
            entry(1, KeyLogEntryType::Rotate)
        ]
    );

    let descriptor = |version: u32| ServerDescriptorRecord {
        version,
        statement: record(&format!("v{version}")),
    };
    assert!(repo.store_descriptor(descriptor(2)).await.unwrap());
    assert!(!repo.store_descriptor(descriptor(2)).await.unwrap());
    assert!(!repo.store_descriptor(descriptor(1)).await.unwrap());
    assert!(repo.store_descriptor(descriptor(3)).await.unwrap());
    assert_eq!(repo.descriptor().await.unwrap(), Some(descriptor(3)));
}

#[tokio::test]
async fn a_peers_checked_handle_domain_is_kept_across_fetches() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let inserted = repo.insert(pin(0), PeerStatus::Active).await.unwrap();
    assert_eq!(inserted.handle_domain, None);
    repo.set_handle_domain(PEER, Some("rim-platform.org".to_string()))
        .await
        .unwrap();
    repo.mark_fetched(PEER, None).await.unwrap();
    let advanced = repo.advance(pin(2), 0).await.unwrap().expect("advanced");
    assert_eq!(advanced.handle_domain.as_deref(), Some("rim-platform.org"));
    repo.set_handle_domain(PEER, None).await.unwrap();
    assert_eq!(repo.find(PEER).await.unwrap().unwrap().handle_domain, None);
}

#[tokio::test]
async fn a_pin_advances_only_from_the_expected_head_while_active() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let inserted = repo.insert(pin(0), PeerStatus::Active).await.unwrap();
    assert_eq!(inserted.status, PeerStatus::Active);
    assert_eq!(inserted.head_seq, 0);
    assert_eq!(
        repo.insert(pin(0), PeerStatus::Active)
            .await
            .unwrap_err()
            .kind(),
        RepoErrorKind::Conflict
    );
    assert_eq!(repo.key_log(PEER).await.unwrap(), pin(0).log);

    // From a head that is not the stored one: nothing changes.
    assert!(repo.advance(pin(2), 1).await.unwrap().is_none());
    let advanced = repo.advance(pin(2), 0).await.unwrap().expect("advanced");
    assert_eq!(advanced.head_seq, 2);
    assert_eq!(advanced.previous_key.as_deref(), Some("ed25519:key1"));
    assert_eq!(advanced.rotation_signer, Some(PeerRotationSigner::Server));
    assert_eq!(advanced.first_seen_at, inserted.first_seen_at);
    assert_eq!(repo.key_log(PEER).await.unwrap(), pin(2).log);

    // A status changes only from the one expected.
    assert!(
        !repo
            .replace_status(PEER, PeerStatus::Blocked, PeerStatus::Active)
            .await
            .unwrap()
    );
    assert!(
        repo.replace_status(PEER, PeerStatus::Active, PeerStatus::Blocked)
            .await
            .unwrap()
    );
    assert!(repo.advance(pin(3), 2).await.unwrap().is_none(), "blocked");
    assert!(
        repo.replace_status(PEER, PeerStatus::Blocked, PeerStatus::Active)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .replace_status("rims1_unknown", PeerStatus::Active, PeerStatus::Blocked)
            .await
            .unwrap()
    );

    // A forked peer does not advance.
    assert!(repo.set_status(PEER, PeerStatus::Forked).await.unwrap());
    assert!(repo.advance(pin(3), 2).await.unwrap().is_none());
    assert_eq!(
        repo.find(PEER).await.unwrap().unwrap().status,
        PeerStatus::Forked
    );
}

#[tokio::test]
async fn mark_fetched_takes_only_a_newer_descriptor_and_delete_cascades() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let before = repo.insert(pin(1), PeerStatus::Active).await.unwrap();
    assert_eq!(before.descriptor_version, 2);

    repo.mark_fetched(PEER, Some((1, record("older"))))
        .await
        .unwrap();
    let after = repo.find(PEER).await.unwrap().unwrap();
    assert_eq!(after.descriptor, before.descriptor);
    repo.mark_fetched(PEER, Some((5, record("newer"))))
        .await
        .unwrap();
    let after = repo.find(PEER).await.unwrap().unwrap();
    assert_eq!(after.descriptor_version, 5);
    assert_eq!(after.descriptor, record("newer"));
    repo.mark_fetched(PEER, None).await.unwrap();
    assert!(repo.find(PEER).await.unwrap().unwrap().fetched_at >= after.fetched_at);

    repo.touch(PEER).await.unwrap();
    assert!(
        repo.find(PEER)
            .await
            .unwrap()
            .unwrap()
            .last_request_at
            .is_some()
    );

    assert!(repo.delete(PEER).await.unwrap());
    assert!(!repo.delete(PEER).await.unwrap());
    assert!(repo.find(PEER).await.unwrap().is_none());
    assert!(repo.key_log(PEER).await.unwrap().is_empty());
}

#[tokio::test]
async fn a_nonce_is_accepted_once_and_expired_ones_are_deleted() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmS2sNonceRepository::new(ctx.db.clone());
    let later = Utc::now() + Duration::minutes(10);
    assert!(repo.insert(PEER, "n1", later).await.unwrap());
    assert!(!repo.insert(PEER, "n1", later).await.unwrap());
    // The same nonce from another peer is that peer's own.
    assert!(repo.insert("rims1_other", "n1", later).await.unwrap());

    assert!(
        repo.insert(PEER, "old", Utc::now() - Duration::seconds(1))
            .await
            .unwrap()
    );
    assert_eq!(repo.delete_expired().await.unwrap(), 1);
    assert!(!repo.insert(PEER, "n1", later).await.unwrap());
    assert!(repo.insert(PEER, "old", later).await.unwrap());
}

#[tokio::test]
async fn one_of_two_concurrent_inserts_of_a_nonce_wins() {
    let _ctx = support::TestCtx::new().await;
    // Two pools, so the inserts run on separate connections at the same time.
    let a = SeaOrmS2sNonceRepository::new(DbHandle::from(support::db_conn().await));
    let b = SeaOrmS2sNonceRepository::new(DbHandle::from(support::db_conn().await));
    let expires = Utc::now() + Duration::minutes(10);
    for i in 0..25 {
        let nonce = format!("race-{i}");
        let (first, second) = tokio::join!(
            a.insert(PEER, &nonce, expires),
            b.insert(PEER, &nonce, expires)
        );
        let wins = [first.unwrap(), second.unwrap()]
            .iter()
            .filter(|won| **won)
            .count();
        assert_eq!(wins, 1, "nonce {nonce}");
    }
}

async fn table_exists(conn: &sea_orm::DatabaseConnection, table: &str) -> bool {
    conn.query_one_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT to_regclass($1)::text AS name",
        [table.into()],
    ))
    .await
    .unwrap()
    .and_then(|row| row.try_get::<Option<String>>("", "name").ok().flatten())
    .is_some()
}

#[tokio::test]
async fn the_migration_goes_down_and_up() {
    let _ctx = support::TestCtx::new().await;
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
    let tables = [
        "server_key_log_entry",
        "server_descriptor",
        "peer_server",
        "peer_key_log_entry",
        "s2s_nonce",
    ];

    Migrator::down(&conn, Some(steps)).await.expect("down");
    for table in tables {
        assert!(!table_exists(&conn, table).await, "{table} after down");
    }
    Migrator::up(&conn, None).await.expect("up");
    for table in tables {
        assert!(table_exists(&conn, table).await, "{table} after up");
    }
    let err = conn
        .execute_unprepared(
            "INSERT INTO server_descriptor (id, version, payload, signature) VALUES (2, 1, '', '')",
        )
        .await;
    assert!(err.is_err(), "only id 1 is allowed");
    let err = conn
        .execute_unprepared(
            "INSERT INTO server_key_log_entry (seq, entry_type, payload, signature) \
             VALUES (0, 'other', '', '')",
        )
        .await;
    assert!(err.is_err(), "entry_type is checked");
}
