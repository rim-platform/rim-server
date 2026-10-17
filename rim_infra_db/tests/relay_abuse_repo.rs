#![cfg(feature = "db-tests")]

//! Principal blocks and abuse reports against a real database: blocks replace their reason,
//! a block ignores pending requests for every recipient, reports resolve once and outlive
//! their reporter and request, and the migration refuses `down` while blocks or reports exist.

mod support;

use chrono::{Duration, Utc};
use migration::{Migrator, MigratorTrait};
use rim_core::relay::model::{
    AbuseReportId, DmRequestStatus, NewAbuseReport, NewDmRequest, NewRelayEnvelope, NewRelayItem,
};
use rim_core::relay::repository::{RelayAbuseRepository, RelayRepository};
use rim_core::user::model::UserId;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository;
use rim_infra_db::sea_orm_impl::relay_abuse::SeaOrmRelayAbuseRepository;
use sea_orm::{ConnectionTrait, DbBackend, Statement};
use uuid::Uuid;

const MIGRATION_NAME: &str = "m20261010_010907_add_relay_abuse";
const HOST: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
const SPAMMER: &str = "rimp1_spammer";

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

/// A pending request from `from` to `user` holding `held` items.
async fn pending_request(
    relay: &SeaOrmRelayRepository,
    user_id: UserId,
    from: &str,
    held: usize,
) -> rim_core::relay::model::DmRequest {
    let conversation_id = Uuid::now_v7();
    let request = relay
        .insert_request(NewDmRequest {
            status: DmRequestStatus::Pending,
            user_id,
            from_principal_id: from.to_string(),
            card_payload: "{}".to_string(),
            card_signature: "ed25519:c".to_string(),
            host_server_id: HOST.to_string(),
            conversation_id,
            expires_at: Utc::now() + Duration::days(30),
        })
        .await
        .unwrap();
    for n in 0..held {
        let envelope = relay
            .insert_envelope(NewRelayEnvelope {
                host_server_id: HOST.to_string(),
                workspace_id: Uuid::now_v7(),
                stream: format!("c:{conversation_id}"),
                seq: Some(n as i64 + 1),
                payload: format!("{{\"n\":{n}}}"),
                signature: "ed25519:s".to_string(),
            })
            .await
            .unwrap();
        relay
            .insert_item(NewRelayItem {
                user_id,
                envelope_id: envelope,
                device_key_id: None,
                request_id: Some(request.id),
                size_bytes: 10,
                expires_at: Utc::now() + Duration::days(30),
            })
            .await
            .unwrap();
    }
    request
}

#[tokio::test]
async fn blocks_replace_their_reason_and_lift_once() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmRelayAbuseRepository::new(ctx.db.clone());
    assert!(!repo.is_principal_blocked(SPAMMER).await.unwrap());
    assert!(repo.block_principal(SPAMMER, "spam").await.unwrap());
    assert!(!repo.block_principal(SPAMMER, "spam wave").await.unwrap());
    assert!(
        repo.block_principal("rimp1_other", "phishing")
            .await
            .unwrap()
    );
    assert!(repo.is_principal_blocked(SPAMMER).await.unwrap());
    let blocks = repo.list_principal_blocks().await.unwrap();
    assert_eq!(blocks.len(), 2);
    let spammer = blocks
        .iter()
        .find(|b| b.principal_id == SPAMMER)
        .expect("listed");
    assert_eq!(spammer.reason, "spam wave");
    assert!(repo.unblock_principal(SPAMMER).await.unwrap());
    assert!(!repo.unblock_principal(SPAMMER).await.unwrap());
    assert!(!repo.is_principal_blocked(SPAMMER).await.unwrap());
    assert!(repo.unblock_principal("rimp1_other").await.unwrap());
}

#[tokio::test]
async fn a_block_ignores_pending_requests_for_every_recipient_and_keeps_their_invites() {
    let ctx = support::TestCtx::new().await;
    let bob = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let carol = ctx.create_key_only_user("carol", "rimp1_carol").await;
    let relay = SeaOrmRelayRepository::new(ctx.db.clone());
    let abuse = SeaOrmRelayAbuseRepository::new(ctx.db.clone());
    let to_bob = pending_request(&relay, bob.id, SPAMMER, 3).await;
    let to_carol = pending_request(&relay, carol.id, SPAMMER, 2).await;
    let from_friend = pending_request(&relay, bob.id, "rimp1_friend", 2).await;

    let ignored = abuse
        .ignore_requests_from(SPAMMER, Utc::now() + Duration::days(30))
        .await
        .unwrap();
    assert_eq!(ignored, 2);
    for (user, request) in [(bob.id, &to_bob), (carol.id, &to_carol)] {
        let stored = relay.get_request(user, request.id).await.unwrap().unwrap();
        assert_eq!(stored.status, DmRequestStatus::Ignored);
        assert_eq!(
            relay.count_held(request.id).await.unwrap(),
            1,
            "the invite stays"
        );
    }
    let friend = relay
        .get_request(bob.id, from_friend.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(friend.status, DmRequestStatus::Pending);
    assert_eq!(relay.count_held(from_friend.id).await.unwrap(), 2);
}

#[tokio::test]
async fn reports_resolve_once_and_outlive_their_reporter_and_request() {
    let ctx = support::TestCtx::new().await;
    let bob = ctx.create_key_only_user("bob", "rimp1_bob").await;
    let relay = SeaOrmRelayRepository::new(ctx.db.clone());
    let abuse = SeaOrmRelayAbuseRepository::new(ctx.db.clone());
    let request = pending_request(&relay, bob.id, SPAMMER, 1).await;
    let from_user = abuse
        .insert_report(NewAbuseReport {
            reporter_user_id: Some(bob.id),
            subject_principal_id: Some(SPAMMER.to_string()),
            subject_server_id: Some(HOST.to_string()),
            request_id: Some(request.id),
            reason: "unsolicited ads".to_string(),
        })
        .await
        .unwrap();
    assert_eq!(from_user.reporter_user_id, Some(bob.id));
    assert_eq!(from_user.request_id, Some(request.id));
    assert!(from_user.resolved_at.is_none());
    let from_operator = abuse
        .insert_report(NewAbuseReport {
            reporter_user_id: None,
            subject_principal_id: None,
            subject_server_id: Some(HOST.to_string()),
            request_id: None,
            reason: "mail to the abuse address".to_string(),
        })
        .await
        .unwrap();

    let all = abuse.list_reports(false).await.unwrap();
    assert_eq!(
        all.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![from_user.id, from_operator.id]
    );
    let resolved = abuse
        .resolve_report(from_operator.id)
        .await
        .unwrap()
        .expect("resolved");
    let at = resolved.resolved_at.expect("resolved_at");
    let again = abuse
        .resolve_report(from_operator.id)
        .await
        .unwrap()
        .expect("still there");
    assert_eq!(again.resolved_at, Some(at), "the first resolution stays");
    assert!(
        abuse
            .resolve_report(AbuseReportId::new())
            .await
            .unwrap()
            .is_none()
    );
    let open = abuse.list_reports(true).await.unwrap();
    assert_eq!(
        open.iter().map(|r| r.id).collect::<Vec<_>>(),
        vec![from_user.id]
    );

    ctx.db
        .execute_unprepared(&format!(
            "DELETE FROM \"user\" WHERE id = '{}'",
            Uuid::from(bob.id)
        ))
        .await
        .unwrap();
    let kept = abuse.list_reports(false).await.unwrap();
    assert_eq!(kept.len(), 2);
    let report = kept.iter().find(|r| r.id == from_user.id).expect("kept");
    assert_eq!(report.reporter_user_id, None);
    assert_eq!(report.request_id, None, "the request went with its user");
    assert_eq!(report.subject_principal_id.as_deref(), Some(SPAMMER));
}

#[tokio::test]
async fn the_migration_refuses_down_while_blocks_or_reports_exist() {
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

    let abuse = SeaOrmRelayAbuseRepository::new(ctx.db.clone());
    abuse.block_principal(SPAMMER, "spam").await.unwrap();
    assert!(
        Migrator::down(&conn, Some(steps)).await.is_err(),
        "a block stops the down migration"
    );
    // Each migration commits on its own: the later ones rolled back before the refusal.
    Migrator::up(&conn, None).await.expect("up again");
    abuse.unblock_principal(SPAMMER).await.unwrap();
    let report = abuse
        .insert_report(NewAbuseReport {
            reporter_user_id: None,
            subject_principal_id: Some(SPAMMER.to_string()),
            subject_server_id: None,
            request_id: None,
            reason: "spam".to_string(),
        })
        .await
        .unwrap();
    assert!(
        Migrator::down(&conn, Some(steps)).await.is_err(),
        "a report stops the down migration"
    );
    Migrator::up(&conn, None).await.expect("up again");
    assert_eq!(
        count(
            &ctx.db,
            &format!(
                "SELECT count(*) AS n FROM relay_abuse_report WHERE id = '{}'",
                Uuid::from(report.id)
            )
        )
        .await,
        1
    );
    ctx.db
        .execute_unprepared("DELETE FROM relay_abuse_report")
        .await
        .unwrap();

    Migrator::down(&conn, Some(steps)).await.expect("down");
    for table in ["relay_principal_block", "relay_abuse_report"] {
        assert!(!table_exists(&conn, table).await, "{table} after down");
    }
    Migrator::up(&conn, None).await.expect("up");
    for table in ["relay_principal_block", "relay_abuse_report"] {
        assert!(table_exists(&conn, table).await, "{table} after up");
    }
}
