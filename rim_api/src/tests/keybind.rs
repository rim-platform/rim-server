use super::*;

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
