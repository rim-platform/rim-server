use super::*;

// ── workspacePresence ────────────────────────────────────────────────────

fn workspace_presence_request(actor: UserId, server_id: ServerId) -> Request {
    authed_request(
        r#"query($sid: ID!) { workspacePresence(workspaceId: $sid) { userId status } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })))
}

#[tokio::test]
async fn workspace_presence_rejects_non_member() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let mut server_member = MockServerMemberService::new();
    server_member
        .expect_visible_members()
        .withf(move |q| q.server_id == server_id && q.actor.user_id == actor)
        .times(1)
        .returning(|_q| Box::pin(async { Err(ServiceError::PermissionDenied) }));

    let schema = schema_with_server_mocks(MockServerService::new(), server_member);
    let res = schema
        .execute(workspace_presence_request(actor, server_id))
        .await;

    let code = res
        .errors
        .first()
        .and_then(|e| e.extensions.as_ref())
        .and_then(|ext| ext.get("code").cloned());
    assert_eq!(
        code,
        Some(Value::from("FORBIDDEN")),
        "a non-member must not read workspace presence"
    );
}

#[tokio::test]
async fn workspace_presence_allows_member() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let mut server_member = MockServerMemberService::new();
    server_member
        .expect_visible_members()
        .times(1)
        .returning(|_q| Box::pin(async { Ok(None) }));

    let schema = schema_with_server_mocks(MockServerService::new(), server_member);
    let res = schema
        .execute(workspace_presence_request(actor, server_id))
        .await;

    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["workspacePresence"], serde_json::json!([]));
}
