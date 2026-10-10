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
        .expect_get_member()
        .withf(move |q| q.server_id == server_id && q.user_id == actor)
        .times(1)
        .returning(|_q| Box::pin(async { Err(ServiceError::PermissionDenied) }));

    let schema = schema_with_server_mocks(MockServerService::new(), server_member);
    let res = schema
        .execute(workspace_presence_request(actor, server_id))
        .await;

    assert!(
        !res.errors.is_empty(),
        "a non-member must not read workspace presence"
    );
}

#[tokio::test]
async fn workspace_presence_allows_member() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let mut server_member = MockServerMemberService::new();
    server_member
        .expect_get_member()
        .times(1)
        .returning(|_q| Box::pin(async { Ok(test_server_member_dto()) }));

    let schema = schema_with_server_mocks(MockServerService::new(), server_member);
    let res = schema
        .execute(workspace_presence_request(actor, server_id))
        .await;

    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["workspacePresence"], serde_json::json!([]));
}
