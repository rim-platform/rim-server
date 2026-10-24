use super::*;

#[tokio::test]
async fn query_requires_auth() {
    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let res = schema
        .execute(Request::new(
            r#"query { conversation(serverId: "00000000-0000-0000-0000-000000000000", conversationId: "00000000-0000-0000-0000-000000000000") { id } }"#,
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
async fn service_error_maps_to_graphql_code() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut conv = MockConversationService::new();
    conv.expect_get_conversation()
        .returning(|_q| Box::pin(async { Err(ServiceError::PermissionDenied) }));

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) { conversation(serverId: $sid, conversationId: $cid) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;

    assert!(!res.errors.is_empty());
    assert_eq!(
        res.errors[0]
            .extensions
            .as_ref()
            .and_then(|e| e.get("code")),
        Some(&Value::from("FORBIDDEN"))
    );
}

#[tokio::test]
async fn service_error_unauthenticated_maps_to_graphql_code() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();

    let mut conv = MockConversationService::new();
    conv.expect_get_conversation()
        .returning(|_q| Box::pin(async { Err(ServiceError::Unauthenticated) }));

    let schema = schema_with_mocks(
        MockAuthService::new(),
        conv,
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let req = authed_request(
        r#"query($sid: ID!, $cid: ID!) { conversation(serverId: $sid, conversationId: $cid) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
        "cid": conversation_id.to_string(),
    })));

    let res = schema.execute(req).await;

    assert!(!res.errors.is_empty());
    assert_eq!(
        res.errors[0]
            .extensions
            .as_ref()
            .and_then(|e| e.get("code")),
        Some(&Value::from("UNAUTHENTICATED"))
    );
}

#[tokio::test]
async fn mutation_logout_requires_actor_and_session() {
    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let res = schema.execute(Request::new(r#"mutation { logout }"#)).await;

    assert!(!res.errors.is_empty());
    assert!(res.errors[0].message.contains("unauthenticated"));
}

#[tokio::test]
async fn mutation_logout_sets_auth_intent_clear_session() {
    let actor = UserId::new();
    let session_id = SessionId::new();

    let mut auth = MockAuthService::new();
    auth.expect_logout().returning(move |cmd| {
        assert_eq!(cmd.session_id, session_id);
        Box::pin(async { Ok(()) })
    });

    let schema = schema_with_mocks(
        auth,
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let intent_ctx = Arc::new(crate::auth_intent::AuthIntentCtx::default());
    let req = Request::new(r#"mutation { logout }"#)
        .data(Arc::clone(&intent_ctx))
        .data(AuthActorCtx::new(Some(actor), Some(session_id)));

    let res = schema.execute(req).await;
    assert!(res.errors.is_empty());

    let intent = intent_ctx.take();
    assert!(matches!(
        intent,
        Some(crate::auth_intent::AuthIntent::ClearSession)
    ));
}

#[tokio::test]
async fn query_id_parsing_error_is_reported() {
    let actor = UserId::new();

    let schema = schema_with_mocks(
        MockAuthService::new(),
        MockConversationService::new(),
        MockMessageService::new(),
        MockMessageAttachmentService::new(),
        MockThreadService::new(),
    );

    let res = schema
        .execute(authed_request(
            r#"query { message(serverId: "bad", conversationId: "bad", messageId: "bad") { id } }"#,
            actor,
        ))
        .await;

    assert!(!res.errors.is_empty());
    // Don't assert exact message, just that it fails before hitting the service.
}
