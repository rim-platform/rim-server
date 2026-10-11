use super::*;

// ── server query ─────────────────────────────────────────────────────

#[tokio::test]
async fn server_query_returns_server() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let mut server = MockServerService::new();
    server.expect_get_server().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_server_dto();
            dto.id = server_id;
            dto.name = "My Server".to_string();
            Ok(dto)
        })
    });

    let schema = schema_with_server_mocks(server, MockServerMemberService::new());

    let req = authed_request(
        r#"query($sid: ID!) { server(serverId: $sid) { id name } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["server"]["id"],
        serde_json::json!(server_id.to_string())
    );
    assert_eq!(json["server"]["name"], serde_json::json!("My Server"));
}

// ── Server.viewerRole ─────────────────────────────────────────────────

/// Builds a schema + query for the `viewerRole` sub-field test cases below, differing only
/// in what `get_member` returns for the requesting actor.
async fn run_viewer_role_query(
    actor: UserId,
    server_id: ServerId,
    get_member_result: impl Fn() -> rim_services::ServiceResult<
        rim_services::server_member::dto::ServerMemberDto,
    > + Send
    + Sync
    + 'static,
) -> async_graphql::Response {
    let mut server = MockServerService::new();
    server.expect_get_server().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_server_dto();
            dto.id = server_id;
            Ok(dto)
        })
    });

    let mut server_member = MockServerMemberService::new();
    server_member
        .expect_get_member()
        .returning(move |_q| Box::pin(std::future::ready(get_member_result())));

    let schema = schema_with_server_mocks(server, server_member);

    let req = authed_request(
        r#"query($sid: ID!) { server(serverId: $sid) { id viewerRole } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    schema.execute(req).await
}

#[tokio::test]
async fn viewer_role_resolves_owner() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let res = run_viewer_role_query(actor, server_id, move || {
        let mut dto = test_server_member_dto();
        dto.server_id = server_id;
        dto.user_id = actor;
        dto.member_role = rim_core::server_member::model::ServerMemberRole::Owner;
        Ok(dto)
    })
    .await;

    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["server"]["viewerRole"], serde_json::json!("OWNER"));
}

#[tokio::test]
async fn viewer_role_resolves_moderator() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let res = run_viewer_role_query(actor, server_id, move || {
        let mut dto = test_server_member_dto();
        dto.server_id = server_id;
        dto.user_id = actor;
        dto.member_role = rim_core::server_member::model::ServerMemberRole::Moderator;
        Ok(dto)
    })
    .await;

    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["server"]["viewerRole"], serde_json::json!("MODERATOR"));
}

#[tokio::test]
async fn viewer_role_resolves_guest() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let res = run_viewer_role_query(actor, server_id, move || {
        let mut dto = test_server_member_dto();
        dto.server_id = server_id;
        dto.user_id = actor;
        dto.member_role = rim_core::server_member::model::ServerMemberRole::Guest;
        Ok(dto)
    })
    .await;

    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["server"]["viewerRole"], serde_json::json!("GUEST"));
}

/// Non-member path: `viewerRole` is non-null in the schema, so a caller who isn't a member
/// must get a GraphQL error (matching existing server-visibility behavior), never a fake
/// role.
#[tokio::test]
async fn viewer_role_non_member_returns_error_not_a_fake_role() {
    let actor = UserId::new();
    let server_id = ServerId::new();

    let res = run_viewer_role_query(actor, server_id, || {
        Err(rim_services::ServiceError::PermissionDenied)
    })
    .await;

    assert!(
        !res.errors.is_empty(),
        "non-member viewerRole query must error, not resolve a fake role"
    );
}

// ── serverMembers query ──────────────────────────────────────────────

#[tokio::test]
async fn server_members_query_returns_members() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let member_user_id = UserId::new();
    let joined_at = chrono::Utc::now();

    let mut server_member = MockServerMemberService::new();
    server_member.expect_list_members().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_server_member_dto();
            dto.server_id = server_id;
            dto.user_id = member_user_id;
            dto.member_role = rim_core::server_member::model::ServerMemberRole::Moderator;
            dto.joined_at = joined_at;
            Ok(vec![dto])
        })
    });

    let schema = schema_with_server_mocks(MockServerService::new(), server_member);

    let req = authed_request(
        r#"query($sid: ID!) { serverMembers(serverId: $sid) { userId memberRole joinedAt } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["serverMembers"][0]["userId"],
        serde_json::json!(member_user_id.to_string())
    );
    assert_eq!(
        json["serverMembers"][0]["memberRole"],
        serde_json::json!("MODERATOR")
    );
    assert!(json["serverMembers"][0]["joinedAt"].is_string());
}
