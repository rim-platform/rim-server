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
                "avatar_url must start with /api/files/".to_string(),
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
