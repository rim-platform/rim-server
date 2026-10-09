#![cfg(feature = "db-tests")]

//! `device_link` storage: conditional transitions, expiry by the database clock, cleanup, the
//! one-live-link index, and the `add_device_link` migration.

mod support;

use chrono::Duration;
use migration::{Migrator, MigratorTrait};
use rim_core::device_link::model::{DeviceLinkState, NewDeviceLink, SealedBytes};
use rim_core::device_link::repository::DeviceLinkRepository;
use rim_core::repository::error::RepoErrorKind;
use rim_core::user::model::UserId;
use rim_infra_db::sea_orm_impl::device_link::SeaOrmDeviceLinkRepository;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261009_112441_add_device_link";
const TTL: Duration = Duration::minutes(10);

fn hash(n: u8) -> Vec<u8> {
    vec![n; 32]
}

fn sealed(n: u8) -> SealedBytes {
    SealedBytes {
        nonce: vec![n; 12],
        ciphertext: vec![n; 40],
    }
}

fn new_link(user_id: UserId, n: u8) -> NewDeviceLink {
    NewDeviceLink {
        link_hash: hash(n),
        user_id,
        ttl: TTL,
    }
}

async fn exec(conn: &DatabaseConnection, sql: &str, values: Vec<sea_orm::Value>) -> u64 {
    conn.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap_or_else(|e| panic!("{sql}: {e}"))
    .rows_affected()
}

/// Move a link's expiry `seconds` from now (negative: into the past).
async fn expire_in(conn: &DatabaseConnection, link_hash: &[u8], seconds: i64) {
    exec(
        conn,
        "UPDATE device_link SET expires_at = now() + make_interval(secs => $2) \
         WHERE link_hash = $1",
        vec![link_hash.to_vec().into(), (seconds as f64).into()],
    )
    .await;
}

/// The raw sealed columns and `ended_at` of a link, bypassing the repository's expiry filter.
async fn raw_columns(conn: &DatabaseConnection, link_hash: &[u8]) -> (bool, bool) {
    let row = conn
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT (offer_nonce IS NULL AND offer_ciphertext IS NULL \
                     AND answer_nonce IS NULL AND answer_ciphertext IS NULL) AS cleared, \
                    ended_at IS NOT NULL AS ended \
             FROM device_link WHERE link_hash = $1",
            [link_hash.to_vec().into()],
        ))
        .await
        .expect("select")
        .expect("row");
    (
        row.try_get("", "cleared").expect("cleared"),
        row.try_get("", "ended").expect("ended"),
    )
}

async fn table_exists(conn: &DatabaseConnection) -> bool {
    conn.query_one_raw(Statement::from_string(
        DbBackend::Postgres,
        "SELECT to_regclass('device_link') IS NOT NULL AS present",
    ))
    .await
    .expect("select")
    .expect("row")
    .try_get("", "present")
    .expect("present")
}

#[tokio::test]
async fn a_link_moves_through_its_states_and_ends_without_sealed_bytes() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_user("bob").await;

    assert!(repo.lock_user(alice.id).await.unwrap());
    assert!(!repo.lock_user(UserId::new()).await.unwrap());

    let created = repo.create(new_link(alice.id, 1)).await.unwrap();
    assert_eq!(created.state, DeviceLinkState::Open);
    assert_eq!(created.user_id, alice.id);
    assert!(!created.expired);
    let ttl = created.expires_at - created.created_at;
    assert!(ttl > Duration::minutes(9) && ttl <= TTL, "{ttl}");
    assert_eq!(repo.find(&hash(1)).await.unwrap(), Some(created.clone()));
    assert_eq!(repo.find(&hash(9)).await.unwrap(), None);

    // Session transitions only match the link's user.
    assert_eq!(repo.complete(&hash(1), alice.id).await.unwrap(), None);
    let offered = repo
        .offer(&hash(1), sealed(7))
        .await
        .unwrap()
        .expect("offered");
    assert_eq!(offered.state, DeviceLinkState::Offered);
    assert_eq!(offered.offer, Some(sealed(7)));
    assert!(offered.offered_at.is_some());
    // One offer per link.
    assert_eq!(repo.offer(&hash(1), sealed(7)).await.unwrap(), None);
    assert_eq!(
        repo.answer(&hash(1), bob.id, sealed(8), TTL).await.unwrap(),
        None
    );

    // Pull the expiry close, then check that answering resets it to now + TTL.
    expire_in(&conn, &hash(1), 30).await;
    let answered = repo
        .answer(&hash(1), alice.id, sealed(8), TTL)
        .await
        .unwrap()
        .expect("answered");
    assert_eq!(answered.state, DeviceLinkState::Answered);
    assert_eq!(answered.offer, Some(sealed(7)));
    assert_eq!(answered.answer, Some(sealed(8)));
    let answered_at = answered.answered_at.expect("answered_at");
    assert!(answered.expires_at - answered_at > Duration::minutes(9));

    assert_eq!(repo.complete(&hash(1), bob.id).await.unwrap(), None);
    let consumed = repo
        .complete(&hash(1), alice.id)
        .await
        .unwrap()
        .expect("consumed");
    assert_eq!(consumed.state, DeviceLinkState::Consumed);
    assert_eq!((consumed.offer, consumed.answer), (None, None));
    assert!(consumed.ended_at.is_some());
    assert_eq!(raw_columns(&conn, &hash(1)).await, (true, true));
    // A consumed link cannot be cancelled.
    assert_eq!(repo.cancel(&hash(1), None).await.unwrap(), None);

    // Cancelling an offered link clears its offer too.
    repo.create(new_link(bob.id, 2)).await.unwrap();
    repo.offer(&hash(2), sealed(3))
        .await
        .unwrap()
        .expect("offered");
    assert_eq!(repo.cancel(&hash(2), Some(alice.id)).await.unwrap(), None);
    let cancelled = repo
        .cancel(&hash(2), Some(bob.id))
        .await
        .unwrap()
        .expect("cancelled");
    assert_eq!(cancelled.state, DeviceLinkState::Cancelled);
    assert_eq!(cancelled.offer, None);
    assert_eq!(raw_columns(&conn, &hash(2)).await, (true, true));
    assert_eq!(repo.cancel(&hash(2), None).await.unwrap(), None);

    // Anyone holding the id may cancel.
    repo.create(new_link(alice.id, 3)).await.unwrap();
    let cancelled = repo
        .cancel(&hash(3), None)
        .await
        .unwrap()
        .expect("cancelled");
    assert_eq!(cancelled.state, DeviceLinkState::Cancelled);
}

#[tokio::test]
async fn two_concurrent_offers_on_one_link_have_exactly_one_winner() {
    let ctx = support::TestCtx::new().await;
    let alice = ctx.create_user("alice").await;
    let first = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let second = SeaOrmDeviceLinkRepository::new(support::db_conn().await.into());

    for n in 1..=20u8 {
        first.cancel_live_links(alice.id).await.unwrap();
        first.create(new_link(alice.id, n)).await.unwrap();
        let link_hash = hash(n);
        let (a, b) = tokio::join!(
            first.offer(&link_hash, sealed(100)),
            second.offer(&link_hash, sealed(200)),
        );
        let winners: Vec<_> = [a.unwrap(), b.unwrap()].into_iter().flatten().collect();
        assert_eq!(winners.len(), 1, "round {n}");
        let stored = first.find(&link_hash).await.unwrap().expect("link");
        assert_eq!(stored.offer, winners[0].offer);
    }
}

#[tokio::test]
async fn transitions_refuse_expired_links_and_reads_hide_their_bytes() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_user("bob").await;
    let carol = ctx.create_user("carol").await;

    // Open, expired: no offer.
    repo.create(new_link(alice.id, 1)).await.unwrap();
    expire_in(&conn, &hash(1), -1).await;
    assert_eq!(repo.offer(&hash(1), sealed(1)).await.unwrap(), None);
    assert_eq!(repo.cancel(&hash(1), None).await.unwrap(), None);
    let link = repo.find(&hash(1)).await.unwrap().expect("link");
    assert!(link.expired);
    assert_eq!(link.state, DeviceLinkState::Open);

    // Offered, expired: no answer, and the stored offer is not read back.
    repo.create(new_link(bob.id, 2)).await.unwrap();
    repo.offer(&hash(2), sealed(2))
        .await
        .unwrap()
        .expect("offered");
    expire_in(&conn, &hash(2), 0).await;
    assert_eq!(
        repo.answer(&hash(2), bob.id, sealed(3), TTL).await.unwrap(),
        None
    );
    let link = repo.find(&hash(2)).await.unwrap().expect("link");
    assert!(link.expired);
    assert_eq!(link.state, DeviceLinkState::Offered);
    assert_eq!(link.offer, None);
    assert_eq!(raw_columns(&conn, &hash(2)).await, (false, false));

    // Answered, expired: no completion, no cancel, no answer read back.
    repo.create(new_link(carol.id, 3)).await.unwrap();
    repo.offer(&hash(3), sealed(4))
        .await
        .unwrap()
        .expect("offered");
    repo.answer(&hash(3), carol.id, sealed(5), TTL)
        .await
        .unwrap()
        .expect("answered");
    expire_in(&conn, &hash(3), -60).await;
    assert_eq!(repo.complete(&hash(3), carol.id).await.unwrap(), None);
    assert_eq!(repo.cancel(&hash(3), Some(carol.id)).await.unwrap(), None);
    let link = repo.find(&hash(3)).await.unwrap().expect("link");
    assert!(link.expired);
    assert_eq!((link.offer, link.answer), (None, None));
}

#[tokio::test]
async fn cleanup_deletes_only_links_expired_more_than_the_grace_ago() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let users = [
        ctx.create_user("alice").await,
        ctx.create_user("bob").await,
        ctx.create_user("carol").await,
    ];
    for (n, user) in (1u8..).zip(&users) {
        repo.create(new_link(user.id, n)).await.unwrap();
    }
    expire_in(&conn, &hash(1), -2 * 3600).await;
    expire_in(&conn, &hash(2), -3600 + 60).await;

    assert_eq!(repo.delete_expired(Duration::hours(1)).await.unwrap(), 1);
    assert_eq!(repo.find(&hash(1)).await.unwrap(), None);
    assert!(repo.find(&hash(2)).await.unwrap().expect("kept").expired);
    assert!(!repo.find(&hash(3)).await.unwrap().expect("kept").expired);
    assert_eq!(repo.delete_expired(Duration::hours(1)).await.unwrap(), 0);
}

#[tokio::test]
async fn one_live_link_per_user() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    let bob = ctx.create_user("bob").await;

    let first = repo.create(new_link(alice.id, 1)).await.unwrap();
    let err = repo.create(new_link(alice.id, 2)).await.unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);
    // Another user is not blocked, and a link hash is unique.
    repo.create(new_link(bob.id, 2)).await.unwrap();
    repo.cancel_live_links(bob.id).await.unwrap();
    let err = repo.create(new_link(bob.id, 1)).await.unwrap_err();
    assert_eq!(err.kind(), RepoErrorKind::Conflict);

    // An expired link still holds the slot until it is cancelled with the rest.
    expire_in(&conn, &hash(1), -1).await;
    assert!(repo.create(new_link(alice.id, 3)).await.is_err());
    assert_eq!(
        repo.cancel_live_links(alice.id).await.unwrap(),
        vec![first.id]
    );
    let link = repo.find(&hash(1)).await.unwrap().expect("link");
    assert_eq!(link.state, DeviceLinkState::Cancelled);
    repo.create(new_link(alice.id, 3)).await.unwrap();

    // Ended links never block.
    repo.offer(&hash(3), sealed(1))
        .await
        .unwrap()
        .expect("offered");
    repo.answer(&hash(3), alice.id, sealed(2), TTL)
        .await
        .unwrap()
        .expect("answered");
    repo.complete(&hash(3), alice.id)
        .await
        .unwrap()
        .expect("consumed");
    assert!(repo.cancel_live_links(alice.id).await.unwrap().is_empty());
    repo.create(new_link(alice.id, 4)).await.unwrap();
}

#[tokio::test]
async fn deleting_the_user_deletes_their_links() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    let alice = ctx.create_user("alice").await;
    repo.create(new_link(alice.id, 1)).await.unwrap();

    let deleted = exec(
        &conn,
        r#"DELETE FROM "user" WHERE id = $1"#,
        vec![Uuid::from(alice.id).into()],
    )
    .await;
    assert_eq!(deleted, 1);
    assert_eq!(repo.find(&hash(1)).await.unwrap(), None);
}

#[tokio::test]
async fn the_table_checks_its_columns() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let alice = ctx.create_user("alice").await;
    let insert = |link_hash: Vec<u8>, state: &'static str, with_offer_nonce: bool| {
        let conn = conn.clone();
        async move {
            conn.execute_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "INSERT INTO device_link (id, link_hash, user_id, state, offer_nonce, expires_at) \
                 VALUES ($1, $2, $3, $4, $5, now())",
                [
                    Uuid::now_v7().into(),
                    link_hash.into(),
                    Uuid::from(alice.id).into(),
                    state.into(),
                    with_offer_nonce.then(|| vec![0u8; 12]).into(),
                ],
            ))
            .await
        }
    };
    assert!(insert(vec![1; 31], "cancelled", false).await.is_err());
    assert!(insert(vec![1; 32], "expired", false).await.is_err());
    assert!(insert(vec![1; 32], "cancelled", true).await.is_err());
    assert!(insert(vec![1; 32], "cancelled", false).await.is_ok());
}

/// Migration steps from the newest down to just before `add_device_link`, robust to later
/// migrations being appended after it.
fn steps_to_before_migration() -> u32 {
    let names: Vec<String> = Migrator::migrations()
        .iter()
        .map(|m| m.name().to_string())
        .collect();
    let index = names
        .iter()
        .position(|n| n == MIGRATION_NAME)
        .expect("migration must be registered");
    u32::try_from(names.len() - index).expect("step count fits u32")
}

#[tokio::test]
async fn migration_down_drops_the_table_and_up_restores_it() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    let alice = ctx.create_user("alice").await;
    SeaOrmDeviceLinkRepository::new(ctx.db.clone())
        .create(new_link(alice.id, 1))
        .await
        .unwrap();
    assert!(table_exists(&conn).await);

    Migrator::down(&conn, Some(steps_to_before_migration()))
        .await
        .expect("roll back add_device_link");
    assert!(!table_exists(&conn).await);

    Migrator::up(&conn, None).await.expect("migrate up again");
    assert!(table_exists(&conn).await);
    let repo = SeaOrmDeviceLinkRepository::new(ctx.db.clone());
    assert_eq!(repo.find(&hash(1)).await.unwrap(), None);
    repo.create(new_link(alice.id, 1)).await.unwrap();
}
