#![cfg(feature = "db-tests")]

//! The IP deny list against a real database: a block on the same range replaces its reason and
//! expiry, expired blocks drop out of the active list and are purged, and the migration refuses
//! `down` while blocks exist.

mod support;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::ip_block::model::NewIpBlock;
use rim_core::ip_block::repository::IpBlockRepository;
use rim_infra_db::sea_orm_impl::ip_block::SeaOrmIpBlockRepository;

const MIGRATION_NAME: &str = "m20261012_142739_add_ip_block";

fn block(cidr: &str, reason: &str, expires_at: Option<chrono::DateTime<Utc>>) -> NewIpBlock {
    NewIpBlock {
        cidr: cidr.to_string(),
        reason: reason.to_string(),
        expires_at,
    }
}

#[tokio::test]
async fn a_block_on_the_same_range_replaces_its_reason_and_expiry() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIpBlockRepository::new(ctx.db.clone());
    let expiry = Utc::now() + Duration::days(7);

    let (first, inserted) = repo
        .upsert(block("192.0.2.0/24", "spam", Some(expiry)))
        .await
        .unwrap();
    assert!(inserted);
    assert_eq!(first.reason, "spam");
    assert_eq!(
        first.expires_at.map(|t| t.timestamp_micros()),
        Some(expiry.timestamp_micros())
    );

    let (second, inserted) = repo
        .upsert(block("192.0.2.0/24", "scraping", None))
        .await
        .unwrap();
    assert!(!inserted);
    assert_eq!(second.id, first.id);
    assert_eq!(second.created_at, first.created_at);
    assert_eq!(second.reason, "scraping");
    assert_eq!(second.expires_at, None);
    assert_eq!(repo.list().await.unwrap(), vec![second]);

    assert!(repo.delete("192.0.2.0/24").await.unwrap());
    assert!(!repo.delete("192.0.2.0/24").await.unwrap());
    assert!(repo.list().await.unwrap().is_empty());
}

#[tokio::test]
async fn expired_blocks_leave_the_active_list_and_are_purged() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmIpBlockRepository::new(ctx.db.clone());
    let now = Utc::now();
    repo.upsert(block("198.51.100.0/24", "forever", None))
        .await
        .unwrap();
    repo.upsert(block(
        "2001:db8::/32",
        "a week",
        Some(now + Duration::days(7)),
    ))
    .await
    .unwrap();
    repo.upsert(block(
        "203.0.113.9/32",
        "over",
        Some(now - Duration::hours(1)),
    ))
    .await
    .unwrap();

    let active: Vec<String> = repo
        .list_active(now)
        .await
        .unwrap()
        .into_iter()
        .map(|b| b.cidr)
        .collect();
    assert_eq!(active, ["198.51.100.0/24", "2001:db8::/32"]);
    assert_eq!(repo.list().await.unwrap().len(), 3);

    assert_eq!(repo.delete_expired(now).await.unwrap(), 1);
    assert_eq!(repo.delete_expired(now).await.unwrap(), 0);
    assert_eq!(repo.list().await.unwrap().len(), 2);
    assert_eq!(
        repo.delete_expired(now + Duration::days(8)).await.unwrap(),
        1
    );
}

#[tokio::test]
async fn the_migration_refuses_down_while_blocks_exist() {
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

    let repo = SeaOrmIpBlockRepository::new(ctx.db.clone());
    repo.upsert(block("192.0.2.1/32", "spam", None))
        .await
        .unwrap();
    assert!(
        Migrator::down(&conn, Some(steps)).await.is_err(),
        "a block stops the down migration"
    );
    // Each migration commits on its own: the later ones rolled back before the refusal.
    Migrator::up(&conn, None).await.expect("up again");
    assert_eq!(repo.list().await.unwrap().len(), 1, "the block survives");

    repo.delete("192.0.2.1/32").await.unwrap();
    Migrator::down(&conn, Some(steps)).await.expect("down");
    Migrator::up(&conn, None).await.expect("up");
    assert!(repo.list().await.unwrap().is_empty());
}
