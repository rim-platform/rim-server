//! Instance feature gating: `serverInfo.features`, and `FEATURE_DISABLED` from the resolvers of
//! each disabled feature. The schemas use unprimed service mocks, so a resolver that reached
//! its service past the gate would panic.

use rim_core::server_feature::model::ServerFeature;

use super::*;

fn schema() -> crate::AppSchema {
    schema_with_user_mock(MockUserService::new())
}

/// The first error's `code` and `feature` extensions.
fn refusal(res: &async_graphql::Response) -> (Value, Value) {
    let err = res.errors.first().expect("an error");
    let ext = err.extensions.as_ref().expect("extensions");
    (
        ext.get("code").cloned().unwrap_or(Value::Null),
        ext.get("feature").cloned().unwrap_or(Value::Null),
    )
}

async fn assert_feature_disabled(query: &str, feature: ServerFeature) {
    let req = with_features_disabled(authed_request(query, UserId::new()), &[feature]);
    let res = schema().execute(req).await;
    assert_eq!(
        refusal(&res),
        (
            Value::from("FEATURE_DISABLED"),
            Value::from(feature.as_str())
        ),
        "{query}"
    );
}

#[tokio::test]
async fn server_info_lists_every_feature_with_its_state() {
    let res = schema()
        .execute(Request::new(
            "{ serverInfo { features { key state reason } } }",
        ))
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    let features = data["serverInfo"]["features"].as_array().expect("features");
    let keys: Vec<&str> = features
        .iter()
        .map(|f| f["key"].as_str().expect("key"))
        .collect();
    assert_eq!(
        keys,
        ServerFeature::ALL
            .iter()
            .map(ServerFeature::as_str)
            .collect::<Vec<_>>()
    );
    assert!(
        features
            .iter()
            .all(|f| f["state"] == "ENABLED" && f["reason"].is_null())
    );
}

#[tokio::test]
async fn server_info_reports_disabled_and_unavailable_features() {
    let gate: Arc<dyn rim_services::feature::service::FeatureGate> =
        Arc::new(rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &[ServerFeature::CustomEmoji],
            &[(ServerFeature::Email, "SMTP is not configured".to_string())],
        ));
    let res = schema()
        .execute(Request::new("{ serverInfo { features { key state reason } } }").data(gate))
        .await;
    assert_no_errors(&res);
    let data = res.data.into_json().expect("json");
    let feature = |key: &str| {
        data["serverInfo"]["features"]
            .as_array()
            .expect("features")
            .iter()
            .find(|f| f["key"] == key)
            .cloned()
            .expect(key)
    };
    assert_eq!(
        feature("custom_emoji"),
        serde_json::json!({ "key": "custom_emoji", "state": "DISABLED", "reason": null })
    );
    assert_eq!(
        feature("email"),
        serde_json::json!({ "key": "email", "state": "UNAVAILABLE", "reason": "SMTP is not configured" })
    );
    assert_eq!(feature("attachments")["state"], "ENABLED");
}

#[tokio::test]
async fn attachments_resolvers_answer_feature_disabled() {
    let server = ServerId::new();
    let id = uuid::Uuid::now_v7();
    assert_feature_disabled(
        &format!(r#"mutation {{ createAttachment(input: {{ serverId: "{server}", conversationId: "{id}", filename: "a.txt", contentType: "text/plain", sizeBytes: 5 }}) {{ attachmentId }} }}"#),
        ServerFeature::Attachments,
    )
    .await;
    assert_feature_disabled(
        &format!(r#"mutation {{ deleteAttachment(input: {{ serverId: "{server}", attachmentId: "{id}" }}) }}"#),
        ServerFeature::Attachments,
    )
    .await;
}

#[tokio::test]
async fn custom_emoji_resolvers_answer_feature_disabled() {
    let server = ServerId::new();
    assert_feature_disabled(
        &format!(r#"mutation {{ createCustomEmoji(input: {{ serverId: "{server}", name: "party", imageBase64: "AA==" }}) {{ id }} }}"#),
        ServerFeature::CustomEmoji,
    )
    .await;
    assert_feature_disabled(
        &format!(r#"{{ customEmojis(serverId: "{server}") {{ id }} }}"#),
        ServerFeature::CustomEmoji,
    )
    .await;
    assert_feature_disabled(
        &format!(
            r#"mutation {{ deleteCustomEmoji(emojiId: "{}") }}"#,
            uuid::Uuid::now_v7()
        ),
        ServerFeature::CustomEmoji,
    )
    .await;
}

#[tokio::test]
async fn workspace_plugin_resolvers_answer_feature_disabled() {
    let server = ServerId::new();
    assert_feature_disabled(
        &format!(r#"{{ workspacePlugins(serverId: "{server}") {{ pluginId }} }}"#),
        ServerFeature::WorkspacePlugins,
    )
    .await;
    assert_feature_disabled(
        &format!(r#"mutation {{ removeWorkspacePlugin(serverId: "{server}", pluginId: "p") }}"#),
        ServerFeature::WorkspacePlugins,
    )
    .await;
}

#[tokio::test]
async fn ai_command_resolvers_answer_feature_disabled() {
    assert_feature_disabled("{ commandDiscover { id } }", ServerFeature::AiCommands).await;
    assert_feature_disabled(
        "{ commands(surface: AI) { id } }",
        ServerFeature::AiCommands,
    )
    .await;
    assert_feature_disabled(
        r#"mutation { executeCommand(input: { commandId: "chat.post_message", initiator: AGENT }) { status } }"#,
        ServerFeature::AiCommands,
    )
    .await;
    assert_feature_disabled(
        &format!(r#"mutation {{ executeMacro(input: {{ macroId: "{}", initiator: AGENT }}) {{ status }} }}"#, uuid::Uuid::now_v7()),
        ServerFeature::AiCommands,
    )
    .await;
}

#[tokio::test]
async fn setting_an_avatar_answers_feature_disabled() {
    assert_feature_disabled(
        r#"mutation { updateMyProfile(input: { avatarUrl: "/api/files/avatars/u/avatar.png" }) { id } }"#,
        ServerFeature::Avatars,
    )
    .await;
    assert_feature_disabled(
        r#"mutation { updateMyProfile(input: { avatarUrl: "", statusText: "away" }) { id } }"#,
        ServerFeature::Avatars,
    )
    .await;
}

/// With avatars off, profile edits without an avatar still go through.
#[tokio::test]
async fn profile_edits_without_an_avatar_pass_while_avatars_are_off() {
    let mut user = MockUserService::new();
    user.expect_update_my_profile()
        .withf(|cmd| cmd.avatar_url.is_none())
        .times(1)
        .returning(|_| Box::pin(async { Ok(test_user_dto()) }));
    let res = schema_with_user_mock(user)
        .execute(with_features_disabled(
            authed_request(
                r#"mutation { updateMyProfile(input: { statusText: "away" }) { id } }"#,
                UserId::new(),
            ),
            &[ServerFeature::Avatars],
        ))
        .await;
    assert_no_errors(&res);
}

/// A stored avatar URL reads as null on users and members while avatars are off.
#[tokio::test]
async fn avatar_urls_read_null_while_avatars_are_off() {
    const URL: &str = "/api/files/avatars/u/avatar.png";
    let mut user = MockUserService::new();
    user.expect_get_me().returning(|_| {
        Box::pin(async {
            Ok(rim_services::user::dto::UserDto {
                avatar_url: Some(URL.to_string()),
                ..test_user_dto()
            })
        })
    });
    let schema = schema_with_user_mock(user);
    let query = "{ me { username avatarUrl } }";

    let res = schema.execute(authed_request(query, UserId::new())).await;
    assert_no_errors(&res);
    assert_eq!(res.data.into_json().expect("json")["me"]["avatarUrl"], URL);

    let res = schema
        .execute(with_features_disabled(
            authed_request(query, UserId::new()),
            &[ServerFeature::Avatars],
        ))
        .await;
    assert_no_errors(&res);
    let off = res.data.into_json().expect("json");
    assert_eq!(off["me"]["username"], "testuser");
    assert!(off["me"]["avatarUrl"].is_null());

    let mut members = MockServerMemberService::new();
    members.expect_list_members().returning(|_| {
        Box::pin(async {
            Ok(vec![rim_services::server_member::dto::ServerMemberDto {
                avatar_url: Some(URL.to_string()),
                ..test_server_member_dto()
            }])
        })
    });
    let schema = schema_with_server_mocks(MockServerService::new(), members);
    let query = format!(
        r#"{{ serverMembers(serverId: "{}") {{ username avatarUrl }} }}"#,
        ServerId::new()
    );
    let res = schema.execute(authed_request(&query, UserId::new())).await;
    assert_no_errors(&res);
    assert_eq!(
        res.data.into_json().expect("json")["serverMembers"][0]["avatarUrl"],
        URL
    );
    let res = schema
        .execute(with_features_disabled(
            authed_request(&query, UserId::new()),
            &[ServerFeature::Avatars],
        ))
        .await;
    assert_no_errors(&res);
    let off = res.data.into_json().expect("json");
    assert_eq!(off["serverMembers"][0]["username"], "testuser");
    assert!(off["serverMembers"][0]["avatarUrl"].is_null());
}

/// With the AI surface off, the palette still lists commands, without their agent-facing
/// metadata.
#[tokio::test]
async fn commands_lose_their_agent_metadata_while_the_ai_surface_is_off() {
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
                required_capabilities: vec![],
                visible_in_palette: true,
                visible_in_context_menus: false,
                keybind_capable: false,
                dangerous: false,
                user_command_id: None,
                nl_description: Some("Post a message".to_string()),
                intent_samples: vec!["say hi".to_string()],
                ai_callable: true,
                sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                typed_verb: rim_services::command::verb::TypedVerb::Other,
                rate_limit_per_minute: None,
            }])
        })
    });
    let schema = schema_with_command_mock(command);
    let query = "{ commands { id nlDescription intentSamples aiCallable } }";

    let res = schema.execute(authed_request(query, UserId::new())).await;
    assert_no_errors(&res);
    let on = res.data.into_json().expect("json");
    assert_eq!(on["commands"][0]["aiCallable"], true);
    assert_eq!(on["commands"][0]["nlDescription"], "Post a message");

    let res = schema
        .execute(with_features_disabled(
            authed_request(query, UserId::new()),
            &[ServerFeature::AiCommands],
        ))
        .await;
    assert_no_errors(&res);
    let off = res.data.into_json().expect("json");
    assert_eq!(off["commands"][0]["id"], "chat.post_message");
    assert_eq!(off["commands"][0]["aiCallable"], false);
    assert!(off["commands"][0]["nlDescription"].is_null());
    assert_eq!(off["commands"][0]["intentSamples"], serde_json::json!([]));
}
