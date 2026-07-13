//! Live-database wiring-correctness test for on-call schedules (Track C1a).
//!
//! Persists a schedule + rotation layer + participants + override, then asserts that
//! [`DefaultOnCallService::who_is_on_call`] (going through real repos) resolves to the same
//! `current_user_id` as calling [`rim_core::oncall::resolve::resolve_on_call`] directly over the
//! equivalent in-memory [`rim_core::oncall::resolve::ScheduleSnapshot`] — i.e. that the
//! repo-to-domain-model wiring (mappers, JOINs the service does by hand) doesn't silently diverge
//! from the pure resolver it's supposed to be a thin wrapper around.
//!
//! Only compiled/run with `--features infra-db-tests` and a live `DATABASE_URL` (see CLAUDE.md).

#![cfg(feature = "infra-db-tests")]

mod support;

use std::str::FromStr;

use chrono::{Duration, Utc};

use rim_core::oncall::model::{
    NewOnCallOverride, NewOnCallSchedule, NewRotationLayer, RotationKind,
};
use rim_core::oncall::repository::{
    OnCallOverrideRepository, OnCallScheduleRepository, RotationLayerRepository,
    RotationParticipantRepository,
};
use rim_core::oncall::resolve::{ResolvedLayer, ScheduleSnapshot, resolve_on_call};
use rim_infra_db::DbHandle;
use rim_infra_services::AppRepos;
use rim_infra_services::oncall::DefaultOnCallService;
use rim_services::Actor;
use rim_services::oncall::command::WhoIsOnCallQuery;
use rim_services::oncall::service::OnCallService;

fn build_oncall_service(db: sea_orm::DatabaseConnection) -> DefaultOnCallService {
    let repos = AppRepos::with_handle(&DbHandle::Connection(db)).for_oncall();
    DefaultOnCallService::new(repos)
}

#[tokio::test]
async fn who_is_on_call_matches_pure_resolver() {
    let db = support::db_conn().await;
    support::reset_db(&db).await;

    let schedule_repo =
        rim_infra_db::sea_orm_impl::on_call_schedule::SeaOrmOnCallScheduleRepository::new(
            DbHandle::Connection(db.clone()),
        );
    let layer_repo =
        rim_infra_db::sea_orm_impl::on_call_rotation_layer::SeaOrmRotationLayerRepository::new(
            DbHandle::Connection(db.clone()),
        );
    let participant_repo = rim_infra_db::sea_orm_impl::on_call_rotation_participant::SeaOrmRotationParticipantRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let override_repo =
        rim_infra_db::sea_orm_impl::on_call_override::SeaOrmOnCallOverrideRepository::new(
            DbHandle::Connection(db.clone()),
        );
    let server_repo = rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let user_repo = rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository::new(
        DbHandle::Connection(db.clone()),
    );
    let server_member_repo =
        rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository::new(
            DbHandle::Connection(db.clone()),
        );

    use rim_core::server::repository::ServerRepository;
    use rim_core::server_member::repository::ServerMemberRepository;
    use rim_core::user::repository::UserRepository;

    let server = server_repo.create_server("s").await.unwrap();
    let owner = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "owner".to_string(),
            password: "hash".to_string(),
            email: "owner@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();
    server_member_repo
        .add_member(
            server.id,
            owner.id,
            rim_core::server_member::model::ServerMemberRole::Owner,
        )
        .await
        .unwrap();
    let u1 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u1".to_string(),
            password: "hash".to_string(),
            email: "u1@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();
    let u2 = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "u2".to_string(),
            password: "hash".to_string(),
            email: "u2@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();
    let override_user = user_repo
        .create_user(rim_core::user::model::NewUser {
            username: "override_user".to_string(),
            password: "hash".to_string(),
            email: "overrideuser@example.com".to_string(),
            display_name: None,
        })
        .await
        .unwrap();

    let anchor = Utc::now() - Duration::hours(1);
    let schedule = schedule_repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "America/Chicago".to_string(),
            created_by: owner.id,
        })
        .await
        .unwrap();
    let layer = layer_repo
        .create(NewRotationLayer {
            schedule_id: schedule.id,
            name: "Primary rotation".to_string(),
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: anchor,
        })
        .await
        .unwrap();
    participant_repo
        .set_participants(layer.id, &[u1.id, u2.id])
        .await
        .unwrap();

    let now = Utc::now();
    // An override that does NOT currently cover `now` — placed in the future so the layer
    // resolution is what's exercised by this assertion, while still proving the service loads
    // and passes overrides through to the resolver correctly (see the override-active case below).
    override_repo
        .create(NewOnCallOverride {
            schedule_id: schedule.id,
            user_id: override_user.id,
            start_at: now + Duration::days(1),
            end_at: now + Duration::days(2),
            created_by: owner.id,
        })
        .await
        .unwrap();

    let svc = build_oncall_service(db.clone());
    let result = svc
        .who_is_on_call(WhoIsOnCallQuery {
            actor: Actor::new(owner.id),
            schedule_id: schedule.id,
            at: Some(now),
        })
        .await
        .unwrap();

    // Build the equivalent in-memory snapshot and resolve directly with the pure resolver.
    let tz = chrono_tz::Tz::from_str(&schedule.timezone).unwrap();
    let participants = vec![u1.id, u2.id];
    let resolved_layer = ResolvedLayer {
        priority: layer.priority,
        kind: layer.kind,
        anchor_at: layer.anchor_at,
        participants: &participants,
    };
    let overrides = override_repo.list_by_schedule(schedule.id).await.unwrap();
    let snapshot = ScheduleSnapshot {
        timezone: tz,
        layers: std::slice::from_ref(&resolved_layer),
        overrides: &overrides,
    };
    let expected = resolve_on_call(&snapshot, now);

    assert_eq!(result.current_user_id, expected);
    assert!(
        result.current_user_id.is_some(),
        "expected a layer participant to be on call at `now`"
    );

    // Now verify the override-active branch: querying at an instant inside the override window
    // must resolve to the override user, matching the pure resolver at that instant too.
    let inside_override = now + Duration::days(1) + Duration::hours(1);
    let result2 = svc
        .who_is_on_call(WhoIsOnCallQuery {
            actor: Actor::new(owner.id),
            schedule_id: schedule.id,
            at: Some(inside_override),
        })
        .await
        .unwrap();
    let expected2 = resolve_on_call(&snapshot, inside_override);
    assert_eq!(result2.current_user_id, expected2);
    assert_eq!(result2.current_user_id, Some(override_user.id));
}
