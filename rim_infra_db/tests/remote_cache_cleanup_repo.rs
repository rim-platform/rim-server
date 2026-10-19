#![cfg(feature = "db-tests")]

//! The purges of stale cached remote identities and idle peer pins against a real database: what
//! each keeps and deletes, the key-log head kept for a forgotten pin, and the limit of pins per
//! origin.

mod support;

use chrono::{Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use rim_core::relay::repository::RelayRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::s2s::model::{MAX_PEERS_PER_ORIGIN, PeerPin, PeerStatus, SignedRecord};
use rim_core::s2s::repository::PeerServerRepository;
use rim_core::user::model::UserId;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use rim_infra_db::sea_orm_impl::s2s::SeaOrmPeerServerRepository;

async fn execute(db: &DbHandle, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

async fn count(db: &DbHandle, sql: &str) -> i64 {
    db.query_one_raw(Statement::from_sql_and_values(DbBackend::Postgres, sql, []))
        .await
        .unwrap()
        .unwrap()
        .try_get::<i64>("", "n")
        .unwrap()
}

fn record(text: &str) -> SignedRecord {
    SignedRecord {
        payload: format!("{{\"p\":\"{text}\"}}"),
        signature: format!("ed25519:{text}"),
    }
}

/// A pin of `server_id` at `origin` whose log runs to `head_seq`.
fn pin(server_id: &str, origin: &str, head_seq: u32) -> PeerPin {
    PeerPin {
        server_id: server_id.to_string(),
        origin: origin.to_string(),
        head_seq,
        head_key: format!("ed25519:key{head_seq}"),
        previous_key: None,
        rotated_at: None,
        rotation_signer: None,
        descriptor_version: 1,
        descriptor: record("d"),
        log: (0..=head_seq)
            .map(|i| record(&format!("{server_id}-e{i}")))
            .collect(),
    }
}

/// Make a pin look last fetched, first seen and last asked `days` ago.
async fn age_pin(db: &DbHandle, server_id: &str, days: i32) {
    execute(
        db,
        "UPDATE peer_server SET first_seen_at = now() - $2::int * interval '1 day', \
         fetched_at = now() - $2::int * interval '1 day', \
         last_request_at = now() - $2::int * interval '1 day' WHERE server_id = $1",
        vec![server_id.into(), days.into()],
    )
    .await;
}

/// Turn a local user into a remote one with `principal_id` and, when given, `home_relay`.
async fn make_remote(db: &DbHandle, user_id: UserId, principal_id: &str, home: Option<&str>) {
    execute(
        db,
        "UPDATE \"user\" SET kind = 'remote', principal_id = $2, identity_key = 'ed25519:x', \
         home_relay = $3 WHERE id = $1",
        vec![
            Uuid::from(user_id).into(),
            principal_id.into(),
            home.map(str::to_string).into(),
        ],
    )
    .await;
}

#[tokio::test]
async fn idle_unreferenced_active_pins_are_forgotten_with_their_head() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let cutoff = Utc::now() - Duration::days(90);

    // Idle and unreferenced: forgotten.
    repo.insert(
        pin("rims1_idle", "https://idle.example", 2),
        PeerStatus::Active,
    )
    .await
    .unwrap();
    age_pin(&ctx.db, "rims1_idle", 100).await;
    // Fetched recently, asked recently, or seen recently: kept.
    for (id, column) in [
        ("rims1_fetched", "fetched_at"),
        ("rims1_asked", "last_request_at"),
        ("rims1_new", "first_seen_at"),
    ] {
        repo.insert(
            pin(id, &format!("https://{id}.example"), 0),
            PeerStatus::Active,
        )
        .await
        .unwrap();
        age_pin(&ctx.db, id, 100).await;
        execute(
            &ctx.db,
            &format!("UPDATE peer_server SET {column} = now() WHERE server_id = $1"),
            vec![id.into()],
        )
        .await;
    }
    // Never asked, fetched long ago: forgotten as well.
    repo.insert(
        pin("rims1_unasked", "https://unasked.example", 0),
        PeerStatus::Active,
    )
    .await
    .unwrap();
    age_pin(&ctx.db, "rims1_unasked", 100).await;
    execute(
        &ctx.db,
        "UPDATE peer_server SET last_request_at = NULL WHERE server_id = 'rims1_unasked'",
        vec![],
    )
    .await;
    // Forked, blocked and listed in the configuration: kept however idle.
    for (id, status) in [
        ("rims1_forked", PeerStatus::Forked),
        ("rims1_blocked", PeerStatus::Blocked),
        ("rims1_listed", PeerStatus::Active),
    ] {
        repo.insert(pin(id, &format!("https://{id}.example"), 0), status)
            .await
            .unwrap();
        age_pin(&ctx.db, id, 100).await;
    }

    let mut forgotten = repo
        .forget_idle(cutoff, &["rims1_listed".to_string()])
        .await
        .unwrap();
    forgotten.sort();
    assert_eq!(forgotten, vec!["rims1_idle", "rims1_unasked"]);
    assert!(repo.find("rims1_idle").await.unwrap().is_none());
    assert!(repo.key_log("rims1_idle").await.unwrap().is_empty());
    let head = repo.forgotten("rims1_idle").await.unwrap().expect("head");
    assert_eq!(head.head_seq, 2);
    assert_eq!(head.head_payload, record("rims1_idle-e2").payload);
    for id in [
        "rims1_fetched",
        "rims1_asked",
        "rims1_new",
        "rims1_forked",
        "rims1_blocked",
        "rims1_listed",
    ] {
        assert!(repo.find(id).await.unwrap().is_some(), "{id} kept");
        assert!(
            repo.forgotten(id).await.unwrap().is_none(),
            "{id} has no head"
        );
    }
    // Without the configuration's list, the listed pin goes too.
    assert_eq!(
        repo.forget_idle(cutoff, &[]).await.unwrap(),
        vec!["rims1_listed"]
    );
}

#[tokio::test]
async fn pins_something_here_refers_to_are_kept() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let refs = [
        "rims1_home",
        "rims1_subscription",
        "rims1_outbox",
        "rims1_inbound",
        "rims1_envelope",
        "rims1_request",
        "rims1_cache",
        "rims1_card",
    ];
    for id in refs.iter().chain(&["rims1_free"]) {
        repo.insert(
            pin(id, &format!("https://{id}.example"), 0),
            PeerStatus::Active,
        )
        .await
        .unwrap();
        age_pin(&ctx.db, id, 100).await;
    }
    let local = ctx.create_user("cleanup_local").await;
    let home = ctx.create_user("cleanup_home").await;
    make_remote(&ctx.db, home.id, "rimp1_home", Some("rims1_home")).await;
    let carded = ctx.create_user("cleanup_carded").await;
    make_remote(&ctx.db, carded.id, "rimp1_carded", None).await;
    let db = &ctx.db;
    execute(
        db,
        "INSERT INTO profile_card (user_id, version, payload, signature) \
         VALUES ($1, 1, '{\"relays\":[\"https://rims1_card.example\"]}', 'ed25519:x')",
        vec![Uuid::from(carded.id).into()],
    )
    .await;
    execute(
        db,
        "INSERT INTO relay_subscription (id, user_id, host_server_id, workspace_id, status) \
         VALUES ($1, $2, 'rims1_subscription', $1, 'ended')",
        vec![Uuid::now_v7().into(), Uuid::from(local.id).into()],
    )
    .await;
    execute(
        db,
        "INSERT INTO s2s_outbox (peer_server_id, item, size_bytes) VALUES ('rims1_outbox', '{}', 2)",
        vec![],
    )
    .await;
    execute(
        db,
        "INSERT INTO s2s_peer_inbound (peer_server_id) VALUES ('rims1_inbound')",
        vec![],
    )
    .await;
    execute(
        db,
        "INSERT INTO relay_envelope (host_server_id, workspace_id, stream, payload, signature) \
         VALUES ('rims1_envelope', $1, 'm', '{}', 'ed25519:x')",
        vec![Uuid::now_v7().into()],
    )
    .await;
    execute(
        db,
        "INSERT INTO relay_dm_request (id, user_id, from_principal_id, card_payload, \
         card_signature, host_server_id, conversation_id, expires_at) \
         VALUES ($1, $2, 'rimp1_x', '{}', 'ed25519:x', 'rims1_request', $1, now())",
        vec![Uuid::now_v7().into(), Uuid::from(local.id).into()],
    )
    .await;
    SeaOrmRelayRepository::new(db.clone())
        .store_identity("rimp1_cached", "rims1_cache", "{}")
        .await
        .unwrap();

    let forgotten = repo
        .forget_idle(Utc::now() - Duration::days(90), &[])
        .await
        .unwrap();
    assert_eq!(forgotten, vec!["rims1_free"]);
    for id in refs {
        assert!(repo.find(id).await.unwrap().is_some(), "{id} kept");
    }
}

#[tokio::test]
async fn a_new_pin_clears_the_forgotten_head_and_a_reset_clears_either() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let peer = "rims1_back";
    let origin = "https://back.example";
    repo.insert(pin(peer, origin, 1), PeerStatus::Active)
        .await
        .unwrap();
    age_pin(&ctx.db, peer, 100).await;
    repo.forget_idle(Utc::now() - Duration::days(90), &[])
        .await
        .unwrap();
    assert!(repo.forgotten(peer).await.unwrap().is_some());

    let back = repo
        .insert(pin(peer, origin, 2), PeerStatus::Forked)
        .await
        .unwrap();
    assert_eq!(back.status, PeerStatus::Forked);
    assert!(repo.forgotten(peer).await.unwrap().is_none());

    // A reset forgets a pin, and on its own a forgotten head.
    assert!(repo.delete(peer).await.unwrap());
    repo.insert(pin(peer, origin, 0), PeerStatus::Active)
        .await
        .unwrap();
    age_pin(&ctx.db, peer, 100).await;
    repo.forget_idle(Utc::now() - Duration::days(90), &[])
        .await
        .unwrap();
    assert!(repo.delete(peer).await.unwrap());
    assert!(repo.forgotten(peer).await.unwrap().is_none());
    assert!(!repo.delete(peer).await.unwrap());
}

#[tokio::test]
async fn an_origin_holds_at_most_its_limit_of_pins() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmPeerServerRepository::new(ctx.db.clone());
    let origin = "https://crowded.example";
    for i in 0..MAX_PEERS_PER_ORIGIN {
        repo.insert(pin(&format!("rims1_c{i}"), origin, 0), PeerStatus::Active)
            .await
            .unwrap();
    }
    let err = repo
        .insert(pin("rims1_extra", origin, 0), PeerStatus::Active)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::PreconditionFailed);
    assert!(repo.find("rims1_extra").await.unwrap().is_none());
    // A server pinned there already is a conflict, as anywhere.
    let err = repo
        .insert(pin("rims1_c0", origin, 0), PeerStatus::Active)
        .await
        .unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
    // Another origin is not affected.
    repo.insert(
        pin("rims1_extra", "https://quiet.example", 0),
        PeerStatus::Active,
    )
    .await
    .unwrap();
    assert_eq!(
        count(
            &ctx.db,
            "SELECT count(*) AS n FROM peer_server WHERE origin = 'https://crowded.example'"
        )
        .await,
        i64::try_from(MAX_PEERS_PER_ORIGIN).unwrap()
    );
}

#[tokio::test]
async fn stale_cached_identities_are_purged_unless_held_as_remote_users() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    for principal in ["rimp1_stale", "rimp1_member", "rimp1_fresh"] {
        repo.store_identity(principal, "rims1_relay", "{}")
            .await
            .unwrap();
    }
    execute(
        &ctx.db,
        "UPDATE remote_identity_cache SET fetched_at = now() - interval '31 days' \
         WHERE principal_id IN ('rimp1_stale', 'rimp1_member')",
        vec![],
    )
    .await;
    let member = ctx.create_user("cache_member").await;
    make_remote(&ctx.db, member.id, "rimp1_member", Some("rims1_relay")).await;

    let purged = repo
        .purge_identities(Utc::now() - Duration::days(30))
        .await
        .unwrap();
    assert_eq!(purged, 1);
    assert!(repo.cached_identity("rimp1_stale").await.unwrap().is_none());
    assert!(
        repo.cached_identity("rimp1_member")
            .await
            .unwrap()
            .is_some()
    );
    assert!(repo.cached_identity("rimp1_fresh").await.unwrap().is_some());
    assert_eq!(
        repo.purge_identities(Utc::now() - Duration::days(30))
            .await
            .unwrap(),
        0
    );
}
