#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_graphql::{Request, Value, Variables};

    use rim_core::auth::model::SessionId;
    use rim_core::conversation::model::ConversationId;
    use rim_core::message::model::MessageId;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;

    use rim_services::auth::service::MockAuthService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_thread::service::MockThreadService;
    use rim_services::ServiceError;

    use crate::build_schema;
    use crate::query::AuthActorCtx;

    fn schema_with_mocks(
        auth: MockAuthService,
        conv: MockConversationService,
        msg: MockMessageService,
        thread: MockThreadService,
    ) -> crate::AppSchema {
        build_schema(Arc::new(auth), Arc::new(conv), Arc::new(msg), Arc::new(thread))
    }

    #[tokio::test]
    async fn query_requires_auth() {
        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
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
        conv.expect_get_conversation().returning(|_q| {
            Box::pin(async { Err(ServiceError::PermissionDenied) })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!) { conversation(serverId: $sid, conversationId: $cid) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
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
    async fn mutation_logout_requires_actor_and_session() {
        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let res = schema
            .execute(Request::new(r#"mutation { logout }"#))
            .await;

        assert!(!res.errors.is_empty());
        assert!(res.errors[0].message.contains("unauthenticated"));
    }

    #[tokio::test]
    async fn mutation_login_sets_auth_intent_set_session() {
        let user_id = UserId::new();
        let session_id = SessionId::new();
        let now = chrono::Utc::now();

        let mut auth = MockAuthService::new();
        auth.expect_login().returning(move |_u, _p| {
            Box::pin(async move {
                Ok(rim_services::auth::dto::LoginResultDto {
                    user_id,
                    session: rim_services::auth::dto::AuthSessionDto {
                        user_id,
                        session_id,
                        expires_at: now,
                    },
                    session_token: "rim_sess_test".to_string(),
                    csrf_token: "rim_csrf_test".to_string(),
                })
            })
        });

        let schema = schema_with_mocks(
            auth,
            MockConversationService::new(),
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let intent_ctx = Arc::new(crate::auth_intent::AuthIntentCtx::default());
        let req = Request::new(
            r#"mutation { login(input: { username: "u", password: "p" }) { userId } }"#,
        )
        .data(Arc::clone(&intent_ctx));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let intent = intent_ctx.take();
        assert!(matches!(
            intent,
            Some(crate::auth_intent::AuthIntent::SetSession { .. })
        ));
    }

    #[tokio::test]
    async fn mutation_logout_sets_auth_intent_clear_session() {
        let actor = UserId::new();
        let session_id = SessionId::new();

        let mut auth = MockAuthService::new();
        auth.expect_logout().returning(move |_actor, sid| {
            assert_eq!(sid, session_id);
            Box::pin(async { Ok(()) })
        });

        let schema = schema_with_mocks(
            auth,
            MockConversationService::new(),
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let intent_ctx = Arc::new(crate::auth_intent::AuthIntentCtx::default());
        let req = Request::new(r#"mutation { logout }"#)
            .data(Arc::clone(&intent_ctx))
            .data(AuthActorCtx::new(Some(actor), Some(session_id)));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty());

        let intent = intent_ctx.take();
        assert!(matches!(intent, Some(crate::auth_intent::AuthIntent::ClearSession)));
    }

    #[tokio::test]
    async fn query_id_parsing_error_is_reported() {
        let actor = UserId::new();

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let res = schema
            .execute(
                Request::new(
                    r#"query { message(serverId: "bad", conversationId: "bad", messageId: "bad") { id } }"#,
                )
                .data(AuthActorCtx::new(Some(actor), Some(SessionId::new()))),
            )
            .await;

        assert!(!res.errors.is_empty());
        // Don't assert exact message, just that it fails before hitting the service.
    }

    #[tokio::test]
    async fn thread_query_calls_service_when_authed() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let thread_id = MessageThreadId::new();

        let mut thread = MockThreadService::new();
        thread.expect_get_thread().returning(move |_q| {
            Box::pin(async move {
                Ok(rim_services::message_thread::dto::ThreadDto {
                id: thread_id,
                server_id,
                conversation_id,
                root_message_id: None,
                root_author_id: None,
                root_text: None,
                created_at: chrono::Utc::now(),
                last_message_at: chrono::Utc::now(),
                })
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            thread,
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!, $tid: ID!) { thread(serverId: $sid, conversationId: $cid, threadId: $tid) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "tid": thread_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty());
        assert!(res.data != Value::Null);
    }

    // Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
    #[test]
    fn _keep_ids_referenced() {
        let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
    }
}
