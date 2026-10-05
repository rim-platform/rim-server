use super::*;

#[tokio::test]
async fn keybinds_query_returns_keybinds() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind
        .expect_list_keybinds()
        .returning(|_query| Box::pin(async { Ok(vec![test_keybind_dto()]) }));

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"query { keybinds(scope: USER) { keystroke commandId } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn resolved_keybinds_query_returns_keybinds() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind.expect_resolve_keybinds().returning(|_query| {
        Box::pin(async {
            let mut dto = test_keybind_dto();
            dto.scope = rim_services::keybind::dto::KeybindScopeDto::Workspace;
            dto.source = rim_services::keybind::dto::KeybindSourceDto::Workspace;
            dto.workspace_id = Some(rim_core::server::model::WorkspaceId::new());
            dto.command_id = "workspace.open_command_palette".to_string();
            Ok(vec![dto])
        })
    });

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"query { resolvedKeybinds { keystroke commandId scope } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn resolve_keybind_query_returns_single_binding() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind.expect_resolve_keybind().returning(|_query| {
        Box::pin(async {
            let mut dto = test_keybind_dto();
            dto.scope = rim_services::keybind::dto::KeybindScopeDto::Workspace;
            dto.source = rim_services::keybind::dto::KeybindSourceDto::Workspace;
            dto.workspace_id = Some(rim_core::server::model::WorkspaceId::new());
            dto.command_id = "workspace.open_command_palette".to_string();
            Ok(Some(dto))
        })
    });

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"query { resolveKeybind(keystroke: "Ctrl+R", context: GLOBAL) { commandId source } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn keybind_layers_query_returns_layered_view() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind.expect_inspect_keybind_layers().returning(|_query| {
        Box::pin(async {
            let mut ws_dto = test_keybind_dto();
            ws_dto.scope = rim_services::keybind::dto::KeybindScopeDto::Workspace;
            ws_dto.source = rim_services::keybind::dto::KeybindSourceDto::Workspace;
            ws_dto.workspace_id = Some(rim_core::server::model::WorkspaceId::new());
            ws_dto.command_id = "workspace.open_command_palette".to_string();

            Ok(rim_services::keybind::dto::KeybindLayersDto {
                default_keybind: Some(rim_services::keybind::dto::DefaultKeybindDto {
                    source: rim_services::keybind::dto::KeybindSourceDto::Default,
                    keystroke: "Ctrl+R".to_string(),
                    command_id: "workspace.cycle_recent_activity".to_string(),
                    context: rim_services::keybind::dto::KeybindContextDto::Global,
                    preset_args: None,
                }),
                user_keybind: None,
                workspace_keybind: Some(ws_dto.clone()),
                resolved_persisted_keybind: Some(ws_dto),
            })
        })
    });

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"query { keybindLayers(keystroke: "Ctrl+R", context: GLOBAL) { defaultKeybind { commandId } workspaceKeybind { commandId } resolvedPersistedKeybind { commandId } } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn default_keybinds_query_returns_defaults() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind.expect_list_default_keybinds().returning(|_query| {
        Box::pin(async {
            Ok(vec![rim_services::keybind::dto::DefaultKeybindDto {
                source: rim_services::keybind::dto::KeybindSourceDto::Default,
                keystroke: "Ctrl+K".to_string(),
                command_id: "workspace.open_command_palette".to_string(),
                context: rim_services::keybind::dto::KeybindContextDto::Global,
                preset_args: None,
            }])
        })
    });

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"query { defaultKeybinds { keystroke source commandId } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}

#[tokio::test]
async fn create_keybind_mutation_returns_keybind() {
    let actor = UserId::new();
    let mut keybind = MockKeybindService::new();
    keybind
        .expect_create_keybind()
        .returning(|_cmd| Box::pin(async { Ok(test_keybind_dto()) }));

    let schema = schema_with_keybind_mock(keybind);

    let req = authed_request(
        r#"mutation { createKeybind(input: { scope: USER, keystroke: "Ctrl+R", commandId: "workspace.cycle_recent_activity", context: GLOBAL }) { keystroke commandId } }"#,
        actor,
    );
    let res = schema.execute(req).await;
    assert_no_errors(&res);
}
