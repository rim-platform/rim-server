#![cfg(feature = "db-tests")]

//! Delegations, relay subscriptions and revocation fan-outs against a real database: one
//! delegation per workspace, device and relay with the longer one kept, what counts as active,
//! ending them, cascades, subscriptions and fan-outs, join requests a relay's users filed at
//! other hosts and the relay a host's join request names, and the migration's refusing `down`.

mod support;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::host::model::{DelegationStored, NewDelegation, NewRemoteUser};
use rim_core::host::repository::{DelegationRepository, HostRepository};
use rim_core::key_identity::model::{DeviceKey, NewDeviceKey};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::relay::model::{FanoutStatus, SubscriptionStatus};
use rim_core::relay::repository::{RelayRepository, SubscriptionRepository};
use rim_core::server::model::ServerId;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::UserId;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use rim_infra_db::sea_orm_impl::remote_workspace::{
    SeaOrmDelegationRepository, SeaOrmSubscriptionRepository,
};
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261010_010132_add_remote_workspaces";
const RELAY: &str = "rims1_T5LLFxkrobgyvlf3WVxuvLlfqYSZAssRFgmvCfI2R-c";
const OTHER_RELAY: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

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

async fn remote_user(db: &DbHandle, name: &str) -> UserId {
    SeaOrmHostRepository::new(db.clone())
        .create_remote_user(NewRemoteUser {
            principal_id: format!("rimp1_{name}"),
            identity_key: format!("ed25519:{name}"),
            username: format!("r_{name}"),
            display_name: name.to_string(),
            home_relay: None,
        })
        .await
        .unwrap()
}

async fn member(db: &DbHandle, server_id: ServerId, user_id: UserId) {
    SeaOrmServerMemberRepository::new(db.clone())
        .add_member(server_id, user_id, ServerMemberRole::Member)
        .await
        .unwrap();
}

fn delegation(
    workspace_id: ServerId,
    user_id: UserId,
    device: &DeviceKey,
    relay: &str,
    hours: i64,
) -> NewDelegation {
    NewDelegation {
        workspace_id,
        user_id,
        device_key_id: device.id,
        relay_server_id: relay.to_string(),
        payload: format!("{{\"hours\":{hours}}}"),
        signature: "ed25519:d".to_string(),
        expires_at: Utc::now() + Duration::hours(hours),
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
async fn one_delegation_per_workspace_device_and_relay_and_the_longer_one_stays() {
    let ctx = support::TestCtx::new().await;
    let workspace = ctx.create_server("hosted").await.id;
    let user = remote_user(&ctx.db, "bob").await;
    member(&ctx.db, workspace, user).await;
    let laptop = device(&ctx.db, user, 1).await;
    let repo = SeaOrmDelegationRepository::new(ctx.db.clone());
    let now = Utc::now();

    let first = repo
        .store(delegation(workspace, user, &laptop, RELAY, 6), now)
        .await
        .unwrap();
    assert!(matches!(first, DelegationStored::Stored { .. }));
    assert!(
        repo.is_active(workspace, laptop.id, RELAY, now)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .is_active(workspace, laptop.id, OTHER_RELAY, now)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .is_active(workspace, laptop.id, RELAY, now + Duration::hours(7))
            .await
            .unwrap()
    );
    let longer = repo
        .store(delegation(workspace, user, &laptop, RELAY, 12), now)
        .await
        .unwrap();
    assert!(matches!(longer, DelegationStored::Stored { .. }));
    let shorter = repo
        .store(delegation(workspace, user, &laptop, RELAY, 3), now)
        .await
        .unwrap();
    assert!(
        matches!(shorter, DelegationStored::Kept { expires_at } if expires_at == longer.expires_at()),
        "a shorter delegation does not replace an active longer one"
    );
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM host_delegation").await,
        1
    );
    assert_eq!(
        count(
            &ctx.db,
            "SELECT count(*) AS n FROM host_delegation WHERE payload = '{\"hours\":12}'"
        )
        .await,
        1
    );

    // Another relay holds its own row; an ended row is replaced by any new one.
    repo.store(delegation(workspace, user, &laptop, OTHER_RELAY, 2), now)
        .await
        .unwrap();
    assert_eq!(repo.count_active_for_relay(RELAY, now).await.unwrap(), 1);
    assert_eq!(
        repo.count_active_for_relay(OTHER_RELAY, now).await.unwrap(),
        1
    );
    assert_eq!(
        repo.end_for_user(workspace, user, Some(RELAY))
            .await
            .unwrap(),
        1
    );
    assert_eq!(repo.count_active_for_relay(RELAY, now).await.unwrap(), 0);
    let renewed = repo
        .store(delegation(workspace, user, &laptop, RELAY, 3), now)
        .await
        .unwrap();
    assert!(matches!(renewed, DelegationStored::Stored { .. }));
    assert_eq!(repo.count_active_for_relay(RELAY, now).await.unwrap(), 1);
}

#[tokio::test]
async fn only_members_with_live_devices_and_unexpired_delegations_are_delegated() {
    let ctx = support::TestCtx::new().await;
    let workspace = ctx.create_server("hosted").await.id;
    let other = ctx.create_server("other").await.id;
    let bob = remote_user(&ctx.db, "bob").await;
    let carol = remote_user(&ctx.db, "carol").await;
    member(&ctx.db, workspace, bob).await;
    member(&ctx.db, other, bob).await;
    member(&ctx.db, workspace, carol).await;
    let bobs = device(&ctx.db, bob, 1).await;
    let carols = device(&ctx.db, carol, 2).await;
    let repo = SeaOrmDelegationRepository::new(ctx.db.clone());
    let now = Utc::now();
    repo.store(delegation(workspace, bob, &bobs, RELAY, 6), now)
        .await
        .unwrap();
    repo.store(delegation(other, bob, &bobs, RELAY, 6), now)
        .await
        .unwrap();
    repo.store(delegation(workspace, carol, &carols, OTHER_RELAY, 6), now)
        .await
        .unwrap();

    assert!(repo.device_delegated(bobs.id, RELAY, now).await.unwrap());
    assert!(
        !repo
            .device_delegated(bobs.id, OTHER_RELAY, now)
            .await
            .unwrap()
    );
    let members = repo.delegated_members(workspace, now).await.unwrap();
    assert_eq!(members.len(), 2);
    assert!(members.iter().all(|m| m.role == ServerMemberRole::Member));
    let relays = repo.delegated_relays(workspace, now).await.unwrap();
    assert_eq!(relays.len(), 2);
    assert!(relays.contains(&(bob, RELAY.to_string())));
    assert!(relays.contains(&(carol, OTHER_RELAY.to_string())));
    let mut workspaces = repo.delegated_workspaces(bob, RELAY, now).await.unwrap();
    workspaces.sort_by_key(|w| Uuid::from(*w));
    let mut both = vec![workspace, other];
    both.sort_by_key(|w| Uuid::from(*w));
    assert_eq!(workspaces, both);

    // Later than the expiry nothing is active.
    let later = now + Duration::hours(7);
    assert!(!repo.device_delegated(bobs.id, RELAY, later).await.unwrap());
    assert!(
        repo.delegated_members(workspace, later)
            .await
            .unwrap()
            .is_empty()
    );

    // A departed member is no longer delegated nor listed, but their relay still is until the
    // delegation ends.
    SeaOrmServerMemberRepository::new(ctx.db.clone())
        .remove_member(workspace, carol)
        .await
        .unwrap();
    assert_eq!(
        repo.delegated_members(workspace, now).await.unwrap().len(),
        1
    );
    assert!(
        repo.delegated_relays(workspace, now)
            .await
            .unwrap()
            .contains(&(carol, OTHER_RELAY.to_string()))
    );
    assert!(
        !repo
            .device_delegated(carols.id, OTHER_RELAY, now)
            .await
            .unwrap()
    );

    // A revoked device is not delegated; cleanup ends its delegations and the departed one's.
    SeaOrmKeyIdentityRepository::new(ctx.db.clone())
        .revoke_device(bobs.id, "{}", "ed25519:r")
        .await
        .unwrap();
    assert!(!repo.device_delegated(bobs.id, RELAY, now).await.unwrap());
    assert_eq!(repo.end_lapsed(now).await.unwrap(), 3);
    assert!(
        repo.delegated_relays(workspace, now)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(repo.end_lapsed(now).await.unwrap(), 0);
}

#[tokio::test]
async fn ending_by_device_and_expiry() {
    let ctx = support::TestCtx::new().await;
    let workspace = ctx.create_server("hosted").await.id;
    let bob = remote_user(&ctx.db, "bob").await;
    member(&ctx.db, workspace, bob).await;
    let laptop = device(&ctx.db, bob, 1).await;
    let phone = device(&ctx.db, bob, 2).await;
    let repo = SeaOrmDelegationRepository::new(ctx.db.clone());
    let now = Utc::now();
    repo.store(delegation(workspace, bob, &laptop, RELAY, 6), now)
        .await
        .unwrap();
    repo.store(delegation(workspace, bob, &phone, RELAY, 1), now)
        .await
        .unwrap();
    assert_eq!(repo.end_for_device(laptop.id).await.unwrap(), 1);
    assert_eq!(repo.end_for_device(laptop.id).await.unwrap(), 0);
    assert!(repo.device_delegated(phone.id, RELAY, now).await.unwrap());
    assert_eq!(repo.end_lapsed(now + Duration::hours(2)).await.unwrap(), 1);
    assert_eq!(repo.end_for_user(workspace, bob, None).await.unwrap(), 0);
}

#[tokio::test]
async fn deleting_users_devices_and_workspaces_cascades() {
    let ctx = support::TestCtx::new().await;
    let workspace = ctx.create_server("hosted").await.id;
    let gone = ctx.create_server("gone").await.id;
    let bob = remote_user(&ctx.db, "bob").await;
    let carol = remote_user(&ctx.db, "carol").await;
    let bobs = device(&ctx.db, bob, 1).await;
    let carols = device(&ctx.db, carol, 2).await;
    let repo = SeaOrmDelegationRepository::new(ctx.db.clone());
    let subscriptions = SeaOrmSubscriptionRepository::new(ctx.db.clone());
    let now = Utc::now();
    repo.store(delegation(workspace, bob, &bobs, RELAY, 6), now)
        .await
        .unwrap();
    repo.store(delegation(gone, carol, &carols, RELAY, 6), now)
        .await
        .unwrap();
    subscriptions
        .upsert_subscription(
            bob,
            RELAY,
            workspace.into(),
            SubscriptionStatus::Active,
            None,
        )
        .await
        .unwrap();
    subscriptions
        .insert_fanouts(bobs.id, &[RELAY.to_string()], now)
        .await
        .unwrap();

    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM server WHERE id = '{}'",
            Uuid::from(gone)
        ))
        .await
        .unwrap();
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM host_delegation").await,
        1
    );
    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM device_key WHERE id = '{}'",
            Uuid::from(bobs.id)
        ))
        .await
        .unwrap();
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM host_delegation").await,
        0
    );
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_revocation_fanout").await,
        0
    );
    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM \"user\" WHERE id = '{}'",
            Uuid::from(bob)
        ))
        .await
        .unwrap();
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_subscription").await,
        0
    );
}

#[tokio::test]
async fn a_subscription_stays_active_while_any_device_keeps_it() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("alice", "rimp1_alice").await.id;
    let workspace = Uuid::now_v7();
    let repo = SeaOrmSubscriptionRepository::new(ctx.db.clone());
    let now = Utc::now();
    let soon = now + Duration::hours(2);
    let later = now + Duration::hours(10);

    let first = repo
        .upsert_subscription(
            user,
            RELAY,
            workspace,
            SubscriptionStatus::Active,
            Some(later),
        )
        .await
        .unwrap();
    assert_eq!(first.status, SubscriptionStatus::Active);
    // Another device's shorter delegation does not shorten it, and a refusal does not end it.
    let kept = repo
        .upsert_subscription(
            user,
            RELAY,
            workspace,
            SubscriptionStatus::Active,
            Some(soon),
        )
        .await
        .unwrap();
    assert_eq!(
        kept.expires_at.map(|t| t.timestamp()),
        Some(later.timestamp())
    );
    let refused = repo
        .upsert_subscription(user, RELAY, workspace, SubscriptionStatus::Ended, None)
        .await
        .unwrap();
    assert_eq!(refused.status, SubscriptionStatus::Active);
    assert!(
        repo.is_subscribed(user, RELAY, workspace, now)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .is_subscribed(user, OTHER_RELAY, workspace, now)
            .await
            .unwrap()
    );
    assert_eq!(
        repo.subscribed_hosts(user, now).await.unwrap(),
        vec![RELAY.to_string()]
    );
    assert_eq!(
        SeaOrmRelayRepository::new(ctx.db.clone())
            .hosts_of_user(user)
            .await
            .unwrap(),
        vec![RELAY.to_string()],
        "hosts holding a subscription hear about identity changes"
    );

    // Past its expiry it lapses, and a new delegation brings it back.
    assert!(
        !repo
            .is_subscribed(user, RELAY, workspace, later + Duration::seconds(1))
            .await
            .unwrap()
    );
    assert_eq!(
        repo.lapse_expired(later + Duration::seconds(1))
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        repo.list_subscriptions(user).await.unwrap()[0].status,
        SubscriptionStatus::Lapsed
    );
    repo.upsert_subscription(
        user,
        RELAY,
        workspace,
        SubscriptionStatus::Active,
        Some(later),
    )
    .await
    .unwrap();
    assert_eq!(repo.end_subscriptions(user).await.unwrap(), 1);
    assert_eq!(repo.end_subscriptions(user).await.unwrap(), 0);
    assert!(repo.subscribed_hosts(user, now).await.unwrap().is_empty());
}

#[tokio::test]
async fn ending_one_subscription_leaves_the_others() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("alice", "rimp1_alice").await.id;
    let (first, second) = (Uuid::now_v7(), Uuid::now_v7());
    let repo = SeaOrmSubscriptionRepository::new(ctx.db.clone());
    let now = Utc::now();
    let later = now + Duration::hours(10);
    for workspace in [first, second] {
        repo.upsert_subscription(
            user,
            RELAY,
            workspace,
            SubscriptionStatus::Active,
            Some(later),
        )
        .await
        .unwrap();
    }

    assert!(repo.end_subscription(user, RELAY, first).await.unwrap());
    assert!(!repo.end_subscription(user, RELAY, first).await.unwrap());
    let statuses: Vec<_> = repo
        .list_subscriptions(user)
        .await
        .unwrap()
        .into_iter()
        .map(|s| (s.workspace_id, s.status))
        .collect();
    assert_eq!(
        statuses,
        vec![
            (first, SubscriptionStatus::Ended),
            (second, SubscriptionStatus::Active),
        ]
    );
    assert!(!repo.is_subscribed(user, RELAY, first, now).await.unwrap());
    assert!(repo.is_subscribed(user, RELAY, second, now).await.unwrap());

    let joined_again = repo
        .upsert_subscription(user, RELAY, first, SubscriptionStatus::Active, Some(later))
        .await
        .unwrap();
    assert_eq!(joined_again.status, SubscriptionStatus::Active);
}

#[tokio::test]
async fn a_join_request_filed_elsewhere_is_recorded_until_it_expires_or_is_forgotten() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("alice", "rimp1_alice").await.id;
    let workspace = Uuid::now_v7();
    let repo = SeaOrmSubscriptionRepository::new(ctx.db.clone());
    let now = Utc::now();
    let expires = now + Duration::days(30);

    assert!(
        !repo
            .has_join_request(user, RELAY, workspace, now)
            .await
            .unwrap()
    );
    repo.record_join_request(user, RELAY, workspace, Uuid::now_v7(), expires)
        .await
        .unwrap();
    // Asking again replaces the row.
    repo.record_join_request(user, RELAY, workspace, Uuid::now_v7(), expires)
        .await
        .unwrap();
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_join_request").await,
        1
    );
    assert!(
        repo.has_join_request(user, RELAY, workspace, now)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .has_join_request(user, OTHER_RELAY, workspace, now)
            .await
            .unwrap(),
        "a request is the host's it was filed at"
    );
    assert!(
        !repo
            .has_join_request(user, RELAY, workspace, expires)
            .await
            .unwrap(),
        "an expired request is no request"
    );
    assert!(
        repo.delete_join_request(user, RELAY, workspace)
            .await
            .unwrap()
    );
    assert!(
        !repo
            .delete_join_request(user, RELAY, workspace)
            .await
            .unwrap()
    );

    repo.record_join_request(user, RELAY, workspace, Uuid::now_v7(), now)
        .await
        .unwrap();
    repo.record_join_request(user, OTHER_RELAY, workspace, Uuid::now_v7(), expires)
        .await
        .unwrap();
    assert_eq!(repo.delete_expired_join_requests(now).await.unwrap(), 1);
    assert_eq!(repo.delete_join_requests(user).await.unwrap(), 1);
    assert_eq!(
        count(&ctx.db, "SELECT count(*) AS n FROM relay_join_request").await,
        0
    );
}

#[tokio::test]
async fn a_hosts_join_request_names_the_relay_that_filed_it() {
    use rim_core::public_join_link::model::NewJoinRequest;
    use rim_core::public_join_link::repository::PublicJoinLinkRepository;
    use rim_infra_db::sea_orm_impl::public_join_link::SeaOrmPublicJoinLinkRepository;

    let ctx = support::TestCtx::new().await;
    let workspace = ctx.create_server("Hosted").await.id;
    let local = ctx.create_user("local").await.id;
    let remote = remote_user(&ctx.db, "rita").await;
    let links = SeaOrmPublicJoinLinkRepository::new(ctx.db.clone());
    let now = Utc::now();
    let file = |user_id: UserId, relay: Option<&str>| NewJoinRequest {
        server_id: workspace,
        user_id,
        message: None,
        created_at: now,
        expires_at: now + Duration::days(30),
        relay_server_id: relay.map(str::to_string),
    };
    let theirs = links
        .create_request(file(remote, Some(RELAY)))
        .await
        .unwrap();
    let ours = links.create_request(file(local, None)).await.unwrap();

    let host = SeaOrmHostRepository::new(ctx.db.clone());
    assert_eq!(
        host.join_request_relay(theirs.id).await.unwrap(),
        Some((RELAY.to_string(), "rimp1_rita".to_string()))
    );
    assert_eq!(host.join_request_relay(ours.id).await.unwrap(), None);
}

#[tokio::test]
async fn revocation_fan_outs_are_due_retried_and_acknowledged() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_key_only_user("alice", "rimp1_alice").await.id;
    let laptop = device(&ctx.db, user, 1).await;
    let repo = SeaOrmSubscriptionRepository::new(ctx.db.clone());
    let now = Utc::now();
    let hosts = vec![RELAY.to_string(), OTHER_RELAY.to_string()];
    assert_eq!(
        repo.insert_fanouts(laptop.id, &hosts, now).await.unwrap(),
        2
    );
    assert!(
        repo.due_fanouts(now, 10).await.unwrap().is_empty(),
        "nothing is due before the device is revoked"
    );
    SeaOrmKeyIdentityRepository::new(ctx.db.clone())
        .revoke_device(laptop.id, "{\"revoked\":1}", "ed25519:r")
        .await
        .unwrap();
    assert_eq!(
        repo.insert_fanouts(laptop.id, &hosts, now).await.unwrap(),
        0
    );
    let due = repo
        .due_fanouts(now + Duration::seconds(1), 10)
        .await
        .unwrap();
    assert_eq!(due.len(), 2);
    assert_eq!(due[0].principal_id, "rimp1_alice");
    assert_eq!(due[0].revocation_payload, "{\"revoked\":1}");

    repo.record_fanout_attempt(
        laptop.id,
        RELAY,
        1,
        FanoutStatus::Pending,
        now + Duration::seconds(10),
    )
    .await
    .unwrap();
    repo.record_fanout_ack(laptop.id, OTHER_RELAY, 1, "{\"ack\":1}", "ed25519:a")
        .await
        .unwrap();
    assert!(
        repo.due_fanouts(now + Duration::seconds(5), 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        repo.due_fanouts(now + Duration::seconds(11), 10)
            .await
            .unwrap()
            .len(),
        1
    );
    repo.record_fanout_attempt(laptop.id, RELAY, 2, FanoutStatus::GaveUp, now)
        .await
        .unwrap();
    let listed = repo.list_fanouts(laptop.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    let acked = listed
        .iter()
        .find(|f| f.host_server_id == OTHER_RELAY)
        .unwrap();
    assert_eq!(acked.status, FanoutStatus::Acked);
    assert_eq!(acked.ack_payload.as_deref(), Some("{\"ack\":1}"));
    let gave_up = listed.iter().find(|f| f.host_server_id == RELAY).unwrap();
    assert_eq!(
        (gave_up.status, gave_up.attempts),
        (FanoutStatus::GaveUp, 2)
    );
    assert!(
        repo.due_fanouts(now + Duration::days(2), 10)
            .await
            .unwrap()
            .is_empty()
    );
}

#[tokio::test]
async fn the_migration_refuses_down_while_delegations_or_subscriptions_exist() {
    let ctx = support::TestCtx::new().await;
    let conn = support::db_conn().await;
    // Steps from the newest applied migration down to and including this one, read from what is
    // applied right now: a refused `down` leaves the newer migrations rolled back.
    async fn steps_to_here(conn: &sea_orm::DatabaseConnection) -> u32 {
        let applied: Vec<String> = Migrator::get_applied_migrations(conn)
            .await
            .expect("applied migrations")
            .iter()
            .map(|m| m.name().to_string())
            .collect();
        let index = applied
            .iter()
            .position(|n| n == MIGRATION_NAME)
            .expect("migration must be applied");
        u32::try_from(applied.len() - index).expect("steps fit u32")
    }

    let user = ctx.create_key_only_user("alice", "rimp1_alice").await.id;
    SeaOrmSubscriptionRepository::new(ctx.db.clone())
        .upsert_subscription(
            user,
            RELAY,
            Uuid::now_v7(),
            SubscriptionStatus::Active,
            None,
        )
        .await
        .unwrap();
    let steps = steps_to_here(&conn).await;
    assert!(
        Migrator::down(&conn, Some(steps)).await.is_err(),
        "a subscription blocks the down migration"
    );
    // Each migration commits on its own: the later ones rolled back before the refusal.
    Migrator::up(&conn, None).await.expect("up again");
    assert!(table_exists(&conn, "host_delegation").await);
    ctx.db
        .execute_unprepared("DELETE FROM relay_subscription")
        .await
        .unwrap();

    let steps = steps_to_here(&conn).await;
    Migrator::down(&conn, Some(steps)).await.expect("down");
    for table in [
        "host_delegation",
        "relay_subscription",
        "relay_revocation_fanout",
    ] {
        assert!(!table_exists(&conn, table).await, "{table} after down");
    }
    Migrator::up(&conn, None).await.expect("up");
    for table in [
        "host_delegation",
        "relay_subscription",
        "relay_revocation_fanout",
    ] {
        assert!(table_exists(&conn, table).await, "{table} after up");
    }
    let err = conn
        .execute_unprepared(&format!(
            "INSERT INTO relay_subscription (id, user_id, host_server_id, workspace_id, status) \
             VALUES (gen_random_uuid(), '{}', 'x', gen_random_uuid(), 'other')",
            Uuid::from(user)
        ))
        .await;
    assert!(err.is_err(), "relay_subscription.status is checked");
}
