#![cfg(feature = "db-tests")]

mod support;

use chrono::{Duration, SubsecRound, Utc};

use rim_core::conversation::model::ConversationType;
use rim_core::escalation::model::{
    EscalationAction, EscalationRunStatus, EscalationTarget, NewEscalationPolicy, NewEscalationRun,
    NewEscalationStep,
};
use rim_core::escalation::repository::{
    ConversationEscalationPolicyRepository, EscalationPolicyRepository, EscalationRunRepository,
    EscalationStepRepository,
};
use rim_core::incident::model::{IncidentSeverity, NewIncident};
use rim_core::incident::repository::IncidentRepository;
use rim_core::oncall::model::NewOnCallSchedule;
use rim_core::oncall::repository::OnCallScheduleRepository;
use rim_infra_db::sea_orm_impl::conversation_escalation_policy::SeaOrmConversationEscalationPolicyRepository;
use rim_infra_db::sea_orm_impl::escalation_policy::SeaOrmEscalationPolicyRepository;
use rim_infra_db::sea_orm_impl::escalation_run::SeaOrmEscalationRunRepository;
use rim_infra_db::sea_orm_impl::escalation_step::SeaOrmEscalationStepRepository;
use rim_infra_db::sea_orm_impl::incident::SeaOrmIncidentRepository;
use rim_infra_db::sea_orm_impl::on_call_schedule::SeaOrmOnCallScheduleRepository;

#[tokio::test]
async fn policy_create_find_list_update_delete() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;

    let created = repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "Primary Escalation".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();
    assert_eq!(created.name, "Primary Escalation");

    let found = repo.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(found.id, created.id);

    let listed = repo.list_by_server(server.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.id);

    let updated = repo
        .update_name(created.id, "Renamed".to_string())
        .await
        .unwrap();
    assert_eq!(updated.name, "Renamed");
    assert!(updated.updated_at >= created.updated_at);

    repo.delete(created.id).await.unwrap();
    assert!(repo.find_by_id(created.id).await.unwrap().is_none());
}

#[tokio::test]
async fn policy_delete_missing_returns_not_found() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let err = repo
        .delete(rim_core::escalation::model::EscalationPolicyId::new())
        .await
        .unwrap_err();
    assert_eq!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::NotFound
    );
}

/// Round-trips both `EscalationTarget` variants (User + OnCall) and multi-action steps through
/// the `target_kind`/`target_user_id`/`target_schedule_id`/`actions` columns, verifying the
/// encode/decode + CHECK-constraint-matching write path doesn't silently corrupt the target.
#[tokio::test]
async fn step_set_steps_round_trips_targets_and_actions_ordered_by_position() {
    let ctx = support::TestCtx::new().await;
    let policy_repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let step_repo = SeaOrmEscalationStepRepository::new(ctx.db.clone());
    let schedule_repo = SeaOrmOnCallScheduleRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let target_user = ctx.create_user("target").await;

    let schedule = schedule_repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: user.id,
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

    let steps = step_repo
        .set_steps(
            policy.id,
            vec![
                NewEscalationStep {
                    position: 0,
                    delay_seconds: 0,
                    target: EscalationTarget::User(target_user.id),
                    actions: vec![EscalationAction::MentionInChannel],
                },
                NewEscalationStep {
                    position: 1,
                    delay_seconds: 300,
                    target: EscalationTarget::OnCall(schedule.id),
                    actions: vec![EscalationAction::MentionInChannel, EscalationAction::Email],
                },
            ],
        )
        .await
        .unwrap();

    assert_eq!(steps.len(), 2);
    assert_eq!(steps[0].position, 0);
    assert_eq!(steps[0].target, EscalationTarget::User(target_user.id));
    assert_eq!(steps[0].actions, vec![EscalationAction::MentionInChannel]);
    assert_eq!(steps[1].position, 1);
    assert_eq!(steps[1].target, EscalationTarget::OnCall(schedule.id));
    assert_eq!(
        steps[1].actions,
        vec![EscalationAction::MentionInChannel, EscalationAction::Email]
    );

    let listed = step_repo.list_by_policy(policy.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].position, 0);
    assert_eq!(listed[1].position, 1);
}

/// `set_steps` fully replaces the prior step list (delete-then-insert), mirroring
/// `RotationParticipantRepository::set_participants`.
#[tokio::test]
async fn step_set_steps_replaces_prior_steps_atomically() {
    let ctx = support::TestCtx::new().await;
    let policy_repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let step_repo = SeaOrmEscalationStepRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let target_user = ctx.create_user("target").await;

    let policy = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "P".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();

    step_repo
        .set_steps(
            policy.id,
            vec![NewEscalationStep {
                position: 0,
                delay_seconds: 0,
                target: EscalationTarget::User(target_user.id),
                actions: vec![EscalationAction::Email],
            }],
        )
        .await
        .unwrap();

    let replaced = step_repo
        .set_steps(
            policy.id,
            vec![
                NewEscalationStep {
                    position: 0,
                    delay_seconds: 10,
                    target: EscalationTarget::User(target_user.id),
                    actions: vec![EscalationAction::MentionInChannel],
                },
                NewEscalationStep {
                    position: 1,
                    delay_seconds: 20,
                    target: EscalationTarget::User(target_user.id),
                    actions: vec![EscalationAction::Email],
                },
            ],
        )
        .await
        .unwrap();

    assert_eq!(replaced.len(), 2);
    let listed = step_repo.list_by_policy(policy.id).await.unwrap();
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[0].delay_seconds, 10);
}

#[tokio::test]
async fn conversation_escalation_policy_set_then_get_then_overwrite() {
    let ctx = support::TestCtx::new().await;
    let policy_repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let binding_repo = SeaOrmConversationEscalationPolicyRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let conversation = ctx
        .create_conversation(server.id, ConversationType::Channel)
        .await;

    let policy_a = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "A".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();
    let policy_b = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "B".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();

    assert!(binding_repo.get(conversation.id).await.unwrap().is_none());

    binding_repo
        .set(conversation.id, server.id, policy_a.id)
        .await
        .unwrap();
    assert_eq!(
        binding_repo.get(conversation.id).await.unwrap(),
        Some(policy_a.id)
    );

    // Setting again overwrites the previous binding rather than erroring (upsert semantics).
    binding_repo
        .set(conversation.id, server.id, policy_b.id)
        .await
        .unwrap();
    assert_eq!(
        binding_repo.get(conversation.id).await.unwrap(),
        Some(policy_b.id)
    );
}

async fn create_incident_for_run(
    ctx: &support::TestCtx,
    server_id: rim_core::server::model::ServerId,
    started_by: rim_core::user::model::UserId,
) -> rim_core::incident::model::Incident {
    let conversation = ctx
        .create_conversation(server_id, ConversationType::Channel)
        .await;
    let incident_repo = SeaOrmIncidentRepository::new(ctx.db.clone());
    incident_repo
        .create(NewIncident {
            server_id,
            conversation_id: conversation.id,
            started_by,
            severity: Some(IncidentSeverity::Sev2),
            title: "t".to_string(),
            original_topic: None,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
        })
        .await
        .unwrap()
}

#[tokio::test]
async fn run_create_find_advance_set_status_and_list_active() {
    let ctx = support::TestCtx::new().await;
    let policy_repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let run_repo = SeaOrmEscalationRunRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let policy = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "P".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();
    let incident = create_incident_for_run(&ctx, server.id, user.id).await;

    let now = Utc::now();
    let created = run_repo
        .create(NewEscalationRun {
            incident_id: incident.id,
            policy_id: policy.id,
            next_fire_at: Some(now),
        })
        .await
        .unwrap();
    assert_eq!(created.current_step_index, 0);
    assert_eq!(created.status, EscalationRunStatus::Active);

    let found = run_repo.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(found.id, created.id);

    let active = run_repo
        .find_active_by_incident(incident.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(active.id, created.id);

    let listed_active = run_repo.list_active().await.unwrap();
    assert!(listed_active.iter().any(|r| r.id == created.id));

    // Postgres `timestamptz` has microsecond precision, so round-tripping a `chrono::Utc::now()`
    // (nanosecond precision) loses sub-microsecond digits — truncate before comparing.
    let next_fire = (now + Duration::seconds(300)).trunc_subsecs(6);
    let advanced = run_repo
        .advance(created.id, 1, Some(next_fire))
        .await
        .unwrap();
    assert_eq!(advanced.current_step_index, 1);
    assert_eq!(advanced.next_fire_at, Some(next_fire));
    assert_eq!(advanced.status, EscalationRunStatus::Active);

    let completed = run_repo
        .set_status(created.id, EscalationRunStatus::Completed)
        .await
        .unwrap();
    assert_eq!(completed.status, EscalationRunStatus::Completed);
    // `set_status` clears `next_fire_at` when leaving `Active`.
    assert_eq!(completed.next_fire_at, None);

    assert!(
        run_repo
            .find_active_by_incident(incident.id)
            .await
            .unwrap()
            .is_none()
    );
    assert!(
        !run_repo
            .list_active()
            .await
            .unwrap()
            .iter()
            .any(|r| r.id == created.id)
    );
}

/// The partial unique index `idx_escalation_run_active_incident` enforces at most one `active`
/// run per incident — a second `create` for the same incident while the first is still active
/// must fail (DB-level backstop for the "one active run per incident" invariant, §6).
#[tokio::test]
async fn run_create_second_active_run_for_same_incident_conflicts() {
    let ctx = support::TestCtx::new().await;
    let policy_repo = SeaOrmEscalationPolicyRepository::new(ctx.db.clone());
    let run_repo = SeaOrmEscalationRunRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let policy = policy_repo
        .create(NewEscalationPolicy {
            server_id: server.id,
            name: "P".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();
    let incident = create_incident_for_run(&ctx, server.id, user.id).await;

    run_repo
        .create(NewEscalationRun {
            incident_id: incident.id,
            policy_id: policy.id,
            next_fire_at: Some(Utc::now()),
        })
        .await
        .unwrap();

    let err = run_repo
        .create(NewEscalationRun {
            incident_id: incident.id,
            policy_id: policy.id,
            next_fire_at: Some(Utc::now()),
        })
        .await
        .unwrap_err();
    assert_eq!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::Conflict
    );
}
