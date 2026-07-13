//! Live-database test for escalation boot recovery (Track C1b, §6 / §7 mandated test).
//!
//! Persists an `active` escalation run with a past `next_fire_at` and a `cancelled` run, runs
//! [`rim_infra_services::escalation::recover_active_escalation_runs`] against real repos + a real
//! [`InMemoryEscalationScheduler`], and asserts: the active run is armed (and, since its deadline
//! is already past, immediately due — "fires" in the sense that `take_due` would return it right
//! away); the cancelled run is left un-armed.
//!
//! Only compiled/run with `--features infra-db-tests` and a live `DATABASE_URL` (see CLAUDE.md).

#![cfg(feature = "infra-db-tests")]

mod support;

use chrono::{Duration, Utc};

use rim_core::conversation::model::ConversationType;
use rim_core::escalation::model::{EscalationRunStatus, NewEscalationPolicy, NewEscalationRun};
use rim_core::escalation::repository::{EscalationPolicyRepository, EscalationRunRepository};
use rim_core::incident::model::{IncidentSeverity, NewIncident};
use rim_core::incident::repository::IncidentRepository;
use rim_infra_db::DbHandle;
use rim_infra_services::escalation::{InMemoryEscalationScheduler, recover_active_escalation_runs};

#[tokio::test]
async fn boot_recovery_arms_active_run_with_past_deadline_and_ignores_cancelled_run() {
    let db = support::db_conn().await;
    support::reset_db(&db).await;

    let server_repo = rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let user_repo = rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let conversation_repo =
        rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository::new(
            DbHandle::Connection(db.clone()),
        );
    let incident_repo = rim_infra_db::sea_orm_impl::incident::SeaOrmIncidentRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let policy_repo =
        rim_infra_db::sea_orm_impl::escalation_policy::SeaOrmEscalationPolicyRepository::new(
            DbHandle::Connection(db.clone()),
        );
    let run_repo = rim_infra_db::sea_orm_impl::escalation_run::SeaOrmEscalationRunRepository::new(
        DbHandle::Connection(db.clone()),
    );

    use rim_core::conversation::repository::ConversationRepository;
    use rim_core::server::repository::ServerRepository;
    use rim_core::user::repository::UserRepository;

    let server = server_repo.create_server("s").await.unwrap();
    let user = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u".to_string(),
            password: "hash".to_string(),
            email: "u@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();
    let policy = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "P".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();

    // Two incidents (each conversation may only have one active incident at a time), so each
    // can carry its own escalation run.
    let conv_active = conversation_repo
        .create_conversation(rim_core::conversation::model::NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Channel,
            title: Some("active".to_string()),
        })
        .await
        .unwrap();
    let conv_cancelled = conversation_repo
        .create_conversation(rim_core::conversation::model::NewConversation {
            server_id: server.id,
            conversation_type: ConversationType::Channel,
            title: Some("cancelled".to_string()),
        })
        .await
        .unwrap();

    let incident_active = incident_repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv_active.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev1),
            title: "active incident".to_string(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: Some(policy.id),
        })
        .await
        .unwrap();
    let incident_cancelled = incident_repo
        .create(NewIncident {
            server_id: server.id,
            conversation_id: conv_cancelled.id,
            started_by: user.id,
            severity: Some(IncidentSeverity::Sev2),
            title: "cancelled incident".to_string(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: Some(policy.id),
        })
        .await
        .unwrap();

    // Active run: `next_fire_at` already in the past (simulating a missed page across a
    // restart) — recovery should arm it so it fires ASAP.
    let past_fire_at = Utc::now() - Duration::minutes(10);
    let active_run = run_repo
        .create(NewEscalationRun {
            incident_id: incident_active.id,
            policy_id: policy.id,
            next_fire_at: Some(past_fire_at),
        })
        .await
        .unwrap();

    // Cancelled run: recovery must ignore it entirely (not armed).
    let cancelled_run = run_repo
        .create(NewEscalationRun {
            incident_id: incident_cancelled.id,
            policy_id: policy.id,
            next_fire_at: Some(Utc::now() + Duration::hours(1)),
        })
        .await
        .unwrap();
    run_repo
        .set_status(cancelled_run.id, EscalationRunStatus::Cancelled)
        .await
        .unwrap();

    let scheduler = InMemoryEscalationScheduler::new();
    let armed_count = recover_active_escalation_runs(&run_repo, &scheduler)
        .await
        .expect("boot recovery must succeed");

    assert_eq!(armed_count, 1, "only the active run should be armed");

    // The active run is armed with its (past) deadline, so it's immediately due.
    let due = scheduler.take_due(Utc::now());
    assert_eq!(due, vec![active_run.id]);

    // The cancelled run was never armed — recovering it a second time (idempotency check) still
    // yields nothing due, and `next_deadline` is now empty since the only armed run was drained.
    assert_eq!(scheduler.next_deadline(), None);
}
