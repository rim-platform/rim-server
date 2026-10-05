use super::*;

#[tokio::test]
async fn execute_command_mutation_returns_result() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command.expect_execute_command().returning(|_cmd| {
        Box::pin(async {
            Ok(rim_services::command::dto::CommandResultDto {
                status: rim_services::command::dto::CommandResultStatusDto::Success,
                message: "ok".to_string(),
                payload: Some(serde_json::json!({ "message_id": "abc" })),
                follow_up_commands: vec![],
                pipeline_stages: None,
                confirmation_required: None,
            })
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation($args: JSON) { executeCommand(input: { commandId: "chat.post_message", args: $args, commandContext: { focusedSurface: "palette" } }) { status message } }"#,
        actor,
    )
    .variables(Variables::from_json(serde_json::json!({
        "args": { "server_id": "00000000-0000-0000-0000-000000000001" }
    })));

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

/// Round-trip test for Phase 1 Task 4 (Fury v2 §3.11, H4 part 1).
///
/// Verifies that the GraphQL `initiator` field on `ExecuteCommandInput`
/// is correctly translated to `CommandInitiator` on the
/// `ExecuteCommandCmd` passed to the service. The mock asserts on the
/// captured value — if the resolver dropped the field, defaulted it
/// silently, or mapped to the wrong variant, the assertion fires.
///
/// Covers all 8 wire variants. Phase 4 audit-log + per-turn cap will
/// read this field on the service side; this test pins the wire contract
/// so a future GraphQL refactor can't quietly break the audit trail.
#[tokio::test]
async fn execute_command_initiator_round_trips_through_graphql() {
    use rim_services::command::initiator::CommandInitiator;
    let cases: &[(&str, CommandInitiator)] = &[
        ("PALETTE", CommandInitiator::Palette),
        ("AGENT", CommandInitiator::Agent),
        ("MACRO", CommandInitiator::Macro),
        ("KEYBIND", CommandInitiator::Keybind),
        ("CONTEXT_MENU", CommandInitiator::ContextMenu),
        ("PLUGIN", CommandInitiator::Plugin),
        ("CLI", CommandInitiator::Cli),
        ("MCP", CommandInitiator::Mcp),
    ];

    for (wire, expected) in cases {
        let actor = UserId::new();
        let expected_initiator = *expected;
        let mut command = MockCommandService::new();
        command
            .expect_execute_command()
            .withf(move |cmd| cmd.initiator == expected_initiator)
            .times(1)
            .returning(|_cmd| {
                Box::pin(async {
                    Ok(rim_services::command::dto::CommandResultDto {
                        status: rim_services::command::dto::CommandResultStatusDto::Success,
                        message: "ok".to_string(),
                        payload: None,
                        follow_up_commands: vec![],
                        pipeline_stages: None,
                        confirmation_required: None,
                    })
                })
            });

        let schema = schema_with_command_mock(command);
        let query = format!(
            r#"mutation {{ executeCommand(input: {{ commandId: "noop", initiator: {wire} }}) {{ status }} }}"#,
        );
        let req = authed_request(&query, actor);
        let res = schema.execute(req).await;
        assert_no_errors(&res);
    }
}

/// Companion to the round-trip test above: when the GraphQL caller omits
/// `initiator`, the resolver MUST default to `Palette` (the conservative
/// human-attributed default per `CommandInitiator::default()`). Anything
/// else would silently re-tag legacy clients (frontend pre-Phase-4) as
/// agentic.
#[tokio::test]
async fn execute_command_omitted_initiator_defaults_to_palette() {
    use rim_services::command::initiator::CommandInitiator;
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command
        .expect_execute_command()
        .withf(|cmd| cmd.initiator == CommandInitiator::Palette)
        .times(1)
        .returning(|_cmd| {
            Box::pin(async {
                Ok(rim_services::command::dto::CommandResultDto {
                    status: rim_services::command::dto::CommandResultStatusDto::Success,
                    message: "ok".to_string(),
                    payload: None,
                    follow_up_commands: vec![],
                    pipeline_stages: None,
                    confirmation_required: None,
                })
            })
        });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation { executeCommand(input: { commandId: "noop" }) { status } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn create_user_command_mutation_returns_command() {
    let actor = UserId::new();
    let user_command_id = rim_core::command::model::UserCommandId::new();
    let mut command = MockCommandService::new();
    command.expect_create_user_command().returning(move |_cmd| {
        Box::pin(async move {
            Ok(rim_services::command::dto::UserCommandDto {
                id: user_command_id,
                command_key: "user.quick_group".to_string(),
                title: "Quick Group".to_string(),
                description: "desc".to_string(),
                steps: vec![],
                is_enabled: true,
            })
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation { createUserCommand(input: { commandId: "user.quick_group", title: "Quick Group", description: "desc", steps: [{ commandId: "chat.create_group" }] }) { id sourceKind } }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn update_user_command_mutation_returns_command() {
    let actor = UserId::new();
    let user_command_id = rim_core::command::model::UserCommandId::new();
    let mut command = MockCommandService::new();
    command.expect_update_user_command().returning(move |_cmd| {
        Box::pin(async move {
            Ok(rim_services::command::dto::UserCommandDto {
                id: user_command_id,
                command_key: "user.quick_group".to_string(),
                title: "Quick Group Updated".to_string(),
                description: "desc".to_string(),
                steps: vec![],
                is_enabled: false,
            })
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation { updateUserCommand(input: { commandId: "user.quick_group", title: "Quick Group Updated", description: "desc", isEnabled: false, steps: [{ commandId: "chat.create_group" }] }) { id } }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn delete_user_command_mutation_returns_true() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command
        .expect_delete_user_command()
        .returning(|_cmd| Box::pin(async { Ok(()) }));

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation { deleteUserCommand(input: { commandId: "user.quick_group" }) }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn execute_command_rate_limited_error_carries_extension_shape() {
    let actor = UserId::new();
    let mut command = MockCommandService::new();
    command.expect_execute_command().returning(|_cmd| {
        Box::pin(async {
            Err(ServiceError::RateLimited {
                retry_after_seconds: 42,
            })
        })
    });

    let schema = schema_with_command_mock(command);
    let req = authed_request(
        r#"mutation { executeCommand(input: { commandId: "chat.post_message" }) { status } }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert!(!res.errors.is_empty(), "expected a GQL error");
    let ext = res.errors[0]
        .extensions
        .as_ref()
        .expect("missing extensions");
    assert_eq!(ext.get("code"), Some(&Value::from("RATE_LIMITED")));
    assert_eq!(
        ext.get("retryAfterSeconds"),
        Some(&Value::from(42_i64)),
        "retryAfterSeconds must be present and match the service value"
    );
}
