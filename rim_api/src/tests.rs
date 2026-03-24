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
    use rim_services::message_reaction::service::MockMessageReactionService;
    use rim_services::message_thread::service::MockThreadService;
    use rim_services::provider::AppServiceProvider;
    use rim_services::server::service::MockServerService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::setting::service::MockSettingService;
    use rim_services::user::service::MockUserService;

    use crate::auth_context::AuthActorCtx;
    use crate::broadcast::InMemoryBroadcaster;
    use crate::build_schema;

    // ── Test helpers ──────────────────────────────────────────────────────

    fn authed_request(query: &str, actor: UserId) -> Request {
        Request::new(query).data(AuthActorCtx::new(Some(actor), Some(SessionId::new())))
    }

    fn assert_no_errors(res: &async_graphql::Response) {
        assert!(res.errors.is_empty(), "unexpected errors: {:?}", res.errors);
    }

    fn schema_with_user_mock(user: MockUserService) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(user),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    fn schema_with_server_mocks(
        server: MockServerService,
        server_member: MockServerMemberService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                server: Arc::new(server),
                server_member: Arc::new(server_member),
                setting: Arc::new(MockSettingService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    fn schema_with_keybind_mock(keybind: MockKeybindService) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
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
                reaction: Arc::new(MockMessageReactionService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    // ── DTO factory helpers ───────────────────────────────────────────────

    fn test_conversation_dto() -> rim_services::conversation::dto::ConversationDto {
        rim_services::conversation::dto::ConversationDto {
            id: ConversationId::new(),
            server_id: ServerId::new(),
            conversation_type: ConversationType::Group,
            title: None,
            member_ids: vec![],
        }
    }

    fn test_message_dto() -> rim_services::message::dto::MessageDto {
        rim_services::message::dto::MessageDto {
            id: MessageId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            thread_id: None,
            root_thread_id: None,
            author_id: UserId::new(),
            text: Some("test message".to_string()),
            has_attachments: false,
            attachments: vec![],
            parent_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
        }
    }

    fn test_thread_dto() -> rim_services::message_thread::dto::ThreadDto {
        rim_services::message_thread::dto::ThreadDto {
            id: MessageThreadId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            root_message_id: None,
            root_author_id: None,
            root_text: None,
            created_at: chrono::Utc::now(),
            last_message_at: chrono::Utc::now(),
        }
    }

    fn test_keybind_dto() -> rim_services::keybind::dto::KeybindDto {
        rim_services::keybind::dto::KeybindDto {
            id: rim_core::keybind::model::KeybindId::new(),
            scope: rim_services::keybind::dto::KeybindScopeDto::User,
            source: rim_services::keybind::dto::KeybindSourceDto::User,
            workspace_id: None,
            keystroke: "Ctrl+R".to_string(),
            command_id: "workspace.cycle_recent_activity".to_string(),
            context: rim_services::keybind::dto::KeybindContextDto::Global,
            preset_args: None,
            is_enabled: true,
        }
    }

    fn test_user_dto() -> rim_services::user::dto::UserDto {
        rim_services::user::dto::UserDto {
            id: UserId::new(),
            username: "testuser".to_string(),
            display_name: Some("Test User".to_string()),
            avatar_url: None,
            status_text: None,
        }
    }

    fn test_server_dto() -> rim_services::server::dto::ServerDto {
        rim_services::server::dto::ServerDto {
            id: ServerId::new(),
            name: "Test Server".to_string(),
        }
    }

    fn test_server_member_dto() -> rim_services::server_member::dto::ServerMemberDto {
        rim_services::server_member::dto::ServerMemberDto {
            server_id: ServerId::new(),
            user_id: UserId::new(),
            username: "testuser".to_string(),
            member_role: "member".to_string(),
            joined_at: chrono::Utc::now(),
        }
    }

    fn test_attachment_dto() -> rim_services::message_attachment::dto::AttachmentDto {
        rim_services::message_attachment::dto::AttachmentDto {
            id: rim_core::message_attachment::model::MessageAttachmentId::new(),
            status: rim_services::message_attachment::dto::AttachmentStatus::Ready,
            filename: "file.txt".to_string(),
            content_type: "text/plain".to_string(),
            size_bytes: 42,
        }
    }

    // ── Provider / schema builders ────────────────────────────────────────

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
        reaction: Arc<dyn rim_services::message_reaction::service::MessageReactionService>,
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

        fn message_reaction_service(
            &self,
        ) -> Arc<dyn rim_services::message_reaction::service::MessageReactionService> {
            Arc::clone(&self.reaction)
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
        let mut reaction = MockMessageReactionService::new();
        reaction
            .expect_get_reactions_for_messages()
            .returning(|_| Box::pin(async { Ok(std::collections::HashMap::new()) }));
        build_schema(
            Arc::new(TestAppServiceProvider {
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
                reaction: Arc::new(reaction),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                thread: Arc::new(thread),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    fn schema_with_platform_mocks(
        action: MockActionService,
        capability: MockCapabilityService,
        event: MockEventService,
        integration: MockIntegrationService,
        setting: MockSettingService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
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
                reaction: Arc::new(MockMessageReactionService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(setting),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    fn schema_with_command_mock(command: MockCommandService) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
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
                reaction: Arc::new(MockMessageReactionService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
            }),
            InMemoryBroadcaster::new(16),
        )
    }

    // ── Tests ─────────────────────────────────────────────────────────────

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
    async fn command_query_returns_single_command_definition() {
        let actor = UserId::new();
        let mut command = MockCommandService::new();
        command.expect_get_command().returning(|_query| {
            Box::pin(async {
                Ok(Some(rim_services::command::dto::CommandDefinitionDto {
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
                }))
            })
        });

        let schema = schema_with_command_mock(command);
        let req = authed_request(
            r#"query { command(commandId: "chat.post_message") { id category } }"#,
            actor,
        );

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) { conversation(serverId: $sid, conversationId: $cid) { id } }"#,
            actor,
        )
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
    async fn service_error_unauthenticated_maps_to_graphql_code() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();

        let mut conv = MockConversationService::new();
        conv.expect_get_conversation()
            .returning(|_q| Box::pin(async { Err(ServiceError::Unauthenticated) }));

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) { conversation(serverId: $sid, conversationId: $cid) { id } }"#,
            actor,
        )
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
            Some(&Value::from("UNAUTHENTICATED"))
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
        auth.expect_login().returning(move |_cmd| {
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
        assert_no_errors(&res);

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
        auth.expect_logout().returning(move |cmd| {
            assert_eq!(cmd.session_id, session_id);
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
                authed_request(
                    r#"query { message(serverId: "bad", conversationId: "bad", messageId: "bad") { id } }"#,
                    actor,
                ),
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
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.root_thread_id = Some(root_thread_id);
                dto.author_id = actor;
                dto.text = Some("root".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id rootThreadId } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut att = test_attachment_dto();
                att.id = attachment_id;

                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.author_id = actor;
                dto.text = Some("root".to_string());
                dto.has_attachments = true;
                dto.attachments = vec![att];
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { message(serverId: $sid, conversationId: $cid, messageId: $mid) { id hasAttachments attachments { id filename sizeBytes } } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["message"]["hasAttachments"], serde_json::json!(true));
        assert_eq!(
            json["message"]["attachments"][0]["id"],
            serde_json::json!(attachment_id.to_string())
        );
    }

    #[tokio::test]
    async fn message_reference_query_returns_permalink_data() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_get_message_reference().returning(move |_q| {
            Box::pin(async move {
                Ok(rim_services::message::dto::MessageReferenceDto {
                    server_id,
                    conversation_id,
                    message_id,
                    thread_id: None,
                    permalink_path: format!(
                        "/workspaces/{}/conversations/{}/messages/{}",
                        server_id, conversation_id, message_id
                    ),
                    anchor: format!("message-{message_id}"),
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

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { messageReference(serverId: $sid, conversationId: $cid, messageId: $mid) { messageId permalinkPath anchor } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
    }

    #[tokio::test]
    async fn search_messages_query_returns_matches() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_search_messages().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.author_id = actor;
                dto.text = Some("deploy failed".to_string());
                Ok(vec![dto])
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) { searchMessages(serverId: $sid, conversationId: $cid, query: "deploy", limit: 10, includeDeleted: false) { id text } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut dto = test_thread_dto();
                dto.id = thread_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            thread,
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $tid: ID!) { thread(serverId: $sid, conversationId: $cid, threadId: $tid) { id } }"#,
            actor,
        )
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
                    let mut dto = test_thread_dto();
                    dto.id = thread_id;
                    dto.server_id = server_id;
                    dto.conversation_id = conversation_id;
                    dto.root_message_id = Some(root_message_id);
                    dto.root_author_id = Some(actor);
                    dto.root_text = Some("root".to_string());
                    Ok(Some(dto))
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            thread,
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $mid: ID!) { threadByRootMessage(serverId: $sid, conversationId: $cid, rootMessageId: $mid) { id } }"#,
            actor,
        )
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
                let mut dto = test_thread_dto();
                dto.id = thread_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.root_message_id = Some(message_id);
                dto.root_author_id = Some(actor);
                dto.root_text = Some("root".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            thread,
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { createThread(input: { serverId: $sid, conversationId: $cid, rootMessageId: $mid }) { id rootMessageId } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.thread_id = Some(thread_id);
                dto.author_id = actor;
                dto.text = Some("reply".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $tid: ID!) { sendThreadMessage(input: { serverId: $sid, conversationId: $cid, threadId: $tid, text: "reply" }) { id threadId } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "tid": thread_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut dto = test_conversation_dto();
                dto.id = conversation_id;
                dto.server_id = server_id;
                dto.conversation_type = ConversationType::Channel;
                dto.title = Some("Launches".to_string());
                dto.member_ids = vec![actor];
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!) { createConversation(input: { serverId: $sid, isChannel: true, title: "Launches" }) { id title conversationType } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.author_id = actor;
                dto.text = Some("hello".to_string());
                dto.sent_at = sent_at;
                Ok(Page::new(
                    vec![dto],
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

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) { messages(serverId: $sid, conversationId: $cid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
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
                let mut dto = test_conversation_dto();
                dto.id = conversation_id;
                dto.server_id = server_id;
                dto.conversation_type = ConversationType::Direct;
                dto.member_ids = vec![actor];
                Ok(Page::new(
                    vec![dto],
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

        let req = authed_request(
            r#"query($sid: ID!) { inboxConversations(serverId: $sid, dir: BEFORE, limit: 20) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

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
                let mut dto = test_conversation_dto();
                dto.id = conversation_id;
                dto.server_id = server_id;
                dto.conversation_type = ConversationType::Channel;
                dto.member_ids = vec![actor];
                Ok(Page::new(vec![dto], None, None))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!) { channels(serverId: $sid, dir: AFTER, limit: 1) { pageInfo { hasNext hasPrev } next { conversationId } prev { conversationId } } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

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
                let mut dto = test_thread_dto();
                dto.id = thread_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.created_at = now;
                dto.last_message_at = now;
                Ok(Page::new(vec![dto], None, None))
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            MockMessageAttachmentService::new(),
            thread,
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) { threads(serverId: $sid, conversationId: $cid, dir: AFTER, limit: 1) { pageInfo { hasNext hasPrev } next { threadId } prev { threadId } } }"#,
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
                let mut dto = test_attachment_dto();
                dto.id = attachment_id;
                dto.filename = "f.txt".to_string();
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $aid: ID!) { attachment(serverId: $sid, attachmentId: $aid) { id status filename sizeBytes } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { createAttachment(input: { serverId: $sid, conversationId: $cid, messageId: $mid, filename: "f.txt", contentType: "text/plain", sizeBytes: 42 }) { attachmentId uploadToken maxSizeBytes } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": MessageId::new().to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                    let mut dto = test_attachment_dto();
                    dto.id = attachment_id;
                    dto.filename = "f.txt".to_string();
                    Ok(dto)
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            attachment,
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $aid: ID!) { finalizeAttachment(input: { serverId: $sid, attachmentId: $aid }) { id } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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

        let req = authed_request(
            r#"mutation($sid: ID!, $aid: ID!) { deleteAttachment(input: { serverId: $sid, attachmentId: $aid }) }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "aid": attachment_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
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
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.thread_id = Some(thread_id);
                dto.author_id = actor;
                dto.text = Some("reply".to_string());
                dto.sent_at = sent_at;
                Ok(Page::new(
                    vec![dto],
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

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!, $tid: ID!) { threadMessages(serverId: $sid, conversationId: $cid, threadId: $tid, dir: AFTER, limit: 20, includeDeleted: false) { pageInfo { hasNext hasPrev } next { messageId } prev { messageId } } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "tid": thread_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

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

    // ── me query ───────────────────────────────────────────────────────────

    #[tokio::test]
    async fn me_query_returns_current_user() {
        let actor = UserId::new();

        let mut user = MockUserService::new();
        user.expect_get_me().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_user_dto();
                dto.id = actor;
                dto.username = "alice".to_string();
                dto.display_name = Some("Alice".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_user_mock(user);

        let req = authed_request(r#"query { me { id username displayName } }"#, actor);
        let res = schema.execute(req).await;
        assert_no_errors(&res);

        let json = res.data.into_json().unwrap();
        assert_eq!(json["me"]["id"], serde_json::json!(actor.to_string()));
        assert_eq!(json["me"]["username"], serde_json::json!("alice"));
        assert_eq!(json["me"]["displayName"], serde_json::json!("Alice"));
    }

    #[tokio::test]
    async fn me_query_requires_auth() {
        let schema = schema_with_user_mock(MockUserService::new());

        let res = schema.execute(Request::new(r#"query { me { id } }"#)).await;

        assert!(!res.errors.is_empty());
        assert!(
            res.errors[0].message.contains("unauthenticated"),
            "got: {:?}",
            res.errors[0].message
        );
    }

    // ── server query ─────────────────────────────────────────────────────

    #[tokio::test]
    async fn server_query_returns_server() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let mut server = MockServerService::new();
        server.expect_get_server().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_server_dto();
                dto.id = server_id;
                dto.name = "My Server".to_string();
                Ok(dto)
            })
        });

        let schema = schema_with_server_mocks(server, MockServerMemberService::new());

        let req = authed_request(
            r#"query($sid: ID!) { server(serverId: $sid) { id name } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["server"]["id"],
            serde_json::json!(server_id.to_string())
        );
        assert_eq!(json["server"]["name"], serde_json::json!("My Server"));
    }

    // ── serverMembers query ──────────────────────────────────────────────

    #[tokio::test]
    async fn server_members_query_returns_members() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let member_user_id = UserId::new();
        let joined_at = chrono::Utc::now();

        let mut server_member = MockServerMemberService::new();
        server_member.expect_list_members().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_server_member_dto();
                dto.server_id = server_id;
                dto.user_id = member_user_id;
                dto.member_role = "admin".to_string();
                dto.joined_at = joined_at;
                Ok(vec![dto])
            })
        });

        let schema = schema_with_server_mocks(MockServerService::new(), server_member);

        let req = authed_request(
            r#"query($sid: ID!) { serverMembers(serverId: $sid) { userId memberRole joinedAt } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["serverMembers"][0]["userId"],
            serde_json::json!(member_user_id.to_string())
        );
        assert_eq!(
            json["serverMembers"][0]["memberRole"],
            serde_json::json!("admin")
        );
        assert!(json["serverMembers"][0]["joinedAt"].is_string());
    }

    // ── sendMessage mutation ─────────────────────────────────────────────

    #[tokio::test]
    async fn send_message_mutation_creates_message() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_send_message().returning(move |_cmd| {
            Box::pin(async move {
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.author_id = actor;
                dto.text = Some("hello world".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!) { sendMessage(input: { serverId: $sid, conversationId: $cid, text: "hello world" }) { id text authorId } }"#,
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
            json["sendMessage"]["id"],
            serde_json::json!(message_id.to_string())
        );
        assert_eq!(
            json["sendMessage"]["text"],
            serde_json::json!("hello world")
        );
        assert_eq!(
            json["sendMessage"]["authorId"],
            serde_json::json!(actor.to_string())
        );
    }

    // ── editMessage mutation ─────────────────────────────────────────────

    #[tokio::test]
    async fn edit_message_mutation_updates_text() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_edit_message().returning(move |_cmd| {
            Box::pin(async move {
                let mut dto = test_message_dto();
                dto.id = message_id;
                dto.server_id = server_id;
                dto.conversation_id = conversation_id;
                dto.author_id = actor;
                dto.text = Some("updated text".to_string());
                Ok(dto)
            })
        });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { editMessage(input: { serverId: $sid, conversationId: $cid, messageId: $mid, text: "updated text" }) { id text } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["editMessage"]["id"],
            serde_json::json!(message_id.to_string())
        );
        assert_eq!(
            json["editMessage"]["text"],
            serde_json::json!("updated text")
        );
    }

    // ── deleteMessage mutation ───────────────────────────────────────────

    #[tokio::test]
    async fn delete_message_mutation_soft_deletes() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let message_id = MessageId::new();

        let mut msg = MockMessageService::new();
        msg.expect_delete_message()
            .returning(|_cmd| Box::pin(async { Ok(()) }));

        let schema = schema_with_mocks(
            MockAuthService::new(),
            MockConversationService::new(),
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $mid: ID!) { deleteMessage(input: { serverId: $sid, conversationId: $cid, messageId: $mid }) }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "mid": message_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
    }

    // Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
    #[test]
    fn _keep_ids_referenced() {
        let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
    }
}
