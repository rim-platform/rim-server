#[cfg(test)]
#[allow(clippy::module_inception)]
mod tests {
    use std::sync::Arc;

    use async_graphql::{Request, Value, Variables};

    use rim_core::auth::model::SessionId;
    use rim_core::conversation::model::ConversationId;
    use rim_core::conversation::model::ConversationType;
    use rim_core::message::model::MessageId;
    use rim_core::message_thread::model::MessageThreadId;
    use rim_core::repository::pagination::Page;
    use rim_core::server::model::ServerId;
    use rim_core::user::model::UserId;

    use rim_services::ServiceError;
    use rim_services::auth::service::MockAuthService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_thread::service::MockThreadService;

    use crate::build_schema;
    use crate::query::AuthActorCtx;

    fn schema_with_mocks(
        auth: MockAuthService,
        conv: MockConversationService,
        msg: MockMessageService,
        thread: MockThreadService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(auth),
            Arc::new(conv),
            Arc::new(msg),
            Arc::new(thread),
        )
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
        conv.expect_get_conversation()
            .returning(|_q| Box::pin(async { Err(ServiceError::PermissionDenied) }));

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

        let res = schema.execute(Request::new(r#"mutation { logout }"#)).await;

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
    async fn message_query_exposes_root_thread_id() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let root_thread_id = MessageThreadId::new();

        let mut msg = MockMessageService::new();
        msg.expect_get_message().returning(move |_q| {
            Box::pin(async move {
                Ok(rim_services::message::dto::MessageDto {
                    id: message_id,
                    server_id,
                    conversation_id,
                    thread_id: None,
                    root_thread_id: Some(root_thread_id),
                    author_id: actor,
                    text: Some("root".to_string()),
                    has_attachments: false,
                    parent_message_id: None,
                    is_deleted: false,
                    sent_at: chrono::Utc::now(),
                })
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id rootThreadId } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
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

    #[tokio::test]
    async fn thread_by_root_message_query_calls_service_when_authed() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let root_message_id = MessageId::new();
        let thread_id = MessageThreadId::new();

        let mut thread = MockThreadService::new();
        thread
            .expect_get_thread_by_root_message()
            .returning(move |_q| {
                Box::pin(async move {
                    Ok(Some(rim_services::message_thread::dto::ThreadDto {
                        id: thread_id,
                        server_id,
                        conversation_id,
                        root_message_id: Some(root_message_id),
                        root_author_id: Some(actor),
                        root_text: Some("root".to_string()),
                        created_at: chrono::Utc::now(),
                        last_message_at: chrono::Utc::now(),
                    }))
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            thread,
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { threadByRootMessage(serverId: $sid, conversationId: $cid, rootMessageId: $mid) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": root_message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty());
        assert!(res.data != Value::Null);
    }

    #[tokio::test]
    async fn mutation_create_thread_calls_service_when_authed() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let thread_id = MessageThreadId::new();

        let mut thread = MockThreadService::new();
        thread.expect_create_thread().returning(move |_cmd| {
            Box::pin(async move {
                Ok(rim_services::message_thread::dto::ThreadDto {
                    id: thread_id,
                    server_id,
                    conversation_id,
                    root_message_id: Some(message_id),
                    root_author_id: Some(actor),
                    root_text: Some("root".to_string()),
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
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { createThread(input: { serverId: $sid, conversationId: $cid, rootMessageId: $mid }) { id rootMessageId } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn mutation_send_thread_message_calls_service_when_authed() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_send_thread_message().returning(move |_cmd| {
            Box::pin(async move {
                Ok(rim_services::message::dto::MessageDto {
                    id: message_id,
                    server_id,
                    conversation_id,
                    thread_id: Some(thread_id),
                    root_thread_id: None,
                    author_id: actor,
                    text: Some("reply".to_string()),
                    has_attachments: false,
                    parent_message_id: None,
                    is_deleted: false,
                    sent_at: chrono::Utc::now(),
                })
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"mutation($sid: ID!, $cid: ID!, $tid: ID!) { sendThreadMessage(input: { serverId: $sid, conversationId: $cid, threadId: $tid, text: "reply" }) { id threadId } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "tid": thread_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn messages_page_exposes_has_next_without_has_prev() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let next_message_id = MessageId::new();
        let sent_at = chrono::Utc::now();

        let mut msg = MockMessageService::new();
        msg.expect_list_messages().returning(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![rim_services::message::dto::MessageDto {
                        id: message_id,
                        server_id,
                        conversation_id,
                        thread_id: None,
                        root_thread_id: None,
                        author_id: actor,
                        text: Some("hello".to_string()),
                        has_attachments: false,
                        parent_message_id: None,
                        is_deleted: false,
                        sent_at,
                    }],
                    Some(rim_services::message::dto::MessagePageCursor {
                        sent_at,
                        message_id: next_message_id,
                    }),
                    None,
                ))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!) { messages(serverId: $sid, conversationId: $cid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["messages"]["pageInfo"]["hasNext"],
            serde_json::json!(true)
        );
        assert_eq!(
            json["messages"]["pageInfo"]["hasPrev"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["messages"]["next"]["messageId"],
            serde_json::json!(next_message_id.to_string())
        );
        assert_eq!(json["messages"]["prev"], serde_json::Value::Null);
    }

    #[tokio::test]
    async fn inbox_page_exposes_has_prev_without_has_next() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let prev_conversation_id = ConversationId::new();
        let last_activity_at = chrono::Utc::now();

        let mut conv = MockConversationService::new();
        conv.expect_list_inbox_conversations().returning(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![rim_services::conversation::dto::ConversationDto {
                        id: conversation_id,
                        server_id,
                        conversation_type: ConversationType::Direct,
                        title: None,
                        member_ids: vec![actor],
                    }],
                    None,
                    Some(rim_services::conversation::dto::InboxPageCursor {
                        last_activity_at,
                        conversation_id: prev_conversation_id,
                    }),
                ))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!) { inboxConversations(serverId: $sid, dir: BEFORE, limit: 20) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["inboxConversations"]["pageInfo"]["hasNext"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["inboxConversations"]["pageInfo"]["hasPrev"],
            serde_json::json!(true)
        );
        assert_eq!(json["inboxConversations"]["next"], serde_json::Value::Null);
        assert_eq!(
            json["inboxConversations"]["prev"]["conversationId"],
            serde_json::json!(prev_conversation_id.to_string())
        );
    }

    #[tokio::test]
    async fn threads_page_exact_fit_has_no_next_or_prev() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let now = chrono::Utc::now();

        let mut thread = MockThreadService::new();
        thread.expect_list_threads().returning(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![rim_services::message_thread::dto::ThreadDto {
                        id: thread_id,
                        server_id,
                        conversation_id,
                        root_message_id: None,
                        root_author_id: None,
                        root_text: None,
                        created_at: now,
                        last_message_at: now,
                    }],
                    None,
                    None,
                ))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            thread,
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!) { threads(serverId: $sid, conversationId: $cid, dir: AFTER, limit: 1) { pageInfo { hasNext hasPrev } next { threadId } prev { threadId } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["threads"]["pageInfo"]["hasNext"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["threads"]["pageInfo"]["hasPrev"],
            serde_json::json!(false)
        );
        assert_eq!(json["threads"]["next"], serde_json::Value::Null);
        assert_eq!(json["threads"]["prev"], serde_json::Value::Null);
    }

    // Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
    #[test]
    fn _keep_ids_referenced() {
        let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
    }
}
