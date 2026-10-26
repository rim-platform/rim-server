use super::*;

// ── me query ───────────────────────────────────────────────────────────

#[tokio::test]
async fn me_query_returns_current_user() {
    let actor = UserId::new();

    let mut user = MockUserService::new();
    user.expect_get_me().returning(move |_q| {
        Box::pin(async move {
            let mut dto = test_user_dto();
            dto.id = actor;
            dto.username = "alice".to_string();
            dto.display_name = Some("Alice".to_string());
            Ok(dto)
        })
    });
    // Asked only when `probationEndsAt` is selected.
    user.expect_probation_ends_at().never();

    let schema = schema_with_user_mock(user);

    let req = authed_request(r#"query { me { id username displayName } }"#, actor);
    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(json["me"]["id"], serde_json::json!(actor.to_string()));
    assert_eq!(json["me"]["username"], serde_json::json!("alice"));
    assert_eq!(json["me"]["displayName"], serde_json::json!("Alice"));
}

#[tokio::test]
async fn me_reports_when_the_callers_probation_ends() {
    let actor = UserId::new();
    let ends_at = chrono::DateTime::parse_from_rfc3339("2026-10-13T08:30:00Z")
        .expect("time")
        .with_timezone(&chrono::Utc);
    let mut user = MockUserService::new();
    user.expect_get_me()
        .returning(|_| Box::pin(async { Ok(test_user_dto()) }));
    user.expect_probation_ends_at()
        .withf(move |q| q.actor.user_id == actor)
        .times(1)
        .returning(move |_| Box::pin(async move { Ok(Some(ends_at)) }));
    let schema = schema_with_user_mock(user);

    let res = schema
        .execute(authed_request(r#"query { me { probationEndsAt } }"#, actor))
        .await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["me"]["probationEndsAt"],
        serde_json::json!("2026-10-13T08:30:00+00:00")
    );
}

#[tokio::test]
async fn me_reports_until_when_the_callers_profile_is_locked() {
    let actor = UserId::new();
    let until = chrono::DateTime::parse_from_rfc3339("2026-11-02T08:30:00Z")
        .expect("time")
        .with_timezone(&chrono::Utc);
    let mut user = MockUserService::new();
    user.expect_get_me().returning(move |_| {
        Box::pin(async move {
            Ok(rim_services::user::dto::UserDto {
                profile_locked_until: Some(until),
                ..test_user_dto()
            })
        })
    });
    let schema = schema_with_user_mock(user);

    let res = schema
        .execute(authed_request(
            r#"query { me { profileLockedUntil } }"#,
            actor,
        ))
        .await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["me"]["profileLockedUntil"],
        serde_json::json!("2026-11-02T08:30:00+00:00")
    );
}

#[tokio::test]
async fn me_query_requires_auth() {
    let schema = schema_with_user_mock(MockUserService::new());

    let res = schema.execute(Request::new(r#"query { me { id } }"#)).await;

    assert!(!res.errors.is_empty());
    assert!(
        res.errors[0].message.contains("unauthenticated"),
        "got: {:?}",
        res.errors[0].message
    );
}

// ── updateMyProfile mutation ───────────────────────────────────────────

#[tokio::test]
async fn update_my_profile_mutation_returns_updated_user() {
    let actor = UserId::new();
    let mut user = MockUserService::new();
    user.expect_update_my_profile().returning(move |cmd| {
        Box::pin(async move {
            let mut dto = test_user_dto();
            dto.id = cmd.actor.user_id;
            dto.display_name = cmd.display_name;
            dto.avatar_url = cmd.avatar_url;
            dto.status_text = cmd.status_text;
            dto.bio = cmd.bio;
            Ok(dto)
        })
    });

    let schema = schema_with_user_mock(user);

    let req = authed_request(
        r#"mutation($input: UpdateMyProfileInput!) {
                updateMyProfile(input: $input) {
                    id displayName avatarUrl statusText bio
                }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "input": {
            "displayName": "Alice",
            "avatarUrl": "/api/files/avatar/abc",
            "statusText": "busy",
            "bio": "hello"
        }
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["updateMyProfile"]["id"],
        serde_json::json!(actor.to_string())
    );
    assert_eq!(
        json["updateMyProfile"]["displayName"],
        serde_json::json!("Alice")
    );
    assert_eq!(
        json["updateMyProfile"]["avatarUrl"],
        serde_json::json!("/api/files/avatar/abc")
    );
}

#[tokio::test]
async fn update_my_profile_mutation_requires_auth() {
    let schema = schema_with_user_mock(MockUserService::new());

    let res = schema
        .execute(Request::new(
            r#"mutation { updateMyProfile(input: {}) { id } }"#,
        ))
        .await;

    assert!(!res.errors.is_empty());
    assert!(
        res.errors[0].message.contains("unauthenticated"),
        "got: {:?}",
        res.errors[0].message
    );
}

#[tokio::test]
async fn update_my_profile_mutation_maps_validation_error() {
    let actor = UserId::new();
    let mut user = MockUserService::new();
    user.expect_update_my_profile().returning(|_cmd| {
        Box::pin(async move {
            Err(ServiceError::Validation(
                "avatar_url must be the URL of your own uploaded avatar".to_string(),
            ))
        })
    });

    let schema = schema_with_user_mock(user);

    let req = authed_request(
        r#"mutation($input: UpdateMyProfileInput!) {
                updateMyProfile(input: $input) { id }
            }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "input": { "avatarUrl": "https://evil.com/x.png" }
    })));

    let res = schema.execute(req).await;
    assert_eq!(res.errors.len(), 1);
    assert!(res.errors[0].message.contains("avatar_url"));
    assert_eq!(
        res.errors[0]
            .extensions
            .as_ref()
            .and_then(|e| e.get("code")),
        Some(&Value::from("VALIDATION"))
    );
}

/// A federation context whose host says whether a user is remote.
fn federation_with_remote(remote: bool) -> crate::federation::FederationContext {
    let mut host = rim_services::host::service::MockHostService::new();
    host.expect_is_remote_user()
        .returning(move |_| Box::pin(async move { Ok(remote) }));
    crate::federation::FederationContext {
        relay: Arc::new(rim_services::relay::service::MockRelayService::new()),
        host: Arc::new(host),
        workspace_host: Arc::new(
            rim_services::remote_workspace::service::MockWorkspaceHostService::new(),
        ),
        workspace_relay: Arc::new(
            rim_services::remote_workspace::service::MockWorkspaceRelayService::new(),
        ),
        info: rim_services::relay::dto::RelayInfoDto {
            server_id: "rims1_test".to_string(),
            roles: vec!["relay".to_string(), "host".to_string()],
            direct_space_id: ServerId::new(),
        },
        relay_role: true,
        host_role: true,
    }
}

#[tokio::test]
async fn update_my_profile_refuses_a_remote_members_display_name() {
    let actor = UserId::new();
    let schema = schema_with_user_mock(MockUserService::new());

    let req = authed_request(
        r#"mutation { updateMyProfile(input: { displayName: "Bob" }) { id } }"#,
        actor,
    )
    .data(federation_with_remote(true));

    let res = schema.execute(req).await;
    assert_eq!(res.errors.len(), 1);
    let code = res.errors[0]
        .extensions
        .as_ref()
        .and_then(|e| e.get("code"))
        .cloned();
    assert_eq!(code, Some(Value::from("FORBIDDEN")));
}

#[tokio::test]
async fn update_my_profile_lets_a_remote_member_set_avatar_and_status() {
    let actor = UserId::new();
    let mut user = MockUserService::new();
    user.expect_update_my_profile().returning(move |cmd| {
        Box::pin(async move {
            let mut dto = test_user_dto();
            dto.id = cmd.actor.user_id;
            dto.avatar_url = cmd.avatar_url;
            dto.status_text = cmd.status_text;
            Ok(dto)
        })
    });
    let schema = schema_with_user_mock(user);

    // No display name in the input: the host is not even asked whether the user is remote.
    let mut fed = federation_with_remote(true);
    let mut host = rim_services::host::service::MockHostService::new();
    host.expect_is_remote_user().never();
    fed.host = Arc::new(host);
    let req = authed_request(
        r#"mutation {
                updateMyProfile(input: { avatarUrl: "/api/files/avatars/x/avatar.png", statusText: "away" }) {
                    avatarUrl statusText
                }
            }"#,
        actor,
    )
    .data(fed);

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["updateMyProfile"]["avatarUrl"],
        serde_json::json!("/api/files/avatars/x/avatar.png")
    );
    assert_eq!(
        json["updateMyProfile"]["statusText"],
        serde_json::json!("away")
    );
}

#[tokio::test]
async fn update_my_profile_lets_a_local_user_set_a_display_name() {
    let actor = UserId::new();
    let mut user = MockUserService::new();
    user.expect_update_my_profile().returning(move |cmd| {
        Box::pin(async move {
            let mut dto = test_user_dto();
            dto.id = cmd.actor.user_id;
            dto.display_name = cmd.display_name;
            Ok(dto)
        })
    });
    let schema = schema_with_user_mock(user);

    let req = authed_request(
        r#"mutation { updateMyProfile(input: { displayName: "Ada" }) { displayName } }"#,
        actor,
    )
    .data(federation_with_remote(false));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["updateMyProfile"]["displayName"],
        serde_json::json!("Ada")
    );
}
