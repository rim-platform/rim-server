use super::*;

#[tokio::test]
async fn platform_queries_return_definitions() {
    let actor = UserId::new();

    let mut action = MockActionService::new();
    action.expect_list_actions().returning(|_actor| {
        Box::pin(async {
            Ok(vec![rim_services::action::dto::ActionDto {
                id: "post_message".to_string(),
                title: "Post Message".to_string(),
                description: "desc".to_string(),
                capability_ids: vec!["chat.write".to_string()],
            }])
        })
    });

    let mut capability = MockCapabilityService::new();
    capability.expect_list_capabilities().returning(|_actor| {
        Ok(vec![rim_services::capability::dto::CapabilityDto {
            id: "chat.write".to_string(),
            title: "Write Chat".to_string(),
            description: "desc".to_string(),
        }])
    });

    let mut event = MockEventService::new();
    event.expect_list_events().returning(|_actor| {
        Box::pin(async {
            Ok(vec![rim_services::event::dto::EventDto {
                id: "message.created".to_string(),
                title: "Message Created".to_string(),
                description: "desc".to_string(),
            }])
        })
    });

    let mut integration = MockIntegrationService::new();
    integration.expect_list_integrations().returning(|_actor| {
        Box::pin(async {
            Ok(vec![rim_services::integration::dto::IntegrationDto {
                id: "core.messaging".to_string(),
                kind: rim_services::integration::dto::IntegrationKindDto::BuiltIn,
                title: "Core Messaging".to_string(),
                enabled: true,
                principal_id: None,
            }])
        })
    });

    let mut setting = MockSettingService::new();
    setting.expect_list_settings().returning(|_actor| {
        Box::pin(async {
            Ok(vec![rim_services::setting::dto::SettingDto {
                id: "user.notifications.enabled".to_string(),
                scope: rim_services::setting::dto::SettingScopeDto::User,
                title: "Notifications Enabled".to_string(),
                description: "desc".to_string(),
                value_type: "boolean".to_string(),
                is_secret: false,
            }])
        })
    });

    let schema = schema_with_platform_mocks(action, capability, event, integration, setting);

    let req = authed_request(
        r#"query { actions { id } capabilities { id } events { id } integrations { id kind } settings { id scope } }"#,
        actor,
    );

    let res = schema.execute(req).await;
    assert_no_errors(&res);

    let json = res.data.into_json().unwrap();
    assert_eq!(json["actions"][0]["id"], serde_json::json!("post_message"));
    assert_eq!(
        json["capabilities"][0]["id"],
        serde_json::json!("chat.write")
    );
    assert_eq!(
        json["events"][0]["id"],
        serde_json::json!("message.created")
    );
    assert_eq!(
        json["integrations"][0]["id"],
        serde_json::json!("core.messaging")
    );
    assert_eq!(
        json["settings"][0]["id"],
        serde_json::json!("user.notifications.enabled")
    );
}
