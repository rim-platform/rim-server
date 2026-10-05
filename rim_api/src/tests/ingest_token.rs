use super::*;

#[tokio::test]
async fn create_ingest_token_mutation_requires_auth() {
    let schema = schema_with_ingest_token_mocks(
        rim_services::ingest_token::service::MockIngestTokenService::new(),
    );
    let res = schema
        .execute(Request::new(
            r#"mutation($sid: ID!, $cid: ID!) { createIngestToken(input: { serverId: $sid, conversationId: $cid, label: "prom" }) { rawToken } }"#,
        ).variables(Variables::from_json(serde_json::json!({
            "sid": ServerId::new().to_string(),
            "cid": ConversationId::new().to_string(),
        }))))
        .await;
    assert!(!res.errors.is_empty(), "expected an auth error");
}

#[tokio::test]
async fn create_ingest_token_mutation_returns_raw_token() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let conversation_id = ConversationId::new();
    let token_id = rim_core::ingest_token::model::IngestTokenId::new();

    let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
    ingest_token.expect_create_token().returning(move |cmd| {
        Box::pin(async move {
            Ok(
                rim_services::ingest_token::dto::CreateIngestTokenResultDto {
                    token: rim_services::ingest_token::dto::IngestTokenDto {
                        id: token_id,
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        created_by: cmd.actor.user_id,
                        label: cmd.label,
                        severity_default: cmd.severity_default,
                        created_at: chrono::Utc::now(),
                        revoked_at: None,
                        last_used_at: None,
                    },
                    raw_token: "rim_ingest_test_raw_token".to_string(),
                },
            )
        })
    });

    let schema = schema_with_ingest_token_mocks(ingest_token);

    let req = authed_request(
        r#"mutation($sid: ID!, $cid: ID!) { createIngestToken(input: { serverId: $sid, conversationId: $cid, label: "prom" }) { rawToken token { id label } } }"#,
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
        json["createIngestToken"]["rawToken"],
        "rim_ingest_test_raw_token"
    );
    assert_eq!(json["createIngestToken"]["token"]["label"], "prom");
}

#[tokio::test]
async fn revoke_ingest_token_mutation_returns_true() {
    let actor = UserId::new();
    let token_id = rim_core::ingest_token::model::IngestTokenId::new();

    let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
    ingest_token
        .expect_revoke_token()
        .returning(|_| Box::pin(async { Ok(()) }));

    let schema = schema_with_ingest_token_mocks(ingest_token);

    let req = authed_request(
        r#"mutation($id: ID!) { revokeIngestToken(tokenId: $id) }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "id": token_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["revokeIngestToken"], true);
}

#[tokio::test]
async fn ingest_tokens_query_returns_list() {
    let actor = UserId::new();
    let server_id = ServerId::new();
    let token_id = rim_core::ingest_token::model::IngestTokenId::new();

    let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
    ingest_token.expect_list_tokens().returning(move |_| {
        Box::pin(async move {
            Ok(vec![rim_services::ingest_token::dto::IngestTokenDto {
                id: token_id,
                server_id,
                conversation_id: ConversationId::new(),
                created_by: UserId::new(),
                label: "prom".to_string(),
                severity_default: None,
                created_at: chrono::Utc::now(),
                revoked_at: None,
                last_used_at: None,
            }])
        })
    });

    let schema = schema_with_ingest_token_mocks(ingest_token);

    let req = authed_request(
        r#"query($sid: ID!) { ingestTokens(serverId: $sid) { id label } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "sid": server_id.to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(json["ingestTokens"].as_array().unwrap().len(), 1);
    assert_eq!(json["ingestTokens"][0]["label"], "prom");
}
