use super::*;

#[tokio::test]
async fn commands_query_returns_command_definitions() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command.expect_list_commands().returning(|_query| {
        Box::pin(async {
            Ok(vec![rim_services::command::dto::CommandDefinitionDto {
                id: "chat.post_message".to_string(),
                title: "Post Message".to_string(),
                description: "desc".to_string(),
                category: "Chat".to_string(),
                source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                execution_target: rim_services::command::dto::CommandExecutionTargetDto::Backend,
                arguments: vec![],
                required_context: vec![],
                required_capabilities: vec!["chat.write".to_string()],
                visible_in_palette: true,
                visible_in_context_menus: false,
                keybind_capable: false,
                dangerous: false,
                user_command_id: None,
                nl_description: None,
                intent_samples: vec![],
                ai_callable: false,
                sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                typed_verb: rim_services::command::verb::TypedVerb::Other,
                rate_limit_per_minute: None,
            }])
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"query { commands { id category executionTarget } }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    assert_eq!(
        json["commands"][0]["id"],
        serde_json::json!("chat.post_message")
    );
    assert_eq!(json["commands"][0]["category"], serde_json::json!("Chat"));
}

#[tokio::test]
async fn commands_query_accepts_surface_and_context() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command.expect_list_commands().returning(|query| {
        assert!(matches!(
            query.surface,
            rim_services::command::dto::CommandSurfaceDto::ContextMenu
        ));
        assert!(
            query
                .context
                .as_ref()
                .and_then(|ctx| ctx.selected_message_id)
                .is_some()
        );
        Box::pin(async { Ok(vec![]) })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"query($mid: ID!) { commands(surface: CONTEXT_MENU, commandContext: { selectedMessageId: $mid }) { id } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "mid": MessageId::new().to_string(),
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

/// **P3-A1 closure** — `Command.arguments` is the wire contract the
/// frontend AI tool-schema generator (Phase 3 §4.B) reads to emit
/// `input_schema`. Round-trip every per-arg field (incl. `defaultValue`
/// JSON scalar and `allowedValues` Vec<String>) so a regression that
/// drops the conversion silently surfaces here, not at the agent.
#[tokio::test]
async fn commands_query_returns_arguments_with_full_schema() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command.expect_list_commands().returning(|_query| {
        Box::pin(async {
            Ok(vec![rim_services::command::dto::CommandDefinitionDto {
                id: "state.set".to_string(),
                title: "Set State".to_string(),
                description: "desc".to_string(),
                category: "State".to_string(),
                source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                execution_target: rim_services::command::dto::CommandExecutionTargetDto::Backend,
                arguments: vec![
                    rim_services::command::dto::CommandArgumentDto {
                        name: "key".to_string(),
                        title: "Key".to_string(),
                        description: "State key.".to_string(),
                        argument_type: rim_services::command::dto::CommandArgumentTypeDto::String,
                        required: true,
                        default_value: None,
                        allowed_values: None,
                    },
                    rim_services::command::dto::CommandArgumentDto {
                        name: "scope".to_string(),
                        title: "Scope".to_string(),
                        description: "Server/user.".to_string(),
                        argument_type: rim_services::command::dto::CommandArgumentTypeDto::String,
                        required: false,
                        default_value: Some(serde_json::json!("server")),
                        allowed_values: Some(vec!["server".to_string(), "user".to_string()]),
                    },
                ],
                required_context: vec![],
                required_capabilities: vec![],
                visible_in_palette: true,
                visible_in_context_menus: false,
                keybind_capable: false,
                dangerous: false,
                user_command_id: None,
                nl_description: None,
                intent_samples: vec![],
                ai_callable: false,
                sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                typed_verb: rim_services::command::verb::TypedVerb::Other,
                rate_limit_per_minute: None,
            }])
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"query {
                commands {
                    id
                    arguments {
                        name
                        title
                        description
                        argumentType
                        required
                        defaultValue
                        allowedValues
                    }
                }
            }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let json = res.data.into_json().unwrap();
    let args = &json["commands"][0]["arguments"];
    assert_eq!(args.as_array().map(|a| a.len()), Some(2));

    // Required arg with no default / no allowed_values.
    assert_eq!(args[0]["name"], serde_json::json!("key"));
    assert_eq!(args[0]["argumentType"], serde_json::json!("STRING"));
    assert_eq!(args[0]["required"], serde_json::json!(true));
    assert_eq!(args[0]["defaultValue"], serde_json::Value::Null);
    assert_eq!(args[0]["allowedValues"], serde_json::Value::Null);

    // Optional arg with JSON default + enum-style allowed_values.
    assert_eq!(args[1]["name"], serde_json::json!("scope"));
    assert_eq!(args[1]["required"], serde_json::json!(false));
    // `Json` scalar serializes the inner JSON value verbatim — the
    // wire shape the frontend will consume.
    assert_eq!(args[1]["defaultValue"], serde_json::json!("server"));
    assert_eq!(
        args[1]["allowedValues"],
        serde_json::json!(["server", "user"])
    );
}

// ── P5-A7 command_discover resolver tests ──────────────────────────────

/// Full round-trip: GQL query → resolver → MockCommandService →
/// returns `[Command!]!` with expected shape.
#[tokio::test]
async fn command_discover_query_returns_ai_callable_commands() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command
        .expect_command_discover()
        .returning(|_actor, _phrase, _limit, _ws_id| {
            Box::pin(async {
                Ok(vec![
                    rim_services::command::dto::CommandDefinitionDto {
                        id: "chat.list_channels".to_string(),
                        title: "List Channels".to_string(),
                        description: "List all channels in the workspace".to_string(),
                        category: "Chat".to_string(),
                        source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                        execution_target:
                            rim_services::command::dto::CommandExecutionTargetDto::Backend,
                        arguments: vec![],
                        required_context: vec![],
                        required_capabilities: vec!["chat.read".to_string()],
                        visible_in_palette: true,
                        visible_in_context_menus: false,
                        keybind_capable: false,
                        dangerous: false,
                        user_command_id: None,
                        nl_description: Some(
                            "Lists all channels the user has access to".to_string(),
                        ),
                        intent_samples: vec!["show channels".to_string()],
                        ai_callable: true,
                        sensitivity:
                            rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                        typed_verb: rim_services::command::verb::TypedVerb::Other,
                        rate_limit_per_minute: None,
                    },
                    rim_services::command::dto::CommandDefinitionDto {
                        id: "chat.post_message".to_string(),
                        title: "Post Message".to_string(),
                        description: "Send a message to a channel".to_string(),
                        category: "Chat".to_string(),
                        source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                        execution_target:
                            rim_services::command::dto::CommandExecutionTargetDto::Backend,
                        arguments: vec![],
                        required_context: vec![],
                        required_capabilities: vec!["chat.write".to_string()],
                        visible_in_palette: true,
                        visible_in_context_menus: false,
                        keybind_capable: false,
                        dangerous: false,
                        user_command_id: None,
                        nl_description: Some("Sends a message to a channel".to_string()),
                        intent_samples: vec!["send message".to_string()],
                        ai_callable: true,
                        sensitivity:
                            rim_services::command::sensitivity::CommandSensitivityDto::Mutating,
                        typed_verb: rim_services::command::verb::TypedVerb::Other,
                        rate_limit_per_minute: None,
                    },
                ])
            })
        });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"query {
                commandDiscover {
                    id
                    title
                    aiCallable
                    requiredCapabilities
                    nlDescription
                }
            }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let data = res.data.into_json().unwrap();
    let items = data["commandDiscover"].as_array().unwrap();
    assert_eq!(items.len(), 2, "expected 2 AI-callable commands");
    assert_eq!(items[0]["id"], "chat.list_channels");
    assert_eq!(items[0]["aiCallable"], true);
    assert_eq!(items[1]["id"], "chat.post_message");
    assert_eq!(items[1]["aiCallable"], true);
    // nlDescription must be forwarded through (key field for the AI agent).
    assert_eq!(
        items[0]["nlDescription"],
        "Lists all channels the user has access to"
    );
}

/// Verify that `commandDiscover` requires authentication — same gate as all
/// other actor-gated queries. An unauthenticated call must return a GQL error.
#[tokio::test]
async fn command_discover_requires_auth() {
    let command = MockCommandService::new();
    let schema = schema_with_command_mock(command);

    let res = schema
        .execute(Request::new("{ commandDiscover { id } }"))
        .await;

    assert!(
        !res.errors.is_empty(),
        "unauthenticated commandDiscover must return an error"
    );
    assert!(
        res.errors[0].message.contains("unauthenticated"),
        "expected unauthenticated error, got: {:?}",
        res.errors[0].message
    );
}

/// Empty result is a valid response — actor with no AI-callable caps.
#[tokio::test]
async fn command_discover_returns_empty_list_for_actor_with_no_caps() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command
        .expect_command_discover()
        .returning(|_actor, _phrase, _limit, _ws_id| Box::pin(async { Ok(vec![]) }));

    let schema = schema_with_command_mock(command);
    let req = authed_request("{ commandDiscover { id } }", actor);

    let res = schema.execute(req).await;
    assert_no_errors(&res);
    let data = res.data.into_json().unwrap();
    let items = data["commandDiscover"].as_array().unwrap();
    assert!(
        items.is_empty(),
        "actor with no caps must get empty commandDiscover"
    );
}

/// Hawkeye gate: 6th `commandDiscover` call in the same 60-second window must
/// return a `RATE_LIMITED` GQL error. The mock returns `ServiceError::RateLimited`
/// on the 6th invocation; the resolver must surface it as a GQL error whose
/// message contains "rate" (case-insensitive).
#[tokio::test]
async fn command_discover_rate_limited_on_sixth_call() {
    use std::sync::Arc as StdArc;
    use std::sync::atomic::{AtomicU32, Ordering};

    let actor = UserId::new();
    let call_count = StdArc::new(AtomicU32::new(0));
    let call_count2 = call_count.clone();

    let mut command = MockCommandService::new();
    command
        .expect_command_discover()
        .returning(move |_actor, _phrase, _limit, _ws_id| {
            let count = call_count2.fetch_add(1, Ordering::SeqCst) + 1;
            Box::pin(async move {
                if count <= 5 {
                    Ok(vec![])
                } else {
                    Err(rim_services::ServiceError::RateLimited {
                        retry_after_seconds: 42,
                    })
                }
            })
        });

    let schema = schema_with_command_mock(command);
    let gql = "{ commandDiscover { id } }";

    // First 5 calls must succeed.
    for i in 1..=5 {
        let res = schema.execute(authed_request(gql, actor)).await;
        assert!(
            res.errors.is_empty(),
            "call {i} should succeed, got errors: {:?}",
            res.errors
        );
    }

    // 6th call must return an error.
    let res = schema.execute(authed_request(gql, actor)).await;
    assert!(
        !res.errors.is_empty(),
        "6th commandDiscover call must return a rate-limit error"
    );
    let msg = res.errors[0].message.to_lowercase();
    assert!(
        msg.contains("rate"),
        "expected rate-limit error message, got: {:?}",
        res.errors[0].message
    );
}
