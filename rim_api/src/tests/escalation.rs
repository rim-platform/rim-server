use super::*;

// ── Escalation policies (Track C1c) ────────────────────────────────────

#[tokio::test]
async fn escalation_policies_query_returns_policies() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let mut escalation_policy =
        rim_services::escalation::service::MockEscalationPolicyService::new();
    escalation_policy
        .expect_list_policies()
        .returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_escalation_policy_dto();
                dto.server_id = server_id;
                Ok(vec![dto])
            })
        });

    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"query($sid: ID!) {
                escalationPolicies(serverId: $sid) {
                    id name steps { position delaySeconds targetKind targetUserId actions }
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
    let policies = json["escalationPolicies"].as_array().unwrap();
    assert_eq!(policies.len(), 1);
    assert_eq!(policies[0]["name"], serde_json::json!("Primary"));
    assert_eq!(
        policies[0]["steps"][0]["targetKind"],
        serde_json::json!("USER")
    );
    assert_eq!(
        policies[0]["steps"][0]["actions"][0],
        serde_json::json!("EMAIL")
    );
}

#[tokio::test]
async fn create_escalation_policy_mutation_creates_policy() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let target_user_id = UserId::new();

    let mut escalation_policy =
        rim_services::escalation::service::MockEscalationPolicyService::new();
    escalation_policy
        .expect_create_policy()
        .withf(move |cmd| {
            cmd.server_id == server_id
                && cmd.name == "Primary"
                && cmd.steps.len() == 1
                && cmd.steps[0].target
                    == rim_core::escalation::model::EscalationTarget::User(target_user_id)
        })
        .returning(move |cmd| {
            Box::pin(async move {
                let mut dto = test_escalation_policy_dto();
                dto.server_id = server_id;
                dto.name = cmd.name;
                Ok(dto)
            })
        });

    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"mutation($sid: ID!, $uid: ID!) {
                createEscalationPolicy(
                    serverId: $sid
                    name: "Primary"
                    steps: [{ delaySeconds: 0, target: { kind: USER, userId: $uid }, actions: [EMAIL] }]
                ) { name }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "uid": target_user_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["createEscalationPolicy"]["name"],
        serde_json::json!("Primary")
    );
}

/// The flattened `EscalationTargetInput` rejects a field set for the wrong kind before it
/// ever reaches the service — mirrors `RotationKindInputGql`'s strict-rejection tests.
#[tokio::test]
async fn create_escalation_policy_mutation_rejects_mismatched_target_fields() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let schedule_id = rim_core::oncall::model::OnCallScheduleId::new();

    // No expectations set — the input must be rejected before the service is ever called.
    let escalation_policy = rim_services::escalation::service::MockEscalationPolicyService::new();
    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"mutation($sid: ID!, $schedId: ID!) {
                createEscalationPolicy(
                    serverId: $sid
                    name: "Primary"
                    steps: [{ delaySeconds: 0, target: { kind: ON_CALL, scheduleId: $schedId, userId: $schedId }, actions: [EMAIL] }]
                ) { name }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "schedId": schedule_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert!(
        !res.errors.is_empty(),
        "userId set alongside kind: ON_CALL must be rejected"
    );
}

#[tokio::test]
async fn update_escalation_policy_mutation_renames_policy() {
    let actor = UserId::new();
    let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

    let mut escalation_policy =
        rim_services::escalation::service::MockEscalationPolicyService::new();
    escalation_policy
        .expect_update_policy()
        .withf(move |cmd| {
            cmd.policy_id == policy_id
                && cmd.name.as_deref() == Some("Renamed")
                && cmd.steps.is_none()
        })
        .returning(move |cmd| {
            Box::pin(async move {
                let mut dto = test_escalation_policy_dto();
                dto.id = cmd.policy_id;
                dto.name = cmd.name.unwrap();
                Ok(dto)
            })
        });

    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"mutation($pid: ID!) {
                updateEscalationPolicy(policyId: $pid, name: "Renamed") { name }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "pid": policy_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["updateEscalationPolicy"]["name"],
        serde_json::json!("Renamed")
    );
}

#[tokio::test]
async fn delete_escalation_policy_mutation_deletes() {
    let actor = UserId::new();
    let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

    let mut escalation_policy =
        rim_services::escalation::service::MockEscalationPolicyService::new();
    escalation_policy
        .expect_delete_policy()
        .withf(move |cmd| cmd.policy_id == policy_id)
        .returning(|_| Box::pin(async { Ok(()) }));

    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"mutation($pid: ID!) { deleteEscalationPolicy(policyId: $pid) }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "pid": policy_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["deleteEscalationPolicy"], serde_json::json!(true));
}

#[tokio::test]
async fn set_channel_escalation_policy_mutation_sets_default() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

    let mut escalation_policy =
        rim_services::escalation::service::MockEscalationPolicyService::new();
    escalation_policy
        .expect_set_channel_policy()
        .withf(move |cmd| {
            cmd.server_id == server_id
                && cmd.conversation_id == conversation_id
                && cmd.policy_id == policy_id
        })
        .returning(|_| Box::pin(async { Ok(()) }));

    let schema = schema_with_escalation_policy_mock(escalation_policy);

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!, $pid: ID!) {
                setChannelEscalationPolicy(serverId: $sid, conversationId: $cid, policyId: $pid)
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
        "pid": policy_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["setChannelEscalationPolicy"], serde_json::json!(true));
}
