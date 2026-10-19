#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, Utc};
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

use rim_core::auth::model::NewSession;
use rim_core::auth::repository::SessionRepository;
use rim_core::key_identity::model::{DeviceKey, NewDeviceKey};
use rim_core::key_identity::repository::KeyIdentityRepository;
use rim_core::monitoring::MonitoringRepository;
use rim_core::relay::model::{DmRequestStatus, NewDmRequest, NewRelayEnvelope, NewRelayItem};
use rim_core::relay::repository::RelayRepository;
use rim_core::user::model::UserId;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository;
use rim_infra_db::sea_orm_impl::monitoring::SeaOrmMonitoringRepository;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;

async fn execute(db: &DbHandle, sql: &str, values: Vec<sea_orm::Value>) {
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        sql,
        values,
    ))
    .await
    .unwrap();
}

async fn device(db: &DbHandle, user_id: UserId, n: u8) -> DeviceKey {
    SeaOrmKeyIdentityRepository::new(db.clone())
        .add_device(NewDeviceKey {
            user_id,
            device_id: Uuid::from_u128(n.into()),
            public_key: format!("ed25519:{n}"),
            key_hash: vec![n; 32],
            name: format!("d{n}"),
            cert_payload: "{}".into(),
            cert_signature: "ed25519:x".into(),
            key_seq: 0,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn activity_survives_session_removal_and_is_throttled_and_local_only() {
    let ctx = support::TestCtx::new().await;
    let user = ctx.create_user("monitoring_active").await;
    let old = ctx.create_user("monitoring_weekly").await;
    let deleted = ctx.create_user("monitoring_deleted").await;
    let remote = ctx.create_user("monitoring_remote").await;
    execute(
        &ctx.db,
        "UPDATE \"user\" SET kind = 'remote' WHERE id = $1",
        vec![Uuid::from(remote.id).into()],
    )
    .await;
    execute(
        &ctx.db,
        "UPDATE \"user\" SET deleted_at = now() WHERE id = $1",
        vec![Uuid::from(deleted.id).into()],
    )
    .await;
    for (id, age) in [(old.id, 48), (deleted.id, 1), (remote.id, 1)] {
        execute(
            &ctx.db,
            "INSERT INTO user_activity VALUES ($1, now() - $2::int * interval '1 hour')",
            vec![Uuid::from(id).into(), age.into()],
        )
        .await;
    }
    let sessions = SeaOrmSessionRepository::new(ctx.db.clone());
    let session = sessions
        .create_session(NewSession {
            user_id: user.id,
            token_hash: "monitoring-token".into(),
            expires_at: Utc::now() + Duration::days(1),
            user_agent: None,
            ip: None,
        })
        .await
        .unwrap();
    sessions.touch_last_seen_at(session.id).await.unwrap();
    let first = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT last_authenticated_at FROM user_activity WHERE user_id = $1",
            [Uuid::from(user.id).into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<chrono::DateTime<chrono::FixedOffset>>("", "last_authenticated_at")
        .unwrap();
    sessions.touch_last_seen_at(session.id).await.unwrap();
    let second = ctx
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT last_authenticated_at FROM user_activity WHERE user_id = $1",
            [Uuid::from(user.id).into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get::<chrono::DateTime<chrono::FixedOffset>>("", "last_authenticated_at")
        .unwrap();
    assert_eq!(first, second);
    execute(
        &ctx.db,
        "DELETE FROM session WHERE user_id = $1",
        vec![Uuid::from(user.id).into()],
    )
    .await;
    let stats = SeaOrmMonitoringRepository::new(ctx.db.clone())
        .usage_stats()
        .await
        .unwrap();
    assert_eq!(
        (stats.registered_accounts, stats.active_24h, stats.active_7d),
        (2, 1, 2)
    );
}

#[tokio::test]
async fn pending_delivery_deduplicates_devices_and_excludes_held_expired_new_and_revoked() {
    let ctx = support::TestCtx::new().await;
    let user = ctx
        .create_key_only_user("delivery_user", "rimp1_delivery")
        .await;
    let d1 = device(&ctx.db, user.id, 1).await;
    let d2 = device(&ctx.db, user.id, 2).await;
    let repo = SeaOrmRelayRepository::new(ctx.db.clone());
    let monitor = SeaOrmMonitoringRepository::new(ctx.db.clone());
    let envelope = repo
        .insert_envelope(NewRelayEnvelope {
            host_server_id: "rims1_host".into(),
            workspace_id: Uuid::now_v7(),
            stream: "c:test".into(),
            seq: Some(1),
            payload: "{}".into(),
            signature: "ed25519:x".into(),
        })
        .await
        .unwrap();
    let base = NewRelayItem {
        user_id: user.id,
        envelope_id: envelope,
        device_key_id: None,
        request_id: None,
        size_bytes: 10,
        expires_at: Utc::now() + Duration::days(1),
    };
    let i1 = repo.insert_item(base.clone()).await.unwrap();
    let i2 = repo
        .insert_item(NewRelayItem {
            device_key_id: Some(d1.id),
            ..base.clone()
        })
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        expires_at: Utc::now() - Duration::seconds(1),
        ..base.clone()
    })
    .await
    .unwrap();
    let request = repo
        .insert_request(NewDmRequest {
            status: DmRequestStatus::Pending,
            user_id: user.id,
            from_principal_id: "rimp1_other".into(),
            card_payload: "{}".into(),
            card_signature: "ed25519:x".into(),
            host_server_id: "rims1_host".into(),
            conversation_id: Uuid::now_v7(),
            expires_at: Utc::now() + Duration::days(1),
        })
        .await
        .unwrap();
    repo.insert_item(NewRelayItem {
        request_id: Some(request.id),
        ..base
    })
    .await
    .unwrap();
    let d3 = device(&ctx.db, user.id, 3).await;
    let stats = monitor.delivery_stats().await.unwrap();
    assert_eq!(
        (
            stats.items,
            stats.bytes,
            stats.device_items,
            stats.users,
            stats.devices
        ),
        (2, 20, 3, 1, 2)
    );
    assert!(stats.oldest_age_seconds >= 0.0);
    execute(
        &ctx.db,
        "UPDATE \"user\" SET deleted_at = now() WHERE id = $1",
        vec![Uuid::from(user.id).into()],
    )
    .await;
    assert_eq!(monitor.delivery_stats().await.unwrap().items, 0);
    execute(
        &ctx.db,
        "UPDATE \"user\" SET deleted_at = NULL WHERE id = $1",
        vec![Uuid::from(user.id).into()],
    )
    .await;
    repo.ack(d1.id, i2).await.unwrap();
    repo.ack(d1.id, i1).await.unwrap();
    let stats = monitor.delivery_stats().await.unwrap();
    assert_eq!((stats.items, stats.device_items, stats.devices), (1, 1, 1));
    execute(
        &ctx.db,
        "UPDATE device_key SET revoked_at = now() WHERE id = $1",
        vec![Uuid::from(d2.id).into()],
    )
    .await;
    let stats = monitor.delivery_stats().await.unwrap();
    assert_eq!(
        (
            stats.items,
            stats.device_items,
            stats.devices,
            stats.oldest_age_seconds
        ),
        (0, 0, 0, 0.0)
    );
    assert!(repo.device_cursor(d3.id).await.unwrap() >= i2);
}
