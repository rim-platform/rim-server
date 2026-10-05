use super::*;

// ── On-call gap-fill: OnCallShift.isOverride + OnCallSchedule.rotationLayers ───────────

#[tokio::test]
async fn on_call_shifts_query_exposes_is_override() {
    let actor = UserId::new();
    let schedule_id = rim_core::oncall::model::OnCallScheduleId::new();
    let rotation_user = UserId::new();
    let override_user = UserId::new();
    let from = chrono::Utc::now();
    let to = from + chrono::Duration::hours(2);

    let mut oncall = rim_services::oncall::service::MockOnCallService::new();
    oncall.expect_list_shifts().returning(move |_q| {
        Box::pin(async move {
            Ok(vec![
                rim_services::oncall::dto::OnCallShiftDto {
                    user_id: Some(rotation_user),
                    start_at: from,
                    end_at: from + chrono::Duration::hours(1),
                    is_override: false,
                },
                rim_services::oncall::dto::OnCallShiftDto {
                    user_id: Some(override_user),
                    start_at: from + chrono::Duration::hours(1),
                    end_at: to,
                    is_override: true,
                },
            ])
        })
    });

    let schema = schema_with_oncall_mock(oncall);

    let req = authed_request(
        r#"query($sid: ID!, $from: DateTime!, $to: DateTime!) {
                onCallShifts(scheduleId: $sid, from: $from, to: $to) { isOverride }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": schedule_id.to_string(),
        "from": from.to_rfc3339(),
        "to": to.to_rfc3339(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    let shifts = json["onCallShifts"].as_array().unwrap();
    assert_eq!(shifts.len(), 2);
    assert_eq!(shifts[0]["isOverride"], serde_json::json!(false));
    assert_eq!(shifts[1]["isOverride"], serde_json::json!(true));
}

#[tokio::test]
async fn on_call_schedule_rotation_layers_field_resolves_nested_layers() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let participant_id = UserId::new();

    let schedule_dto = test_on_call_schedule_dto();
    let schedule_id = schedule_dto.id;

    let mut oncall = rim_services::oncall::service::MockOnCallService::new();
    oncall.expect_list_schedules().returning(move |_q| {
        let dto = schedule_dto.clone();
        Box::pin(async move { Ok(vec![dto]) })
    });
    oncall
        .expect_list_rotation_layers()
        .withf(move |q| q.schedule_id == schedule_id)
        .returning(move |_q| {
            Box::pin(async move {
                let mut layer = test_rotation_layer_dto();
                layer.schedule_id = schedule_id;
                layer.participants = vec![participant_id];
                Ok(vec![layer])
            })
        });

    let schema = schema_with_oncall_mock(oncall);

    let req = authed_request(
        r#"query($sid: ID!) {
                onCallSchedules(serverId: $sid) {
                    rotationLayers { name participants }
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    let layers = json["onCallSchedules"][0]["rotationLayers"]
        .as_array()
        .unwrap();
    assert_eq!(layers.len(), 1);
    assert_eq!(layers[0]["name"], serde_json::json!("Primary rotation"));
    assert_eq!(
        layers[0]["participants"],
        serde_json::json!([participant_id.to_string()])
    );
}
