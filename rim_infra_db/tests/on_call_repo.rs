#![cfg(feature = "db-tests")]

mod support;

use chrono::{NaiveTime, Utc, Weekday};

use rim_core::oncall::model::{
    NewOnCallOverride, NewOnCallSchedule, NewRotationLayer, RotationKind,
};
use rim_core::oncall::repository::{
    OnCallOverrideRepository, OnCallScheduleRepository, RotationLayerRepository,
    RotationParticipantRepository,
};
use rim_infra_db::sea_orm_impl::on_call_override::SeaOrmOnCallOverrideRepository;
use rim_infra_db::sea_orm_impl::on_call_rotation_layer::SeaOrmRotationLayerRepository;
use rim_infra_db::sea_orm_impl::on_call_rotation_participant::SeaOrmRotationParticipantRepository;
use rim_infra_db::sea_orm_impl::on_call_schedule::SeaOrmOnCallScheduleRepository;

#[tokio::test]
async fn schedule_create_find_list_update_delete() {
    let ctx = support::TestCtx::new().await;
    let repo = SeaOrmOnCallScheduleRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;

    let created = repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "America/Chicago".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();
    assert_eq!(created.name, "Primary");

    let found = repo.find_by_id(created.id).await.unwrap().unwrap();
    assert_eq!(found.id, created.id);
    assert_eq!(found.timezone, "America/Chicago");

    let listed = repo.list_by_server(server.id).await.unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, created.id);

    let updated = repo
        .update(
            created.id,
            Some("Renamed".to_string()),
            Some("UTC".to_string()),
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.timezone, "UTC");

    repo.delete(created.id).await.unwrap();
    assert!(repo.find_by_id(created.id).await.unwrap().is_none());

    let err = repo.delete(created.id).await.unwrap_err();
    assert!(matches!(
        err.kind(),
        rim_core::repository::error::RepoErrorKind::NotFound
    ));
}

#[tokio::test]
async fn rotation_layer_create_find_list_update_delete_for_each_kind() {
    let ctx = support::TestCtx::new().await;
    let schedule_repo = SeaOrmOnCallScheduleRepository::new(ctx.db.clone());
    let layer_repo = SeaOrmRotationLayerRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let user = ctx.create_user("u").await;
    let schedule = schedule_repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "America/Chicago".to_string(),
            created_by: user.id,
        })
        .await
        .unwrap();

    let daily = layer_repo
        .create(NewRotationLayer {
            schedule_id: schedule.id,
            name: "Daily layer".to_string(),
            priority: 0,
            kind: RotationKind::Daily {
                handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap(),
            },
            anchor_at: Utc::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        daily.kind,
        RotationKind::Daily {
            handoff_time: NaiveTime::from_hms_opt(9, 0, 0).unwrap()
        }
    );

    let weekly = layer_repo
        .create(NewRotationLayer {
            schedule_id: schedule.id,
            name: "Weekly layer".to_string(),
            priority: 1,
            kind: RotationKind::Weekly {
                handoff_weekday: Weekday::Mon,
                handoff_time: NaiveTime::from_hms_opt(10, 30, 0).unwrap(),
            },
            anchor_at: Utc::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        weekly.kind,
        RotationKind::Weekly {
            handoff_weekday: Weekday::Mon,
            handoff_time: NaiveTime::from_hms_opt(10, 30, 0).unwrap()
        }
    );

    let custom = layer_repo
        .create(NewRotationLayer {
            schedule_id: schedule.id,
            name: "Custom layer".to_string(),
            priority: 2,
            kind: RotationKind::Custom {
                interval_seconds: 7200,
            },
            anchor_at: Utc::now(),
        })
        .await
        .unwrap();
    assert_eq!(
        custom.kind,
        RotationKind::Custom {
            interval_seconds: 7200
        }
    );

    let found = layer_repo.find_by_id(custom.id).await.unwrap().unwrap();
    assert_eq!(found.priority, 2);

    // list_by_schedule orders priority DESC.
    let listed = layer_repo.list_by_schedule(schedule.id).await.unwrap();
    assert_eq!(listed.len(), 3);
    assert_eq!(listed[0].id, custom.id);
    assert_eq!(listed[1].id, weekly.id);
    assert_eq!(listed[2].id, daily.id);

    // update() switches kind from Custom to Daily, clearing interval_seconds.
    let updated = layer_repo
        .update(
            custom.id,
            Some("Renamed".to_string()),
            Some(9),
            Some(RotationKind::Daily {
                handoff_time: NaiveTime::from_hms_opt(1, 0, 0).unwrap(),
            }),
            None,
        )
        .await
        .unwrap();
    assert_eq!(updated.name, "Renamed");
    assert_eq!(updated.priority, 9);
    assert_eq!(
        updated.kind,
        RotationKind::Daily {
            handoff_time: NaiveTime::from_hms_opt(1, 0, 0).unwrap()
        }
    );

    layer_repo.delete(daily.id).await.unwrap();
    assert!(layer_repo.find_by_id(daily.id).await.unwrap().is_none());
}

#[tokio::test]
async fn rotation_participant_set_replaces_ordering() {
    let ctx = support::TestCtx::new().await;
    let schedule_repo = SeaOrmOnCallScheduleRepository::new(ctx.db.clone());
    let layer_repo = SeaOrmRotationLayerRepository::new(ctx.db.clone());
    let participant_repo = SeaOrmRotationParticipantRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let owner = ctx.create_user("owner").await;
    let u1 = ctx.create_user("u1").await;
    let u2 = ctx.create_user("u2").await;
    let u3 = ctx.create_user("u3").await;

    let schedule = schedule_repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: owner.id,
        })
        .await
        .unwrap();
    let layer = layer_repo
        .create(NewRotationLayer {
            schedule_id: schedule.id,
            name: "Layer".to_string(),
            priority: 0,
            kind: RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: Utc::now(),
        })
        .await
        .unwrap();

    let set1 = participant_repo
        .set_participants(layer.id, &[u1.id, u2.id])
        .await
        .unwrap();
    assert_eq!(set1.len(), 2);
    assert_eq!(set1[0].user_id, u1.id);
    assert_eq!(set1[0].position, 0);
    assert_eq!(set1[1].user_id, u2.id);
    assert_eq!(set1[1].position, 1);

    // Replacing must fully clear the previous set, not append.
    let set2 = participant_repo
        .set_participants(layer.id, &[u3.id, u1.id])
        .await
        .unwrap();
    assert_eq!(set2.len(), 2);
    assert_eq!(set2[0].user_id, u3.id);
    assert_eq!(set2[1].user_id, u1.id);

    let listed = participant_repo.list_by_layer(layer.id).await.unwrap();
    assert_eq!(listed.len(), 2);

    // Clearing entirely.
    let cleared = participant_repo
        .set_participants(layer.id, &[])
        .await
        .unwrap();
    assert!(cleared.is_empty());
}

#[tokio::test]
async fn override_create_find_list_delete() {
    let ctx = support::TestCtx::new().await;
    let schedule_repo = SeaOrmOnCallScheduleRepository::new(ctx.db.clone());
    let override_repo = SeaOrmOnCallOverrideRepository::new(ctx.db.clone());

    let server = ctx.create_server("s").await;
    let owner = ctx.create_user("owner").await;
    let covering_user = ctx.create_user("covering").await;

    let schedule = schedule_repo
        .create(NewOnCallSchedule {
            server_id: server.id,
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: owner.id,
        })
        .await
        .unwrap();

    let now = Utc::now();
    let over = override_repo
        .create(NewOnCallOverride {
            schedule_id: schedule.id,
            user_id: covering_user.id,
            start_at: now,
            end_at: now + chrono::Duration::hours(2),
            created_by: owner.id,
        })
        .await
        .unwrap();
    assert_eq!(over.user_id, covering_user.id);

    let found = override_repo.find_by_id(over.id).await.unwrap().unwrap();
    assert_eq!(found.id, over.id);

    let listed = override_repo.list_by_schedule(schedule.id).await.unwrap();
    assert_eq!(listed.len(), 1);

    override_repo.delete(over.id).await.unwrap();
    assert!(override_repo.find_by_id(over.id).await.unwrap().is_none());
}
