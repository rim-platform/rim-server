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
    use rim_services::action::service::MockActionService;
    use rim_services::auth::service::{MockAuthService, MockRequestAuthenticator};
    use rim_services::capability::service::MockCapabilityService;
    use rim_services::command::service::MockCommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::event::service::MockEventService;
    use rim_services::integration::service::MockIntegrationService;
    use rim_services::keybind::service::MockKeybindService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_attachment::service::MockMessageAttachmentService;
    use rim_services::message_thread::service::MockThreadService;
    use rim_services::provider::AppServiceProvider;
    use rim_services::server::service::MockServerService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::setting::service::MockSettingService;
    use rim_services::user::service::MockUserService;

    use crate::auth_context::AuthActorCtx;
    use crate::build_schema;

    struct TestAppServiceProvider {
        action: Arc<dyn rim_services::action::service::ActionService>,
        auth: Arc<dyn rim_services::auth::service::AuthService>,
        capability: Arc<dyn rim_services::capability::service::CapabilityService>,
        command: Arc<dyn rim_services::command::service::CommandService>,
        conversation: Arc<dyn rim_services::conversation::service::ConversationService>,
        event: Arc<dyn rim_services::event::service::EventService>,
        integration: Arc<dyn rim_services::integration::service::IntegrationService>,
        keybind: Arc<dyn rim_services::keybind::service::KeybindService>,
        message: Arc<dyn rim_services::message::service::MessageService>,
        attachment: Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
        server: Arc<dyn rim_services::server::service::ServerService>,
        server_member: Arc<dyn rim_services::server_member::service::ServerMemberService>,
        setting: Arc<dyn rim_services::setting::service::SettingService>,
        thread: Arc<dyn rim_services::message_thread::service::ThreadService>,
        user: Arc<dyn rim_services::user::service::UserService>,
        request_auth: Arc<dyn rim_services::auth::service::RequestAuthenticator>,
    }

    impl AppServiceProvider for TestAppServiceProvider {
        fn action_service(&self) -> Arc<dyn rim_services::action::service::ActionService> {
            Arc::clone(&self.action)
        }

        fn auth_service(&self) -> Arc<dyn rim_services::auth::service::AuthService> {
            Arc::clone(&self.auth)
        }

        fn capability_service(
            &self,
        ) -> Arc<dyn rim_services::capability::service::CapabilityService> {
            Arc::clone(&self.capability)
        }

        fn command_service(&self) -> Arc<dyn rim_services::command::service::CommandService> {
            Arc::clone(&self.command)
        }

        fn conversation_service(
            &self,
        ) -> Arc<dyn rim_services::conversation::service::ConversationService> {
            Arc::clone(&self.conversation)
        }

        fn event_service(&self) -> Arc<dyn rim_services::event::service::EventService> {
            Arc::clone(&self.event)
        }

        fn integration_service(
            &self,
        ) -> Arc<dyn rim_services::integration::service::IntegrationService> {
            Arc::clone(&self.integration)
        }

        fn keybind_service(&self) -> Arc<dyn rim_services::keybind::service::KeybindService> {
            Arc::clone(&self.keybind)
        }

        fn message_service(&self) -> Arc<dyn rim_services::message::service::MessageService> {
            Arc::clone(&self.message)
        }

        fn message_attachment_service(
            &self,
        ) -> Arc<dyn rim_services::message_attachment::service::MessageAttachmentService> {
            Arc::clone(&self.attachment)
        }

        fn server_service(&self) -> Arc<dyn rim_services::server::service::ServerService> {
            Arc::clone(&self.server)
        }

        fn server_member_service(
            &self,
        ) -> Arc<dyn rim_services::server_member::service::ServerMemberService> {
            Arc::clone(&self.server_member)
        }

        fn setting_service(&self) -> Arc<dyn rim_services::setting::service::SettingService> {
            Arc::clone(&self.setting)
        }

        fn thread_service(&self) -> Arc<dyn rim_services::message_thread::service::ThreadService> {
            Arc::clone(&self.thread)
        }

        fn user_service(&self) -> Arc<dyn rim_services::user::service::UserService> {
            Arc::clone(&self.user)
        }

        fn request_authenticator(
            &self,
        ) -> Arc<dyn rim_services::auth::service::RequestAuthenticator> {
            Arc::clone(&self.request_auth)
        }
    }

    fn schema_with_mocks(
        auth: MockAuthService,
        conv: MockConversationService,
        msg: MockMessageService,
        attachment: MockMessageAttachmentService,
        thread: MockThreadService,
    ) -> crate::AppSchema {
        build_schema(Arc::new(TestAppServiceProvider {
            action: Arc::new(MockActionService::new()),
            auth: Arc::new(auth),
            capability: Arc::new(MockCapabilityService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(conv),
            event: Arc::new(MockEventService::new()),
            integration: Arc::new(MockIntegrationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            message: Arc::new(msg),
            attachment: Arc::new(attachment),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(MockSettingService::new()),
            thread: Arc::new(thread),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
        }))
    }

    fn schema_with_platform_mocks(
        action: MockActionService,
        capability: MockCapabilityService,
        event: MockEventService,
        integration: MockIntegrationService,
        setting: MockSettingService,
    ) -> crate::AppSchema {
        build_schema(Arc::new(TestAppServiceProvider {
            action: Arc::new(action),
            auth: Arc::new(MockAuthService::new()),
            capability: Arc::new(capability),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            event: Arc::new(event),
            integration: Arc::new(integration),
            keybind: Arc::new(MockKeybindService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(setting),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
        }))
    }

    fn schema_with_command_mock(command: MockCommandService) -> crate::AppSchema {
        build_schema(Arc::new(TestAppServiceProvider {
            action: Arc::new(MockActionService::new()),
            auth: Arc::new(MockAuthService::new()),
            capability: Arc::new(MockCapabilityService::new()),
            command: Arc::new(command),
            conversation: Arc::new(MockConversationService::new()),
            event: Arc::new(MockEventService::new()),
            integration: Arc::new(MockIntegrationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(MockSettingService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
        }))
    }

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
            Box::pin(async {
                Ok(vec![rim_services::capability::dto::CapabilityDto {
                    id: "chat.write".to_string(),
                    title: "Write Chat".to_string(),
                    description: "desc".to_string(),
                }])
            })
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

        let req = Request::new(
            r#"query { actions { id } capabilities { id } events { id } integrations { id kind } settings { id scope } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

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
                }])
            })
        });

        let schema = schema_with_command_mock(command);
        let req = Request::new(r#"query { commands { id category executionTarget } }"#)
            .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
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
        let req = Request::new(
            r#"query($mid: ID!) { commands(surface: CONTEXT_MENU, commandContext: { selectedMessageId: $mid }) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "mid": MessageId::new().to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn keybinds_query_returns_keybinds() {
        let actor = UserId::new();
        let mut keybind = MockKeybindService::new();
        keybind.expect_list_keybinds().returning(|_query| {
            Box::pin(async {
                Ok(vec![rim_services::keybind::dto::KeybindDto {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: rim_services::keybind::dto::KeybindScopeDto::User,
                    workspace_id: None,
                    keystroke: "Ctrl+R".to_string(),
                    command_id: "workspace.cycle_recent_activity".to_string(),
                    context: rim_services::keybind::dto::KeybindContextDto::Global,
                    preset_args: None,
                    is_enabled: true,
                }])
            })
        });

        let schema = build_schema(Arc::new(TestAppServiceProvider {
            action: Arc::new(MockActionService::new()),
            auth: Arc::new(MockAuthService::new()),
            capability: Arc::new(MockCapabilityService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            event: Arc::new(MockEventService::new()),
            integration: Arc::new(MockIntegrationService::new()),
            keybind: Arc::new(keybind),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(MockSettingService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
        }));

        let req = Request::new(r#"query { keybinds(scope: USER) { keystroke commandId } }"#)
            .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));
        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn create_keybind_mutation_returns_keybind() {
        let actor = UserId::new();
        let mut keybind = MockKeybindService::new();
        keybind.expect_create_keybind().returning(|_cmd| {
            Box::pin(async {
                Ok(rim_services::keybind::dto::KeybindDto {
                    id: rim_core::keybind::model::KeybindId::new(),
                    scope: rim_services::keybind::dto::KeybindScopeDto::User,
                    workspace_id: None,
                    keystroke: "Ctrl+R".to_string(),
                    command_id: "workspace.cycle_recent_activity".to_string(),
                    context: rim_services::keybind::dto::KeybindContextDto::Global,
                    preset_args: None,
                    is_enabled: true,
                })
            })
        });

        let schema = build_schema(Arc::new(TestAppServiceProvider {
            action: Arc::new(MockActionService::new()),
            auth: Arc::new(MockAuthService::new()),
            capability: Arc::new(MockCapabilityService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            event: Arc::new(MockEventService::new()),
            integration: Arc::new(MockIntegrationService::new()),
            keybind: Arc::new(keybind),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(MockSettingService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
        }));

        let req = Request::new(
            r#"mutation { createKeybind(input: { scope: USER, keystroke: "Ctrl+R", commandId: "workspace.cycle_recent_activity", context: GLOBAL }) { keystroke commandId } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));
        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

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
                })
            })
        });

        let schema = schema_with_command_mock(command);
        let req = Request::new(
            r#"mutation($args: JSON) { executeCommand(input: { commandId: "chat.post_message", args: $args, commandContext: { focusedSurface: "palette" } }) { status message } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "args": { "server_id": "00000000-0000-0000-0000-000000000001" }
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
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
        let req = Request::new(
            r#"mutation { createUserCommand(input: { commandId: "user.quick_group", title: "Quick Group", description: "desc", steps: [{ commandId: "chat.create_group" }] }) { id sourceKind } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
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
        let req = Request::new(
            r#"mutation { updateUserCommand(input: { commandId: "user.quick_group", title: "Quick Group Updated", description: "desc", isEnabled: false, steps: [{ commandId: "chat.create_group" }] }) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn delete_user_command_mutation_returns_true() {
        let actor = UserId::new();
        let mut command = MockCommandService::new();
        command
            .expect_delete_user_command()
            .returning(|_cmd| Box::pin(async { Ok(()) }));

        let schema = schema_with_command_mock(command);
        let req = Request::new(
            r#"mutation { deleteUserCommand(input: { commandId: "user.quick_group" }) }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
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
            MockMessageAttachmentService::new(),
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
            MockMessageAttachmentService::new(),
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
                    attachments: vec![],
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
            MockMessageAttachmentService::new(),
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
    async fn message_query_exposes_attachments() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();
        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

        let mut msg = MockMessageService::new();
        msg.expect_get_message().returning(move |_q| {
            Box::pin(async move {
                Ok(rim_services::message::dto::MessageDto {
                    id: message_id,
                    server_id,
                    conversation_id,
                    thread_id: None,
                    root_thread_id: None,
                    author_id: actor,
                    text: Some("root".to_string()),
                    has_attachments: true,
                    attachments: vec![rim_services::message_attachment::dto::AttachmentDto {
                        id: attachment_id,
                        status: rim_services::message_attachment::dto::AttachmentStatus::Ready,
                        filename: "file.txt".to_string(),
                        content_type: "text/plain".to_string(),
                        size_bytes: 12,
                    }],
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
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id hasAttachments attachments { id filename sizeBytes } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["message"]["hasAttachments"], serde_json::json!(true));
        assert_eq!(
            json["message"]["attachments"][0]["id"],
            serde_json::json!(attachment_id.to_string())
        );
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
            MockMessageAttachmentService::new(),
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
            MockMessageAttachmentService::new(),
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
            MockMessageAttachmentService::new(),
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
                    attachments: vec![],
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
            MockMessageAttachmentService::new(),
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
    async fn mutation_create_conversation_keeps_title_for_channel() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();

        let mut conv = MockConversationService::new();
        conv.expect_create_conversation().returning(move |cmd| {
            assert_eq!(cmd.title.as_deref(), Some("Launches"));
            Box::pin(async move {
                Ok(rim_services::conversation::dto::ConversationDto {
                    id: conversation_id,
                    server_id,
                    conversation_type: ConversationType::Channel,
                    title: Some("Launches".to_string()),
                    member_ids: vec![actor],
                })
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"mutation($sid: ID!) { createConversation(input: { serverId: $sid, isChannel: true, title: "Launches" }) { id title conversationType } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["createConversation"]["title"],
            serde_json::json!("Launches")
        );
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
                        attachments: vec![],
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
            MockMessageAttachmentService::new(),
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
            MockMessageAttachmentService::new(),
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
    async fn channels_page_exact_fit_has_no_next_or_prev() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();

        let mut conv = MockConversationService::new();
        conv.expect_list_channels().returning(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![rim_services::conversation::dto::ConversationDto {
                        id: conversation_id,
                        server_id,
                        conversation_type: ConversationType::Channel,
                        title: None,
                        member_ids: vec![actor],
                    }],
                    None,
                    None,
                ))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!) { channels(serverId: $sid, dir: AFTER, limit: 1) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["channels"]["pageInfo"]["hasNext"],
            serde_json::json!(false)
        );
        assert_eq!(
            json["channels"]["pageInfo"]["hasPrev"],
            serde_json::json!(false)
        );
        assert_eq!(json["channels"]["next"], serde_json::Value::Null);
        assert_eq!(json["channels"]["prev"], serde_json::Value::Null);
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
            MockMessageAttachmentService::new(),
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

    #[tokio::test]
    async fn attachment_query_calls_service_when_authed() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

        let mut attachment = MockMessageAttachmentService::new();
        attachment.expect_get_attachment().returning(move |_q| {
            Box::pin(async move {
                Ok(rim_services::message_attachment::dto::AttachmentDto {
                    id: attachment_id,
                    status: rim_services::message_attachment::dto::AttachmentStatus::Ready,
                    filename: "f.txt".to_string(),
                    content_type: "text/plain".to_string(),
                    size_bytes: 42,
                })
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $aid: ID!) { attachment(serverId: $sid, attachmentId: $aid) { id status filename sizeBytes } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn mutation_create_attachment_returns_upload_ticket() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

        let mut attachment = MockMessageAttachmentService::new();
        attachment
            .expect_create_attachment()
            .returning(move |_cmd| {
                let attachment_id = attachment_id;
                Box::pin(async move {
                    Ok(rim_services::message_attachment::dto::UploadTicketDto {
                        attachment_id,
                        upload_token: "upload-token".to_string(),
                        max_size_bytes: Some(42),
                        expires_at: None,
                    })
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { createAttachment(input: { serverId: $sid, conversationId: $cid, messageId: $mid, filename: "f.txt", contentType: "text/plain", sizeBytes: 42 }) { attachmentId uploadToken maxSizeBytes } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": MessageId::new().to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["createAttachment"]["attachmentId"],
            serde_json::json!(attachment_id.to_string())
        );
    }

    #[tokio::test]
    async fn mutation_finalize_attachment_returns_attachment() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

        let mut attachment = MockMessageAttachmentService::new();
        attachment
            .expect_finalize_attachment()
            .returning(move |_cmd| {
                let attachment_id = attachment_id;
                Box::pin(async move {
                    Ok(rim_services::message_attachment::dto::AttachmentDto {
                        id: attachment_id,
                        status: rim_services::message_attachment::dto::AttachmentStatus::Ready,
                        filename: "f.txt".to_string(),
                        content_type: "text/plain".to_string(),
                        size_bytes: 42,
                    })
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"mutation($sid: ID!, $aid: ID!) { finalizeAttachment(input: { serverId: $sid, attachmentId: $aid }) { id } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn mutation_delete_attachment_returns_true() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let attachment_id = rim_core::message_attachment::model::MessageAttachmentId::new();

        let mut attachment = MockMessageAttachmentService::new();
        attachment
            .expect_delete_attachment()
            .returning(|_cmd| Box::pin(async { Ok(()) }));

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"mutation($sid: ID!, $aid: ID!) { deleteAttachment(input: { serverId: $sid, attachmentId: $aid }) }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);
    }

    #[tokio::test]
    async fn thread_messages_page_exposes_both_directions() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let thread_id = MessageThreadId::new();
        let message_id = MessageId::new();
        let next_message_id = MessageId::new();
        let prev_message_id = MessageId::new();
        let sent_at = chrono::Utc::now();

        let mut msg = MockMessageService::new();
        msg.expect_list_thread_messages().returning(move |_q| {
            Box::pin(async move {
                Ok(Page::new(
                    vec![rim_services::message::dto::MessageDto {
                        id: message_id,
                        server_id,
                        conversation_id,
                        thread_id: Some(thread_id),
                        root_thread_id: None,
                        author_id: actor,
                        text: Some("reply".to_string()),
                        has_attachments: false,
                        attachments: vec![],
                        parent_message_id: None,
                        is_deleted: false,
                        sent_at,
                    }],
                    Some(rim_services::message::dto::MessagePageCursor {
                        sent_at,
                        message_id: next_message_id,
                    }),
                    Some(rim_services::message::dto::MessagePageCursor {
                        sent_at,
                        message_id: prev_message_id,
                    }),
                ))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = Request::new(
            r#"query($sid: ID!, $cid: ID!, $tid: ID!) { threadMessages(serverId: $sid, conversationId: $cid, threadId: $tid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
        )
        .data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "tid": thread_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(res.errors.is_empty(), "errors: {:?}", res.errors);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["threadMessages"]["pageInfo"]["hasNext"],
            serde_json::json!(true)
        );
        assert_eq!(
            json["threadMessages"]["pageInfo"]["hasPrev"],
            serde_json::json!(true)
        );
        assert_eq!(
            json["threadMessages"]["next"]["messageId"],
            serde_json::json!(next_message_id.to_string())
        );
        assert_eq!(
            json["threadMessages"]["prev"]["messageId"],
            serde_json::json!(prev_message_id.to_string())
        );
    }

    // Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
    #[test]
    fn _keep_ids_referenced() {
        let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
    }
}
