use super::*;

// ── acknowledgeIncident + Incident escalation fields (Track C1c) ───────

#[tokio::test]
async fn acknowledge_incident_mutation_acknowledges() {
    let actor = UserId::new();
    let incident_id = rim_core::incident::model::IncidentId::new();

    let mut incident = MockIncidentService::new();
    incident
        .expect_acknowledge_incident()
        .withf(move |cmd| cmd.incident_id == incident_id)
        .returning(move |cmd| {
            Box::pin(async move {
                let mut dto = test_incident_dto();
                dto.id = cmd.incident_id;
                dto.acknowledged_at = Some(chrono::Utc::now());
                dto.acknowledged_by = Some(cmd.actor.user_id);
                Ok(dto)
            })
        });

    let schema = schema_with_incident_mocks(incident, MockUserService::new());

    let req = authed_request(
        r#"mutation($iid: ID!) {
                acknowledgeIncident(incidentId: $iid) { id acknowledgedAt acknowledgedBy }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "iid": incident_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert!(!json["acknowledgeIncident"]["acknowledgedAt"].is_null());
    assert_eq!(
        json["acknowledgeIncident"]["acknowledgedBy"],
        serde_json::json!(actor.to_string())
    );
}

#[tokio::test]
async fn incident_escalation_run_field_resolves_active_run() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let incident_id = rim_core::incident::model::IncidentId::new();

    let mut incident = MockIncidentService::new();
    incident.expect_get_active_incident().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_incident_dto();
            dto.id = incident_id;
            Ok(Some(dto))
        })
    });
    incident
        .expect_get_active_escalation_run()
        .withf(move |id| *id == incident_id)
        .returning(move |_| {
            Box::pin(async move {
                Ok(Some(rim_services::escalation::dto::EscalationRunDto {
                    id: rim_core::escalation::model::EscalationRunId::new(),
                    incident_id,
                    policy_id: rim_core::escalation::model::EscalationPolicyId::new(),
                    current_step_index: 1,
                    next_fire_at: Some(chrono::Utc::now()),
                    status: rim_core::escalation::model::EscalationRunStatus::Active,
                }))
            })
        });

    let schema = schema_with_incident_mocks(incident, MockUserService::new());

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    escalationRun { currentStepIndex status }
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["activeIncident"]["escalationRun"]["currentStepIndex"],
        serde_json::json!(1)
    );
    assert_eq!(
        json["activeIncident"]["escalationRun"]["status"],
        serde_json::json!("ACTIVE")
    );
}

/// No active run (never armed, or already completed/cancelled) is `null`, not an error.
#[tokio::test]
async fn incident_escalation_run_field_is_null_when_no_active_run() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut incident = MockIncidentService::new();
    incident
        .expect_get_active_incident()
        .returning(move |_q| Box::pin(async move { Ok(Some(test_incident_dto())) }));
    incident
        .expect_get_active_escalation_run()
        .returning(|_| Box::pin(async { Ok(None) }));

    let schema = schema_with_incident_mocks(incident, MockUserService::new());

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) { escalationRun { status } }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert!(json["activeIncident"]["escalationRun"].is_null());
}

#[tokio::test]
async fn incident_acknowledged_by_user_field_resolves_nested_user() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let acker_id = UserId::new();

    let mut incident = MockIncidentService::new();
    incident.expect_get_active_incident().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_incident_dto();
            dto.acknowledged_at = Some(chrono::Utc::now());
            dto.acknowledged_by = Some(acker_id);
            Ok(Some(dto))
        })
    });

    let mut user = MockUserService::new();
    user.expect_get_users_by_ids().returning(move |ids| {
        let ids = ids.to_vec();
        Box::pin(async move {
            Ok(ids
                .into_iter()
                .map(|id| {
                    let mut dto = test_user_dto();
                    dto.id = id;
                    dto.username = "acker".to_string();
                    dto
                })
                .collect())
        })
    });

    let schema = schema_with_incident_mocks(incident, user);

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    acknowledgedBy
                    acknowledgedByUser { id username }
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["activeIncident"]["acknowledgedBy"],
        serde_json::json!(acker_id.to_string())
    );
    assert_eq!(
        json["activeIncident"]["acknowledgedByUser"]["username"],
        serde_json::json!("acker")
    );
}

/// `acknowledgedByUser` must be `null`, not an error, when the incident hasn't been
/// acknowledged — the loader must not even be invoked (zero expectations on `user`).
#[tokio::test]
async fn incident_acknowledged_by_user_field_is_null_when_not_acknowledged() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut incident = MockIncidentService::new();
    incident
        .expect_get_active_incident()
        .returning(move |_q| Box::pin(async move { Ok(Some(test_incident_dto())) }));

    let schema = schema_with_incident_mocks(incident, MockUserService::new());

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    acknowledgedBy
                    acknowledgedByUser { id }
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert!(json["activeIncident"]["acknowledgedBy"].is_null());
    assert!(json["activeIncident"]["acknowledgedByUser"].is_null());
}
