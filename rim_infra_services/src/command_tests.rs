#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand, UserCommandId};
    use rim_core::command::repository::UserCommandRepository;
    use rim_core::repository::error::RepoResult;
    use rim_services::command::dto::{
        CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto, CommandResultStatusDto,
        CommandSourceKindDto, CommandSurfaceDto, UserCommandStepDto,
    };
    use rim_services::command::initiator::CommandInitiator;
    use rim_services::command::request::{
        CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
        ListCommandsQuery, UpdateUserCommandCmd,
    };
    use rim_services::command::sensitivity::CommandSensitivityDto;
    use rim_services::command::service::CommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;
    use rim_services::{Actor, ServiceError};

    use crate::CommandRepos;
    use crate::command::registry;
    use crate::command::{CommandServices, DefaultCommandService};

    mockall::mock! {
        UserCommandRepository {}
        #[async_trait]
        impl UserCommandRepository for UserCommandRepository {
            async fn get_by_key_for_user(
                &self,
                owner_user_id: rim_core::user::model::UserId,
                command_key: &str,
            ) -> RepoResult<Option<UserCommand>>;
            async fn list_for_user(
                &self,
                owner_user_id: rim_core::user::model::UserId,
            ) -> RepoResult<Vec<UserCommand>>;
            async fn create_user_command(
                &self,
                new_command: NewUserCommand,
            ) -> RepoResult<UserCommand>;
            async fn update_user_command(
                &self,
                id: UserCommandId,
                update: UpdateUserCommand,
            ) -> RepoResult<UserCommand>;
            async fn delete_user_command(&self, id: UserCommandId) -> RepoResult<()>;
        }
    }

    fn service(
        repo: MockUserCommandRepository,
        conversation_service: MockConversationService,
        message_service: MockMessageService,
    ) -> DefaultCommandService {
        service_with_capability(
            repo,
            conversation_service,
            message_service,
            rim_services::capability::service::MockCapabilityService::new(),
        )
    }

    fn service_with_capability(
        repo: MockUserCommandRepository,
        conversation_service: MockConversationService,
        message_service: MockMessageService,
        capability_service: rim_services::capability::service::MockCapabilityService,
    ) -> DefaultCommandService {
        service_with_moderation(
            repo,
            conversation_service,
            message_service,
            capability_service,
            rim_services::moderation::service::MockModerationService::no_timeouts(),
        )
    }

    fn service_with_moderation(
        repo: MockUserCommandRepository,
        conversation_service: MockConversationService,
        message_service: MockMessageService,
        capability_service: rim_services::capability::service::MockCapabilityService,
        moderation_service: rim_services::moderation::service::MockModerationService,
    ) -> DefaultCommandService {
        service_with_workspace_state(
            repo,
            conversation_service,
            message_service,
            capability_service,
            moderation_service,
            MockWorkspaceStateService::new(),
        )
    }

    fn service_with_workspace_state(
        repo: MockUserCommandRepository,
        conversation_service: MockConversationService,
        message_service: MockMessageService,
        capability_service: rim_services::capability::service::MockCapabilityService,
        moderation_service: rim_services::moderation::service::MockModerationService,
        workspace_state_service: MockWorkspaceStateService,
    ) -> DefaultCommandService {
        DefaultCommandService::new(
            CommandRepos {
                user_command_repo: Arc::new(repo),
                macro_repo: Arc::new(rim_core::macro_def::repository::MockMacroRepository::new()),
            },
            CommandServices {
                auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
                capability_service: Arc::new(capability_service),
                conversation_service: Arc::new(conversation_service),
                message_service: Arc::new(message_service),
                message_reaction_service: Arc::new(
                    rim_services::message_reaction::service::MockMessageReactionService::new(),
                ),
                pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
                read_state_service: Arc::new(
                    rim_services::read_state::service::MockReadStateService::new(),
                ),
                server_member_service: Arc::new(
                    rim_services::server_member::service::MockServerMemberService::new(),
                ),
                user_service: Arc::new(rim_services::user::service::MockUserService::new()),
                workspace_state_service: Arc::new(workspace_state_service),
                audit_log_service: None,
                moderation_service: Arc::new(moderation_service),
                notification_service: Arc::new(
                    rim_services::notification::service::MockNotificationService::new(),
                ),
            },
        )
    }

    #[tokio::test]
    async fn list_commands_includes_builtins_and_user_commands() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().return_once(move |owner_user_id| {
            assert_eq!(owner_user_id, actor_user_id);
            Ok(vec![UserCommand {
                id: rim_core::command::model::UserCommandId::new(),
                owner_user_id,
                command_key: "user.test".to_string(),
                title: "Test".to_string(),
                description: "desc".to_string(),
                steps_json: serde_json::json!([{ "command_id": "chat.post_message", "args": null }]),
                is_enabled: true,
                created_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }])
        });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let commands = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .unwrap();

        assert!(
            commands
                .iter()
                .any(|command| command.id == "chat.post_message")
        );
        assert!(commands.iter().any(|command| command.id == "user.test"));
    }

    #[tokio::test]
    async fn create_user_command_validates_prefix() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let repo = MockUserCommandRepository::new();
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .create_user_command(CreateUserCommandCmd {
                actor,
                command_key: "bad.id".to_string(),
                title: "Bad".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    #[tokio::test]
    async fn execute_builtin_backend_command_posts_message() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let repo = MockUserCommandRepository::new();
        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| {
                assert_eq!(cmd.actor.user_id, actor_user_id);
                assert_eq!(cmd.text.as_deref(), Some("hello"));
                Box::pin(async move {
                    Ok(MessageDto {
                        id: rim_core::message::model::MessageId::new(),
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        thread_id: None,
                        root_thread_id: None,
                        author_id: cmd.actor.user_id,
                        text: cmd.text,
                        has_attachments: false,
                        attachments: vec![],
                        parent_message_id: None,
                        forwarded_from_message_id: None,
                        thread_root_message_id: None,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                        encryption: None,
                    })
                })
            });
        let mut capability_service =
            rim_services::capability::service::MockCapabilityService::new();
        capability_service
            .expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Ok(()));
        let svc = service_with_capability(
            repo,
            MockConversationService::new(),
            message_service,
            capability_service,
        );

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "chat.post_message".to_string(),
                args: Some(serde_json::json!({
                    "server_id": rim_core::server::model::ServerId::new().to_string(),
                    "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                    "text": "hello"
                })),
                context: None,
                confirmation: None,
                is_pipe: false,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    #[tokio::test]
    async fn list_commands_filters_context_menu_by_required_context() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let without_context = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::ContextMenu,
            })
            .await
            .unwrap();
        assert!(without_context.is_empty());

        let with_context = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(CommandContextDto {
                    workspace_id: Some(rim_core::server::model::ServerId::new()),
                    conversation_id: Some(rim_core::conversation::model::ConversationId::new()),
                    thread_id: None,
                    selected_text: None,
                    selected_message_id: Some(rim_core::message::model::MessageId::new()),
                    selected_message_ids: vec![],
                    focused_surface: Some("message_list".to_string()),
                    active_panel: None,
                }),
                surface: CommandSurfaceDto::ContextMenu,
            })
            .await
            .unwrap();
        assert!(
            with_context
                .iter()
                .any(|command| command.id == "chat.reply_to_selected_message")
        );
    }

    #[tokio::test]
    async fn create_user_command_rejects_self_reference() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user().returning(|_, _| Ok(None));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .create_user_command(CreateUserCommandCmd {
                actor,
                command_key: "user.loop".to_string(),
                title: "Loop".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "user.loop".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Validation(_)));
    }

    fn ai_command(id: &str, ai_callable: bool, caps: &[&str]) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: id.to_string(),
            title: id.to_string(),
            description: "test".to_string(),
            category: "Test".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
            required_context: vec![],
            required_capabilities: caps.iter().map(|s| s.to_string()).collect(),
            visible_in_palette: true,
            visible_in_context_menus: false,
            keybind_capable: false,
            dangerous: false,
            user_command_id: None,
            nl_description: None,
            intent_samples: vec![],
            ai_callable,
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: rim_services::command::verb::TypedVerb::Other,
            rate_limit_per_minute: None,
        }
    }

    fn cap_grants(
        granted: &[&'static str],
    ) -> rim_services::capability::service::MockCapabilityService {
        use std::collections::HashMap;
        let granted: std::collections::HashSet<String> =
            granted.iter().map(|s| s.to_string()).collect();
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many().returning(move |_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                out.insert(c.clone(), granted.contains(c));
            }
            Ok(out)
        });
        cap
    }

    #[tokio::test]
    async fn ai_surface_drops_non_ai_callable_commands() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap_grants(&["chat.read"]),
        );

        let input = vec![
            ai_command("a.ai", true, &["chat.read"]),
            ai_command("b.not_ai", false, &["chat.read"]),
            ai_command("c.ai", true, &["chat.read"]),
        ];
        let kept = svc
            .filter_ai_surface_test_only(&actor, None, input)
            .await
            .unwrap();
        let ids: Vec<&str> = kept.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(ids, vec!["a.ai", "c.ai"]);
    }

    #[tokio::test]
    async fn ai_surface_drops_command_when_actor_lacks_required_capability() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap_grants(&["chat.read"]),
        );

        let input = vec![ai_command("write.thing", true, &["chat.write"])];
        let kept = svc
            .filter_ai_surface_test_only(&actor, None, input)
            .await
            .unwrap();
        assert!(kept.is_empty(), "actor lacking chat.write must be filtered");
    }

    #[tokio::test]
    async fn ai_surface_requires_all_capabilities_not_any() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap_grants(&["chat.read"]),
        );

        let only_one_held = vec![ai_command("a.partial", true, &["chat.read", "admin.x"])];
        let kept = svc
            .filter_ai_surface_test_only(&actor, None, only_one_held)
            .await
            .unwrap();
        assert!(kept.is_empty(), "partial capability hold must filter out");

        let svc_all = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap_grants(&["chat.read", "admin.x"]),
        );
        let both = vec![ai_command("a.both", true, &["chat.read", "admin.x"])];
        let kept = svc_all
            .filter_ai_surface_test_only(&actor, None, both)
            .await
            .unwrap();
        assert_eq!(kept.len(), 1);
        assert_eq!(kept[0].id, "a.both");
    }

    #[tokio::test]
    async fn ai_surface_keeps_command_with_no_required_capabilities() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap_grants(&["chat.read"]),
        );

        let input = vec![
            ai_command("free.thing", true, &[]),
            ai_command("read.thing", true, &["chat.read"]),
        ];
        let kept = svc
            .filter_ai_surface_test_only(&actor, None, input)
            .await
            .unwrap();
        let ids: std::collections::BTreeSet<&str> = kept.iter().map(|c| c.id.as_str()).collect();
        assert!(ids.contains("free.thing"));
        assert!(ids.contains("read.thing"));
    }

    #[tokio::test]
    async fn ai_surface_returns_empty_short_circuits_capability_check() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            rim_services::capability::service::MockCapabilityService::new(),
        );

        let input = vec![
            ai_command("a.no", false, &["chat.read"]),
            ai_command("b.no", false, &["chat.write"]),
        ];
        let kept = svc
            .filter_ai_surface_test_only(&actor, None, input)
            .await
            .unwrap();
        assert!(kept.is_empty());
    }

    #[tokio::test]
    async fn list_commands_palette_surface_unaffected_by_ai_callable_flag() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc = service_with_capability(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
            rim_services::capability::service::MockCapabilityService::new(),
        );

        let palette = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .unwrap();

        let expected_palette_count = registry::builtin_commands()
            .iter()
            .filter(|c| c.visible_in_palette)
            .count();
        assert_eq!(palette.len(), expected_palette_count);
        assert!(palette.iter().any(|c| c.id == "chat.post_message"));
    }

    #[tokio::test]
    // Every AI-callable builtin whose capabilities the mock grants (`chat.read`, `chat.write`,
    // `admin.manage_members`) or that needs none.
    async fn list_commands_ai_surface_returns_forty_one_commands_with_caps() {
        use std::collections::HashMap;
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many()
            .withf(|_actor, _ws, caps| {
                let cap_set: std::collections::HashSet<&str> =
                    caps.iter().map(|s| s.as_str()).collect();
                cap_set.len() == 3
                    && cap_set.contains("admin.manage_members")
                    && cap_set.contains("chat.read")
                    && cap_set.contains("chat.write")
            })
            .returning(|_, _, caps| {
                let mut out = HashMap::with_capacity(caps.len());
                for c in caps {
                    out.insert(c.clone(), true);
                }
                Ok(out)
            });

        let svc = service_with_capability(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
            cap,
        );

        let ai = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(CommandContextDto {
                    workspace_id: Some(rim_core::server::model::ServerId::new()),
                    conversation_id: None,
                    thread_id: None,
                    selected_text: None,
                    selected_message_id: None,
                    selected_message_ids: vec![],
                    focused_surface: None,
                    active_panel: None,
                }),
                surface: CommandSurfaceDto::Ai,
            })
            .await
            .unwrap();
        let ids: Vec<&str> = ai.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            ids,
            vec![
                "admin.timeout",
                "chat.archive_channel",
                "chat.create_channel",
                "chat.create_group",
                "chat.get_recent_messages",
                "chat.invite_to_conversation",
                "chat.join_channel",
                "chat.leave_conversation",
                "chat.list_channels",
                "chat.list_members",
                "chat.mark_as_read",
                "chat.open_thread",
                "chat.pin_message",
                "chat.post_message",
                "chat.report_message",
                "chat.scroll_to_bottom",
                "chat.scroll_to_top",
                "chat.search_messages",
                "chat.set_slow_mode",
                "chat.toggle_reaction",
                "chat.unarchive_channel",
                "notifications.dnd",
                "notifications.mark_all_read",
                "notifications.set_level",
                "state.delete",
                "state.get",
                "state.incr",
                "state.list",
                "state.set",
                "workspace.close_panel",
                "workspace.navigate_to_channel",
                "workspace.next_channel",
                "workspace.next_unread_channel",
                "workspace.open_marketplace",
                "workspace.open_search",
                "workspace.open_settings",
                "workspace.previous_channel",
                "workspace.reopen_last_thread",
                "workspace.search_in_channel",
                "workspace.switch_workspace",
                "workspace.toggle_sidebar",
            ]
        );
    }

    #[tokio::test]
    async fn ai_surface_uses_workspace_id_from_context() {
        use std::collections::HashMap;
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let workspace = rim_core::server::model::ServerId::new();

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many()
            .withf(move |_actor, ws, _caps| ws.as_ref() == Some(&workspace))
            .returning(|_, _, caps| {
                let mut out = HashMap::with_capacity(caps.len());
                for c in caps {
                    out.insert(c.clone(), true);
                }
                Ok(out)
            });

        let svc = service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            cap,
        );

        let context = CommandContextDto {
            workspace_id: Some(workspace),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        };
        let input = vec![ai_command("scoped", true, &["chat.read"])];
        let kept = svc
            .filter_ai_surface_test_only(&actor, Some(&context), input)
            .await
            .unwrap();
        assert_eq!(kept.len(), 1);
    }

    #[tokio::test]
    async fn update_and_delete_user_command_work() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let existing = UserCommand {
            id: rim_core::command::model::UserCommandId::new(),
            owner_user_id: actor.user_id,
            command_key: "user.quick_group".to_string(),
            title: "Quick Group".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!([{ "command_id": "chat.create_group", "args": null }]),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let updated = UserCommand {
            title: "Quick Group Updated".to_string(),
            is_enabled: false,
            ..existing.clone()
        };
        let mut repo = MockUserCommandRepository::new();
        let existing_for_lookup = existing.clone();
        let updated_for_return = updated.clone();
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, command_key| {
                assert_eq!(owner_user_id, actor_user_id);
                assert_eq!(command_key, existing_for_lookup.command_key.as_str());
                Ok(Some(existing_for_lookup))
            });
        repo.expect_update_user_command()
            .return_once(move |id, update_cmd| {
                assert_eq!(id, updated_for_return.id);
                assert_eq!(update_cmd.title, "Quick Group Updated");
                assert_eq!(update_cmd.description, "desc");
                assert!(!update_cmd.is_enabled);
                Ok(updated_for_return)
            });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let result = svc
            .update_user_command(UpdateUserCommandCmd {
                actor,
                command_id: existing.command_key.clone(),
                title: updated.title.clone(),
                description: updated.description.clone(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.create_group".to_string(),
                    args: None,
                }],
                is_enabled: false,
            })
            .await
            .unwrap();
        assert_eq!(result.title, "Quick Group Updated");
        assert!(!result.is_enabled);

        let mut repo = MockUserCommandRepository::new();
        let delete_actor_user_id = actor.user_id;
        let expected_command_key = existing.command_key.clone();
        let expected_command_id = existing.id;
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, command_key| {
                assert_eq!(owner_user_id, delete_actor_user_id);
                assert_eq!(command_key, expected_command_key.as_str());
                Ok(Some(existing))
            });
        repo.expect_delete_user_command().return_once(move |id| {
            assert_eq!(id, expected_command_id);
            Ok(())
        });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );
        svc.delete_user_command(DeleteUserCommandCmd {
            actor,
            command_id: "user.quick_group".to_string(),
        })
        .await
        .unwrap();
    }

    #[derive(Clone)]
    struct CapturedAuditWriter(Arc<std::sync::Mutex<Vec<u8>>>);

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for CapturedAuditWriter {
        type Writer = AuditBufWriter;
        fn make_writer(&'a self) -> Self::Writer {
            AuditBufWriter(Arc::clone(&self.0))
        }
    }

    struct AuditBufWriter(Arc<std::sync::Mutex<Vec<u8>>>);
    impl std::io::Write for AuditBufWriter {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    fn capturing_subscriber() -> (
        impl tracing::Subscriber + Send + Sync,
        Arc<std::sync::Mutex<Vec<u8>>>,
    ) {
        use tracing_subscriber::Layer;
        use tracing_subscriber::layer::SubscriberExt;
        let buf = Arc::new(std::sync::Mutex::new(Vec::<u8>::new()));
        let writer = CapturedAuditWriter(Arc::clone(&buf));
        let layer = tracing_subscriber::fmt::layer()
            .with_target(true)
            .with_ansi(false)
            .with_writer(writer)
            .with_filter(tracing_subscriber::filter::filter_fn(|m| {
                m.target() == "audit"
            }));
        let subscriber = tracing_subscriber::registry().with(layer);
        (subscriber, buf)
    }

    fn drain_buf(buf: &Arc<std::sync::Mutex<Vec<u8>>>) -> String {
        String::from_utf8(buf.lock().unwrap().clone()).expect("audit emits are UTF-8 JSON")
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_emits_audit_line_on_success() {
        let (subscriber, buf) = capturing_subscriber();

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Ok(()));

        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| {
                Box::pin(async move {
                    Ok(MessageDto {
                        id: rim_core::message::model::MessageId::new(),
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        thread_id: None,
                        root_thread_id: None,
                        author_id: cmd.actor.user_id,
                        text: cmd.text,
                        has_attachments: false,
                        attachments: vec![],
                        parent_message_id: None,
                        forwarded_from_message_id: None,
                        thread_root_message_id: None,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                        encryption: None,
                    })
                })
            });

        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc =
            service_with_capability(repo, MockConversationService::new(), message_service, cap);

        let server_id = rim_core::server::model::ServerId::new();
        let conversation_id = rim_core::conversation::model::ConversationId::new();

        let _guard = tracing::subscriber::set_default(subscriber);
        {
            svc.execute_command(ExecuteCommandCmd {
                actor,
                command_id: "chat.post_message".to_string(),
                args: Some(serde_json::json!({
                    "server_id": server_id.to_string(),
                    "conversation_id": conversation_id.to_string(),
                    "text": "audit me",
                })),
                context: None,
                confirmation: None,
                is_pipe: false,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .expect("send must succeed");
        }

        let captured = drain_buf(&buf);
        assert!(
            captured.contains("event=") && captured.contains("command_executed"),
            "missing event field; captured = {captured}",
        );
        assert!(
            captured.contains("chat.post_message"),
            "missing command_id; captured = {captured}",
        );
        assert!(
            captured.contains(&actor_user_id.to_string()),
            "missing/incorrect actor_id; captured = {captured}",
        );
        assert!(
            captured.contains("initiator=") && captured.contains("palette"),
            "missing initiator=palette; captured = {captured}",
        );
        assert!(
            captured.contains("result_status=") && captured.contains("success"),
            "missing result_status=success; captured = {captured}",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_emits_audit_line_on_capability_denied() {
        let (subscriber, buf) = capturing_subscriber();

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Err(ServiceError::PermissionDenied));

        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc = service_with_capability(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
            cap,
        );

        let _guard = tracing::subscriber::set_default(subscriber);
        {
            let _ = svc
                    .execute_command(ExecuteCommandCmd {
                        actor,
                        command_id: "chat.post_message".to_string(),
                        args: Some(serde_json::json!({
                            "server_id": rim_core::server::model::ServerId::new().to_string(),
                            "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                            "text": "denied",
                        })),
                        context: None,
                        confirmation: None,
                        is_pipe: false,
                        initiator: rim_services::command::initiator::CommandInitiator::Agent,
                    })
                    .await;
        }

        let captured = drain_buf(&buf);
        assert!(
            captured.contains("event=") && captured.contains("command_executed"),
            "missing event on denial path; captured = {captured}",
        );
        assert!(
            captured.contains("result_status=") && captured.contains("error"),
            "denial must record result_status=error; captured = {captured}",
        );
        assert!(
            captured.to_lowercase().contains("error=") || captured.contains("permission denied"),
            "denial must include error field; captured = {captured}",
        );
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_audit_line_redacts_sensitive_args() {
        let (subscriber, buf) = capturing_subscriber();

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Ok(()));

        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| {
                Box::pin(async move {
                    Ok(MessageDto {
                        id: rim_core::message::model::MessageId::new(),
                        server_id: cmd.server_id,
                        conversation_id: cmd.conversation_id,
                        thread_id: None,
                        root_thread_id: None,
                        author_id: cmd.actor.user_id,
                        text: cmd.text,
                        has_attachments: false,
                        attachments: vec![],
                        parent_message_id: None,
                        forwarded_from_message_id: None,
                        thread_root_message_id: None,
                        is_deleted: false,
                        sent_at: chrono::Utc::now(),
                        encryption: None,
                    })
                })
            });

        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc =
            service_with_capability(repo, MockConversationService::new(), message_service, cap);

        let _guard = tracing::subscriber::set_default(subscriber);
        {
            svc.execute_command(ExecuteCommandCmd {
                    actor,
                    command_id: "chat.post_message".to_string(),
                    args: Some(serde_json::json!({
                        "server_id": rim_core::server::model::ServerId::new().to_string(),
                        "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                        "text": "hello",
                        "password": "hunter2",
                        "api_token": "sk-proj-abcdefghijklmnopqrstuvwxyz0123456789",
                    })),
                    context: None,
                    confirmation: None,
                    is_pipe: false,
                    initiator: rim_services::command::initiator::CommandInitiator::Agent,
                })
                .await
                .expect("send should succeed");
        }

        let captured = drain_buf(&buf);
        assert!(
            captured.contains("[REDACTED]"),
            "audit line must redact sensitive keys; captured = {captured}",
        );
        assert!(
            !captured.contains("hunter2"),
            "raw password value must NOT appear in audit; captured = {captured}",
        );
        assert!(
            !captured.contains("sk-proj-abcdefghijklmnopqrstuvwxyz0123456789"),
            "raw API key value must NOT appear in audit; captured = {captured}",
        );
        assert!(
            captured.contains("hello"),
            "benign args must remain visible; captured = {captured}",
        );
    }

    fn rate_limited_cmd_def(id: &str, limit: u32) -> CommandDefinitionDto {
        CommandDefinitionDto {
            id: id.to_string(),
            title: id.to_string(),
            description: "test rate-limited command".to_string(),
            category: "Test".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
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
            sensitivity: CommandSensitivityDto::Safe,
            typed_verb: rim_services::command::verb::TypedVerb::Read,
            rate_limit_per_minute: Some(limit),
        }
    }

    fn service_with_rate_limited_cmd(
        cmd_def: CommandDefinitionDto,
        audit_svc: Option<Arc<dyn rim_services::audit::service::AuditLogService>>,
    ) -> DefaultCommandService {
        DefaultCommandService::with_extra_builtins(
            CommandRepos {
                user_command_repo: Arc::new(MockUserCommandRepository::new()),
                macro_repo: Arc::new(rim_core::macro_def::repository::MockMacroRepository::new()),
            },
            CommandServices {
                auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
                capability_service: Arc::new(cap_grants(&[])),
                conversation_service: Arc::new(MockConversationService::new()),
                message_service: Arc::new(MockMessageService::new()),
                message_reaction_service: Arc::new(
                    rim_services::message_reaction::service::MockMessageReactionService::new(),
                ),
                pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
                read_state_service: Arc::new(
                    rim_services::read_state::service::MockReadStateService::new(),
                ),
                server_member_service: Arc::new(
                    rim_services::server_member::service::MockServerMemberService::new(),
                ),
                user_service: Arc::new(rim_services::user::service::MockUserService::new()),
                workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
                audit_log_service: audit_svc,
                moderation_service: std::sync::Arc::new(
                    rim_services::moderation::service::MockModerationService::no_timeouts(),
                ),
                notification_service: std::sync::Arc::new(
                    rim_services::notification::service::MockNotificationService::new(),
                ),
            },
            vec![cmd_def],
        )
    }

    /// A palette command with no args in a fresh workspace context.
    fn palette_cmd(actor: Actor, command_id: &str) -> ExecuteCommandCmd {
        ExecuteCommandCmd {
            actor,
            command_id: command_id.to_string(),
            args: None,
            context: Some(workspace_context(rim_core::server::model::ServerId::new())),
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        }
    }

    #[tokio::test]
    async fn r1_third_call_within_window_is_rate_limited_and_audited() {
        use rim_services::audit::dto::AuditOutcome;

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let cmd_id = "test.rate_limited";
        let cmd_def = rate_limited_cmd_def(cmd_id, 2);

        let written: Arc<std::sync::Mutex<Vec<rim_services::audit::dto::AuditLogEntryDto>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let written_clone = written.clone();

        let mut audit_svc = rim_services::audit::service::MockAuditLogService::new();
        audit_svc
            .expect_write_entry()
            .times(3)
            .returning(move |entry| {
                written_clone.lock().unwrap().push(entry);
                Box::pin(async { Ok(()) })
            });

        let svc = service_with_rate_limited_cmd(cmd_def, Some(Arc::new(audit_svc)));

        let r1 = svc.execute_command(palette_cmd(actor, cmd_id)).await;
        assert!(r1.is_ok(), "call 1 should be allowed, got: {r1:?}");

        let r2 = svc.execute_command(palette_cmd(actor, cmd_id)).await;
        assert!(r2.is_ok(), "call 2 should be allowed, got: {r2:?}");

        let r3 = svc.execute_command(palette_cmd(actor, cmd_id)).await;
        assert!(
            matches!(
                r3,
                Err(ServiceError::RateLimited { retry_after_seconds }) if retry_after_seconds > 0
            ),
            "call 3 should be rate-limited with retry_after_seconds > 0, got: {r3:?}"
        );

        let entries = written.lock().unwrap();
        assert_eq!(entries.len(), 3, "all 3 calls must emit audit rows");
        let denied = entries
            .iter()
            .find(|e| e.outcome == AuditOutcome::RateLimited)
            .expect("one audit row must have outcome=RateLimited");
        assert_eq!(denied.command_id, cmd_id);
    }

    #[tokio::test]
    async fn r1_rate_limit_is_per_actor() {
        let actor_a = Actor::new(rim_core::user::model::UserId::new());
        let actor_b = Actor::new(rim_core::user::model::UserId::new());
        let cmd_id = "test.rate_limited_per_actor";
        let cmd_def = rate_limited_cmd_def(cmd_id, 1);

        let mut audit_svc = rim_services::audit::service::MockAuditLogService::new();
        audit_svc
            .expect_write_entry()
            .returning(|_| Box::pin(async { Ok(()) }));

        let svc = service_with_rate_limited_cmd(cmd_def, Some(Arc::new(audit_svc)));

        let ra = svc.execute_command(palette_cmd(actor_a, cmd_id)).await;
        assert!(ra.is_ok(), "actor A call should be allowed, got: {ra:?}");

        let rb = svc.execute_command(palette_cmd(actor_b, cmd_id)).await;
        assert!(
            rb.is_ok(),
            "actor B call should be allowed (independent quota), got: {rb:?}"
        );
    }

    fn service_with_macro(
        cap: rim_services::capability::service::MockCapabilityService,
        macro_svc: Option<Arc<dyn rim_services::macro_def::service::MacroService>>,
    ) -> DefaultCommandService {
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));

        let svc = DefaultCommandService::new(
            CommandRepos {
                user_command_repo: Arc::new(repo),
                macro_repo: Arc::new(rim_core::macro_def::repository::MockMacroRepository::new()),
            },
            CommandServices {
                auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
                capability_service: Arc::new(cap),
                conversation_service: Arc::new(MockConversationService::new()),
                message_service: Arc::new(MockMessageService::new()),
                message_reaction_service: Arc::new(
                    rim_services::message_reaction::service::MockMessageReactionService::new(),
                ),
                pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
                read_state_service: Arc::new(
                    rim_services::read_state::service::MockReadStateService::new(),
                ),
                server_member_service: Arc::new(
                    rim_services::server_member::service::MockServerMemberService::new(),
                ),
                user_service: Arc::new(rim_services::user::service::MockUserService::new()),
                workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
                audit_log_service: None,
                moderation_service: std::sync::Arc::new(
                    rim_services::moderation::service::MockModerationService::no_timeouts(),
                ),
                notification_service: std::sync::Arc::new(
                    rim_services::notification::service::MockNotificationService::new(),
                ),
            },
        );
        if let Some(macro_svc) = macro_svc {
            let _ = svc.set_macro_service(macro_svc);
        }
        svc
    }

    fn full_caps() -> rim_services::capability::service::MockCapabilityService {
        use std::collections::HashMap;
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many().returning(|_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                out.insert(c.clone(), true);
            }
            Ok(out)
        });
        cap
    }

    #[tokio::test]
    async fn command_discover_actor_with_full_caps_gets_all_ai_callable() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_macro(full_caps(), None);

        let result = svc
            .command_discover(&actor, None, None, None)
            .await
            .unwrap();

        let ids: std::collections::BTreeSet<&str> = result.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains("chat.post_message"),
            "chat.post_message must be discoverable with full caps"
        );
        assert!(
            ids.contains("chat.list_channels"),
            "chat.list_channels must be discoverable with full caps"
        );
        assert!(
            result.iter().all(|c| c.ai_callable),
            "command_discover must return only ai_callable commands"
        );
        let sorted: Vec<&str> = ids.iter().copied().collect();
        let unsorted: Vec<&str> = result.iter().map(|c| c.id.as_str()).collect();
        assert_eq!(
            unsorted, sorted,
            "command_discover result must be sorted by id when no intent_phrase"
        );
    }

    #[tokio::test]
    async fn command_discover_actor_with_no_caps_gets_only_no_cap_commands() {
        use std::collections::HashMap;
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many().returning(|_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                out.insert(c.clone(), false);
            }
            Ok(out)
        });

        let svc = service_with_macro(cap, None);

        let result = svc
            .command_discover(&actor, None, None, None)
            .await
            .unwrap();

        assert!(result.iter().all(|c| c.ai_callable));
        assert!(
            result.iter().all(|c| c.required_capabilities.is_empty()),
            "actor with all-denied caps must receive only zero-cap commands, got: {:?}",
            result.iter().map(|c| &c.id).collect::<Vec<_>>()
        );
        let ids: std::collections::BTreeSet<&str> = result.iter().map(|c| c.id.as_str()).collect();
        assert!(
            !ids.contains("chat.post_message"),
            "chat.post_message (requires chat.write) must be absent"
        );
        assert!(
            !ids.contains("chat.list_channels"),
            "chat.list_channels (requires chat.read) must be absent"
        );
    }

    #[tokio::test]
    async fn command_discover_partial_caps_filters_correctly() {
        use std::collections::HashMap;
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many().returning(|_, _, caps| {
            let mut out = HashMap::with_capacity(caps.len());
            for c in caps {
                out.insert(c.clone(), c == "chat.read");
            }
            Ok(out)
        });

        let svc = service_with_macro(cap, None);

        let result = svc
            .command_discover(&actor, None, None, None)
            .await
            .unwrap();

        let ids: std::collections::BTreeSet<&str> = result.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains("chat.list_channels"),
            "chat.list_channels (needs chat.read only) must be present"
        );
        assert!(
            !ids.contains("chat.post_message"),
            "chat.post_message (needs chat.write) must be filtered out"
        );
        assert!(result.iter().all(|c| c.ai_callable));
    }

    #[tokio::test]
    async fn command_discover_macro_inclusion_with_workspace_id() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let workspace_id = rim_core::server::model::ServerId::new();
        let macro_cmd_id = format!("macro.{}", uuid::Uuid::now_v7());
        let macro_cmd_id_clone = macro_cmd_id.clone();

        let mut macro_svc = rim_services::macro_def::service::MockMacroService::new();
        macro_svc
            .expect_synthesize_command_definitions()
            .withf(move |q| q.server_id == workspace_id)
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(vec![CommandDefinitionDto {
                        id: macro_cmd_id_clone.clone(),
                        title: "Send Weekly Summary".to_string(),
                        description: "Posts a weekly summary".to_string(),
                        category: "Macro".to_string(),
                        source_kind: CommandSourceKindDto::User,
                        execution_target: CommandExecutionTargetDto::Backend,
                        arguments: vec![],
                        required_context: vec![],
                        required_capabilities: vec![],
                        visible_in_palette: false,
                        visible_in_context_menus: false,
                        keybind_capable: false,
                        dangerous: false,
                        user_command_id: None,
                        nl_description: Some("Send a weekly summary to the team".to_string()),
                        intent_samples: vec!["send weekly summary".to_string()],
                        ai_callable: true,
                        sensitivity: CommandSensitivityDto::Safe,
                        typed_verb: rim_services::command::verb::TypedVerb::Other,
                        rate_limit_per_minute: None,
                    }])
                })
            });

        let svc = service_with_macro(full_caps(), Some(Arc::new(macro_svc)));

        let result = svc
            .command_discover(&actor, None, None, Some(workspace_id))
            .await
            .unwrap();

        let ids: Vec<&str> = result.iter().map(|c| c.id.as_str()).collect();
        assert!(
            ids.contains(&macro_cmd_id.as_str()),
            "synthesized macro must appear in command_discover when workspace_id is supplied; \
             ids: {ids:?}"
        );
        assert!(result.iter().all(|c| c.ai_callable));
    }

    #[tokio::test]
    async fn command_discover_without_workspace_id_excludes_macros() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_macro(full_caps(), None);

        let result = svc
            .command_discover(&actor, None, None, None)
            .await
            .unwrap();

        assert!(
            result.iter().all(|c| !c.id.starts_with("macro.")),
            "command_discover without workspace must not include synthesized macros"
        );
        assert!(
            !result.is_empty(),
            "command_discover with full caps must return at least one builtin"
        );
    }

    #[tokio::test]
    async fn command_discover_intent_ranking_phrase_post() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_macro(full_caps(), None);

        let ranked = svc
            .command_discover(&actor, Some("post".to_string()), None, None)
            .await
            .unwrap();

        assert!(!ranked.is_empty(), "ranked result must not be empty");

        let post_msg_pos = ranked
            .iter()
            .position(|c| c.id == "chat.post_message")
            .expect("chat.post_message must be in the ranked result");

        let settings_pos = ranked
            .iter()
            .position(|c| c.id == "workspace.open_settings");

        if let Some(settings_pos) = settings_pos {
            assert!(
                post_msg_pos < settings_pos,
                "chat.post_message (has 'post' match) must rank above workspace.open_settings \
                 (no match); positions: post_msg={post_msg_pos}, settings={settings_pos}"
            );
        }
    }

    #[tokio::test]
    async fn command_discover_limit_cap_enforced() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_macro(full_caps(), None);

        let all = svc
            .command_discover(&actor, None, None, None)
            .await
            .unwrap();

        if all.is_empty() {
            return;
        }

        let capped = svc
            .command_discover(&actor, None, Some(200), None)
            .await
            .unwrap();

        assert!(
            capped.len() <= 100,
            "limit=200 must be capped at 100, got {}",
            capped.len()
        );
        let expected_len = all.len().min(100);
        assert_eq!(
            capped.len(),
            expected_len,
            "limit=200 with {} total commands must return min({},100)={}",
            all.len(),
            all.len(),
            expected_len
        );
    }

    #[tokio::test]
    async fn command_discover_rate_limit_fires_on_sixth_call() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_macro(full_caps(), None);

        for i in 1..=5 {
            let result = svc.command_discover(&actor, None, Some(1), None).await;
            assert!(result.is_ok(), "call {i} must be allowed, got: {result:?}");
        }

        let sixth = svc.command_discover(&actor, None, Some(1), None).await;
        assert!(
            matches!(sixth, Err(ServiceError::RateLimited { .. })),
            "6th call in window must return RateLimited, got: {sixth:?}"
        );
    }

    #[tokio::test]
    async fn command_discover_rate_limit_is_per_actor() {
        let actor_a = Actor::new(rim_core::user::model::UserId::new());
        let actor_b = Actor::new(rim_core::user::model::UserId::new());

        let svc = service_with_macro(full_caps(), None);

        for _ in 0..5 {
            let _ = svc.command_discover(&actor_a, None, Some(1), None).await;
        }
        let a_sixth = svc.command_discover(&actor_a, None, Some(1), None).await;
        assert!(
            matches!(a_sixth, Err(ServiceError::RateLimited { .. })),
            "actor A 6th call must be rate-limited"
        );

        let b_first = svc.command_discover(&actor_b, None, Some(1), None).await;
        assert!(
            b_first.is_ok(),
            "actor B first call must be allowed despite actor A exhausted"
        );
    }

    fn service_with_mutating_command(
        cap_svc: rim_services::capability::service::MockCapabilityService,
    ) -> DefaultCommandService {
        use rim_services::command::dto::*;
        use rim_services::command::sensitivity::CommandSensitivityDto;
        use rim_services::command::verb::TypedVerb;

        let mutating_cmd = CommandDefinitionDto {
            id: "test.mutating".to_string(),
            title: "Test Mutating".to_string(),
            description: "A Mutating-sensitivity command".to_string(),
            category: "Test".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
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
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
        };

        DefaultCommandService::with_extra_builtins(
            CommandRepos {
                user_command_repo: Arc::new({
                    let mut r = MockUserCommandRepository::new();
                    r.expect_get_by_key_for_user().returning(|_, _| Ok(None));
                    r
                }),
                macro_repo: Arc::new(rim_core::macro_def::repository::MockMacroRepository::new()),
            },
            CommandServices {
                auth_service: Arc::new(rim_services::auth::service::MockAuthService::new()),
                capability_service: Arc::new(cap_svc),
                conversation_service: Arc::new(MockConversationService::new()),
                message_service: Arc::new(MockMessageService::new()),
                message_reaction_service: Arc::new(
                    rim_services::message_reaction::service::MockMessageReactionService::new(),
                ),
                pin_service: Arc::new(rim_services::pin::service::MockPinService::new()),
                read_state_service: Arc::new(
                    rim_services::read_state::service::MockReadStateService::new(),
                ),
                server_member_service: Arc::new(
                    rim_services::server_member::service::MockServerMemberService::new(),
                ),
                user_service: Arc::new(rim_services::user::service::MockUserService::new()),
                workspace_state_service: Arc::new(MockWorkspaceStateService::new()),
                audit_log_service: None,
                moderation_service: std::sync::Arc::new(
                    rim_services::moderation::service::MockModerationService::no_timeouts(),
                ),
                notification_service: std::sync::Arc::new(
                    rim_services::notification::service::MockNotificationService::new(),
                ),
            },
            vec![mutating_cmd],
        )
    }

    fn cap_svc_allow_all() -> rim_services::capability::service::MockCapabilityService {
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check().returning(|_, _, _| Ok(()));
        cap
    }

    #[tokio::test]
    async fn agent_mutating_no_proof_requires_confirmation() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_mutating_command(cap_svc_allow_all());

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "test.mutating".to_string(),
                args: None,
                context: None,
                confirmation: None,
                is_pipe: false,
                initiator: CommandInitiator::Agent,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::ConfirmationRequired,
            "Agent + Mutating + no proof must require confirmation: {result:?}"
        );
        assert!(
            result.confirmation_required.is_some(),
            "challenge must be non-None so agent can re-submit"
        );
    }

    #[tokio::test]
    async fn palette_mutating_no_proof_executes() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_mutating_command(cap_svc_allow_all());

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "test.mutating".to_string(),
                args: None,
                context: None,
                confirmation: None,
                is_pipe: false,
                initiator: CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Success,
            "Palette + Mutating + no proof must execute without confirmation: {result:?}"
        );
    }

    #[tokio::test]
    async fn agent_pipe_with_mutating_stage_requires_confirmation() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_mutating_command(cap_svc_allow_all());

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "test.mutating | test.mutating".to_string(),
                args: None,
                context: None,
                confirmation: None,
                is_pipe: true,
                initiator: CommandInitiator::Agent,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::ConfirmationRequired,
            "Agent + pipe with Mutating stage + no proof must require confirmation: {result:?}"
        );
        assert!(
            result.confirmation_required.is_some(),
            "challenge token must be issued so the agent can re-submit"
        );
    }

    #[tokio::test]
    async fn palette_pipe_with_mutating_stage_executes() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service_with_mutating_command(cap_svc_allow_all());

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor,
                command_id: "test.mutating | test.mutating".to_string(),
                args: None,
                context: None,
                confirmation: None,
                is_pipe: true,
                initiator: CommandInitiator::Palette,
            })
            .await
            .unwrap();

        assert_eq!(
            result.status,
            CommandResultStatusDto::Success,
            "Palette + pipe with Mutating stage must execute without confirmation: {result:?}"
        );
    }

    #[tokio::test]
    async fn agent_mutating_valid_proof_executes() {
        use rim_services::command::dto::CommandDefinitionDto;
        use rim_services::command::dto::{CommandExecutionTargetDto, CommandSourceKindDto};
        use rim_services::command::initiator::CommandInitiator;
        use rim_services::command::sensitivity::CommandSensitivityDto;
        use rim_services::command::verb::TypedVerb;

        let mutating_def = CommandDefinitionDto {
            id: "test.mutating".to_string(),
            title: "t".to_string(),
            description: "".to_string(),
            category: "".to_string(),
            source_kind: CommandSourceKindDto::BuiltIn,
            execution_target: CommandExecutionTargetDto::Backend,
            arguments: vec![],
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
            sensitivity: CommandSensitivityDto::Mutating,
            typed_verb: TypedVerb::Other,
            rate_limit_per_minute: None,
        };

        assert!(
            mutating_def.requires_confirmation_for_initiator(CommandInitiator::Agent),
            "Agent + Mutating must require confirmation"
        );
        assert!(
            !mutating_def.requires_confirmation_for_initiator(CommandInitiator::Palette),
            "Palette + Mutating must NOT require confirmation"
        );
        let dangerous_def = CommandDefinitionDto {
            dangerous: true,
            sensitivity: CommandSensitivityDto::Safe,
            ..mutating_def.clone()
        };
        assert!(
            dangerous_def.requires_confirmation_for_initiator(CommandInitiator::Palette),
            "dangerous: true must require confirmation even for Palette"
        );
    }

    /// Builds a `MockMacroService` whose `synthesize_command_definitions` returns
    /// one synthetic `macro.<id>` `CommandDefinitionDto` with `ai_callable: true`,
    /// so it survives the AI-surface filter. Shared by the `set_macro_service`
    /// tests below.
    fn mock_macro_service_returning(
        macro_cmd_id: String,
    ) -> rim_services::macro_def::service::MockMacroService {
        use rim_services::command::dto::{CommandExecutionTargetDto, CommandSourceKindDto};

        let mut macro_svc = rim_services::macro_def::service::MockMacroService::new();
        macro_svc
            .expect_synthesize_command_definitions()
            .return_once(move |_| {
                Box::pin(async move {
                    Ok(vec![CommandDefinitionDto {
                        id: macro_cmd_id,
                        title: "Synthetic Macro".to_string(),
                        description: "Synthetic macro command for tests".to_string(),
                        category: "Macro".to_string(),
                        source_kind: CommandSourceKindDto::User,
                        execution_target: CommandExecutionTargetDto::Backend,
                        arguments: vec![],
                        required_context: vec![],
                        required_capabilities: vec![],
                        visible_in_palette: false,
                        visible_in_context_menus: false,
                        keybind_capable: false,
                        dangerous: false,
                        user_command_id: None,
                        nl_description: Some("synthetic".to_string()),
                        intent_samples: vec!["synthetic".to_string()],
                        ai_callable: true,
                        sensitivity: CommandSensitivityDto::Safe,
                        typed_verb: rim_services::command::verb::TypedVerb::Other,
                        rate_limit_per_minute: None,
                    }])
                })
            });
        macro_svc
    }

    /// Builds a `ListCommandsQuery` for `Surface::Ai` carrying a `workspace_id`,
    /// which is required for `list_commands` to reach the macro-discovery branch.
    fn ai_query_with_workspace(
        actor: Actor,
        workspace_id: rim_core::server::model::ServerId,
    ) -> ListCommandsQuery {
        ListCommandsQuery {
            actor,
            context: Some(CommandContextDto {
                workspace_id: Some(workspace_id),
                conversation_id: None,
                thread_id: None,
                selected_text: None,
                selected_message_id: None,
                selected_message_ids: vec![],
                focused_surface: None,
                active_panel: None,
            }),
            surface: CommandSurfaceDto::Ai,
        }
    }

    #[tokio::test]
    async fn set_macro_service_wires_ai_discovery() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let workspace_id = rim_core::server::model::ServerId::new();
        let macro_cmd_id = format!("macro.{}", uuid::Uuid::now_v7());

        let svc = service_with_macro(full_caps(), None);
        let macro_svc = mock_macro_service_returning(macro_cmd_id.clone());
        svc.set_macro_service(Arc::new(macro_svc))
            .expect("first set_macro_service call must succeed");

        let commands = svc
            .list_commands(ai_query_with_workspace(actor, workspace_id))
            .await
            .unwrap();

        assert!(
            commands.iter().any(|c| c.id == macro_cmd_id),
            "synthesized macro command must appear once set_macro_service wires the \
             macro service into list_commands(Surface::Ai)"
        );
    }

    /// The provider holds a single `DefaultCommandService` with the macro service
    /// late-bound behind `Arc<OnceLock<_>>`: setting it on one `Arc` clone must be
    /// observed through every other clone, including the ones keybind and pipeline
    /// hold. Fails if a second `DefaultCommandService` instance is introduced.
    #[tokio::test]
    async fn macro_visible_through_shared_arc_clone() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let workspace_id = rim_core::server::model::ServerId::new();
        let macro_cmd_id = format!("macro.{}", uuid::Uuid::now_v7());

        let svc = Arc::new(service_with_macro(full_caps(), None));
        // Stands in for what keybind/pipeline hold: a separate Arc clone of the
        // *same* underlying DefaultCommandService instance.
        let svc_clone = Arc::clone(&svc);

        let macro_svc = mock_macro_service_returning(macro_cmd_id.clone());
        svc.set_macro_service(Arc::new(macro_svc))
            .expect("first set_macro_service call must succeed");

        let commands = svc_clone
            .list_commands(ai_query_with_workspace(actor, workspace_id))
            .await
            .unwrap();

        assert!(
            commands.iter().any(|c| c.id == macro_cmd_id),
            "the late macro wire on one Arc clone must be visible through every \
             other clone of the shared DefaultCommandService instance"
        );
    }

    #[tokio::test]
    async fn set_macro_service_second_call_is_noop() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let workspace_id = rim_core::server::model::ServerId::new();
        let first_macro_cmd_id = format!("macro.{}", uuid::Uuid::now_v7());
        let second_macro_cmd_id = format!("macro.{}", uuid::Uuid::now_v7());

        let svc = service_with_macro(full_caps(), None);

        let first_macro_svc = mock_macro_service_returning(first_macro_cmd_id.clone());
        let second_macro_svc = mock_macro_service_returning(second_macro_cmd_id.clone());

        assert_eq!(
            svc.set_macro_service(Arc::new(first_macro_svc)),
            Ok(()),
            "first call must succeed"
        );
        assert_eq!(
            svc.set_macro_service(Arc::new(second_macro_svc)),
            Err(()),
            "second call must be rejected"
        );

        let commands = svc
            .list_commands(ai_query_with_workspace(actor, workspace_id))
            .await
            .unwrap();

        assert!(
            commands.iter().any(|c| c.id == first_macro_cmd_id),
            "the first-wired macro service must still be the one in effect"
        );
        assert!(
            !commands.iter().any(|c| c.id == second_macro_cmd_id),
            "the second (rejected) macro service must not have taken effect"
        );
    }

    // ── Timeout gate ──

    fn workspace_context(workspace: rim_core::server::model::ServerId) -> CommandContextDto {
        CommandContextDto {
            workspace_id: Some(workspace),
            conversation_id: None,
            thread_id: None,
            selected_text: None,
            selected_message_id: None,
            selected_message_ids: vec![],
            focused_surface: None,
            active_panel: None,
        }
    }

    fn timed_out_moderation(
        until: chrono::DateTime<chrono::Utc>,
    ) -> rim_services::moderation::service::MockModerationService {
        let mut moderation = rim_services::moderation::service::MockModerationService::new();
        moderation
            .expect_active_timeout()
            .returning(move |_, _| Box::pin(async move { Ok(Some(until)) }));
        moderation
    }

    fn allow_all_caps() -> rim_services::capability::service::MockCapabilityService {
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check().returning(|_, _, _| Ok(()));
        cap
    }

    #[tokio::test]
    async fn timed_out_actor_cannot_run_mutating_commands() {
        let workspace = rim_core::server::model::ServerId::new();
        let until = chrono::Utc::now() + chrono::Duration::minutes(10);
        let mut message_service = MockMessageService::new();
        message_service.expect_send_message().never();
        let svc = service_with_moderation(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            message_service,
            allow_all_caps(),
            timed_out_moderation(until),
        );

        let err = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(rim_core::user::model::UserId::new()),
                command_id: "chat.post_message".to_string(),
                args: Some(serde_json::json!({
                    "server_id": workspace.to_string(),
                    "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
                    "text": "hello"
                })),
                context: Some(workspace_context(workspace)),
                confirmation: None,
                is_pipe: false,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .unwrap_err();
        assert!(
            matches!(err, ServiceError::TimedOut { until: u } if u == until),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn timed_out_actor_still_runs_read_only_commands() {
        let workspace = rim_core::server::model::ServerId::new();
        let mut message_service = MockMessageService::new();
        message_service.expect_search_messages().returning(|_| {
            Box::pin(async {
                Ok(rim_core::repository::pagination::Page::new(
                    vec![],
                    None,
                    None,
                ))
            })
        });
        let mut moderation = rim_services::moderation::service::MockModerationService::new();
        // The gate only consults moderation state for Mutating commands.
        moderation.expect_active_timeout().never();
        let svc = service_with_moderation(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            message_service,
            allow_all_caps(),
            moderation,
        );

        let result = svc
            .execute_command(ExecuteCommandCmd {
                actor: Actor::new(rim_core::user::model::UserId::new()),
                command_id: "chat.search_messages".to_string(),
                args: Some(serde_json::json!({
                    "server_id": workspace.to_string(),
                    "query": "launch"
                })),
                context: Some(workspace_context(workspace)),
                confirmation: None,
                is_pipe: false,
                initiator: rim_services::command::initiator::CommandInitiator::Palette,
            })
            .await
            .expect("read-only command runs");
        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    // ── User commands and pipes ──

    fn new_actor() -> Actor {
        Actor::new(rim_core::user::model::UserId::new())
    }

    fn step(command_id: &str, args: serde_json::Value) -> serde_json::Value {
        serde_json::json!({ "command_id": command_id, "args": args })
    }

    /// An enabled user command of `owner` with the given steps.
    fn stored_user_command(
        owner_user_id: rim_core::user::model::UserId,
        command_key: &str,
        steps: Vec<serde_json::Value>,
    ) -> UserCommand {
        UserCommand {
            id: UserCommandId::new(),
            owner_user_id,
            command_key: command_key.to_string(),
            title: command_key.to_string(),
            description: String::new(),
            steps_json: serde_json::Value::Array(steps),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    /// A user command of `owner` that increments the state key `k`.
    fn bump_command(owner: rim_core::user::model::UserId, command_key: &str) -> UserCommand {
        stored_user_command(
            owner,
            command_key,
            vec![step("state.incr", serde_json::json!({ "key": "k" }))],
        )
    }

    /// A user command of `owner` whose single step is `target`.
    fn calling_command(
        owner: rim_core::user::model::UserId,
        command_key: &str,
        target: &str,
    ) -> UserCommand {
        stored_user_command(
            owner,
            command_key,
            vec![step(target, serde_json::Value::Null)],
        )
    }

    /// `user.level0` .. `user.level{levels - 1}`, each calling the next, the last one
    /// incrementing state.
    fn user_command_chain(owner: rim_core::user::model::UserId, levels: usize) -> Vec<UserCommand> {
        (0..levels)
            .map(|level| {
                let key = format!("user.level{level}");
                if level + 1 == levels {
                    bump_command(owner, &key)
                } else {
                    calling_command(owner, &key, &format!("user.level{}", level + 1))
                }
            })
            .collect()
    }

    /// A repository that answers lookups and listings by owner from `commands`.
    fn repo_with(commands: Vec<UserCommand>) -> MockUserCommandRepository {
        let mut repo = MockUserCommandRepository::new();
        let listed = commands.clone();
        repo.expect_get_by_key_for_user()
            .returning(move |owner_user_id, command_key| {
                Ok(commands
                    .iter()
                    .find(|c| c.owner_user_id == owner_user_id && c.command_key == command_key)
                    .cloned())
            });
        repo.expect_list_for_user().returning(move |owner_user_id| {
            Ok(listed
                .iter()
                .filter(|c| c.owner_user_id == owner_user_id)
                .cloned()
                .collect())
        });
        repo
    }

    fn state_service(
        repo: MockUserCommandRepository,
        workspace_state: MockWorkspaceStateService,
    ) -> DefaultCommandService {
        service_with_workspace_state(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
            rim_services::capability::service::MockCapabilityService::new(),
            rim_services::moderation::service::MockModerationService::no_timeouts(),
            workspace_state,
        )
    }

    /// A service for create and update, where no step runs.
    fn validating_service(repo: MockUserCommandRepository) -> DefaultCommandService {
        state_service(repo, MockWorkspaceStateService::new())
    }

    fn state_entry(
        server_id: rim_core::server::model::ServerId,
        key: &str,
        value: serde_json::Value,
    ) -> rim_services::workspace_state::dto::WorkspaceStateDto {
        rim_services::workspace_state::dto::WorkspaceStateDto {
            id: rim_core::workspace_state::model::WorkspaceStateId::new(),
            server_id,
            key: key.to_string(),
            value,
            scope: rim_services::workspace_state::dto::WorkspaceStateScopeDto::Server,
            user_id: None,
            version: 3,
            expires_at: None,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    /// Expects exactly one `state.incr`, answered with `value` for the requested key.
    fn expect_one_increment(workspace_state: &mut MockWorkspaceStateService, value: i64) {
        workspace_state
            .expect_increment()
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move { Ok(state_entry(cmd.server_id, &cmd.key, value.into())) })
            });
    }

    /// Expects exactly one `state.list`, answered with no entries.
    fn expect_one_empty_list(workspace_state: &mut MockWorkspaceStateService) {
        workspace_state
            .expect_list()
            .times(1)
            .returning(|_| Box::pin(async { Ok(vec![]) }));
    }

    fn pipe_cmd(actor: Actor, expression: &str) -> ExecuteCommandCmd {
        ExecuteCommandCmd {
            is_pipe: true,
            ..palette_cmd(actor, expression)
        }
    }

    fn create_cmd(actor: Actor, key: &str, step_ids: &[&str]) -> CreateUserCommandCmd {
        CreateUserCommandCmd {
            actor,
            command_key: key.to_string(),
            title: "Composite".to_string(),
            description: String::new(),
            steps: step_ids
                .iter()
                .map(|id| UserCommandStepDto {
                    command_id: id.to_string(),
                    args: None,
                })
                .collect(),
        }
    }

    fn validation_message(err: ServiceError) -> String {
        match err {
            ServiceError::Validation(msg) => msg,
            other => panic!("expected a validation error, got {other:?}"),
        }
    }

    /// Steps run in declaration order and the result payload maps each step's command
    /// id to its payload; a step without a payload adds no key.
    #[tokio::test]
    async fn user_command_steps_run_in_order_and_merge_payloads_by_step_id() {
        let actor = new_actor();
        let repo = repo_with(vec![stored_user_command(
            actor.user_id,
            "user.combo",
            vec![
                step("state.incr", serde_json::json!({ "key": "visits" })),
                step("state.list", serde_json::Value::Null),
                step("state.get", serde_json::json!({ "key": "absent" })),
            ],
        )]);
        let mut seq = mockall::Sequence::new();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state
            .expect_increment()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|cmd| {
                Box::pin(async move { Ok(state_entry(cmd.server_id, &cmd.key, 1.into())) })
            });
        workspace_state
            .expect_list()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|q| {
                Box::pin(async move { Ok(vec![state_entry(q.server_id, "visits", 1.into())]) })
            });
        workspace_state
            .expect_get()
            .times(1)
            .in_sequence(&mut seq)
            .returning(|_| Box::pin(async { Ok(None) }));
        let svc = state_service(repo, workspace_state);

        let result = svc
            .execute_command(palette_cmd(actor, "user.combo"))
            .await
            .expect("user command runs");

        assert_eq!(result.status, CommandResultStatusDto::Success);
        assert_eq!(result.message, "user command `user.combo` executed");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "state.incr": { "key": "visits", "value": 1, "version": 3 },
                "state.list": { "entries": [{ "key": "visits", "value": 1, "version": 3 }] },
            }))
        );
    }

    /// A step of a nested user command runs as the outer caller, in the outer call's
    /// workspace context, and its payload nests under the inner command's id.
    #[tokio::test]
    async fn nested_user_command_steps_run_as_the_caller_in_the_callers_context() {
        let actor = new_actor();
        let workspace = rim_core::server::model::ServerId::new();
        let repo = repo_with(vec![
            calling_command(actor.user_id, "user.outer", "user.inner"),
            bump_command(actor.user_id, "user.inner"),
        ]);
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state
            .expect_increment()
            .withf(move |cmd| cmd.actor.user_id == actor.user_id && cmd.server_id == workspace)
            .times(1)
            .returning(|cmd| {
                Box::pin(async move { Ok(state_entry(cmd.server_id, &cmd.key, 7.into())) })
            });
        let svc = state_service(repo, workspace_state);

        let result = svc
            .execute_command(ExecuteCommandCmd {
                context: Some(workspace_context(workspace)),
                ..palette_cmd(actor, "user.outer")
            })
            .await
            .expect("nested user command runs");

        assert_eq!(result.message, "user command `user.outer` executed");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "user.inner": { "state.incr": { "key": "k", "value": 7, "version": 3 } },
            }))
        );
    }

    /// Eight nested user commands are the deepest chain that still runs its final step.
    #[tokio::test]
    async fn user_command_chain_of_eight_levels_runs() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        expect_one_increment(&mut workspace_state, 1);
        let svc = state_service(
            repo_with(user_command_chain(actor.user_id, 8)),
            workspace_state,
        );

        let result = svc
            .execute_command(palette_cmd(actor, "user.level0"))
            .await
            .expect("an eight-level chain runs");

        assert_eq!(result.status, CommandResultStatusDto::Success);
    }

    /// A ninth nested user command is refused with the depth message and no step runs.
    #[tokio::test]
    async fn user_command_chain_deeper_than_eight_levels_is_refused() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_increment().never();
        let svc = state_service(
            repo_with(user_command_chain(actor.user_id, 9)),
            workspace_state,
        );

        let err = svc
            .execute_command(palette_cmd(actor, "user.level0"))
            .await
            .unwrap_err();

        assert_eq!(
            validation_message(err),
            "user command recursion depth exceeded"
        );
    }

    /// A Mutating step inside a user command is refused for a timed-out caller.
    #[tokio::test]
    async fn timed_out_actor_cannot_run_a_mutating_step_through_a_user_command() {
        let actor = new_actor();
        let until = chrono::Utc::now() + chrono::Duration::minutes(10);
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_increment().never();
        let svc = service_with_workspace_state(
            repo_with(vec![bump_command(actor.user_id, "user.bump")]),
            MockConversationService::new(),
            MockMessageService::new(),
            rim_services::capability::service::MockCapabilityService::new(),
            timed_out_moderation(until),
            workspace_state,
        );

        let err = svc
            .execute_command(palette_cmd(actor, "user.bump"))
            .await
            .unwrap_err();

        assert!(
            matches!(err, ServiceError::TimedOut { until: u } if u == until),
            "{err:?}"
        );
    }

    /// An id that is neither builtin nor one of the caller's user commands is NotFound,
    /// including a user command another user owns.
    #[tokio::test]
    async fn command_that_is_not_the_callers_user_command_is_not_found() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_increment().never();
        let svc = state_service(
            repo_with(vec![bump_command(
                rim_core::user::model::UserId::new(),
                "user.theirs",
            )]),
            workspace_state,
        );

        for command_id in ["user.theirs", "user.missing", "nope.missing"] {
            let err = svc
                .execute_command(palette_cmd(actor, command_id))
                .await
                .unwrap_err();
            assert!(
                matches!(err, ServiceError::NotFound),
                "{command_id}: {err:?}"
            );
        }
    }

    /// A user command may compose another of the caller's user commands.
    #[tokio::test]
    async fn user_command_may_reference_another_user_command() {
        let actor = new_actor();
        let mut repo = repo_with(vec![bump_command(actor.user_id, "user.inner")]);
        repo.expect_create_user_command()
            .times(1)
            .returning(|new_command| {
                Ok(UserCommand {
                    steps_json: new_command.steps_json,
                    ..stored_user_command(
                        new_command.owner_user_id,
                        &new_command.command_key,
                        vec![],
                    )
                })
            });
        let svc = validating_service(repo);

        let created = svc
            .create_user_command(create_cmd(actor, "user.outer", &["user.inner"]))
            .await
            .expect("a reference to an existing user command is accepted");

        assert_eq!(created.command_key, "user.outer");
        assert_eq!(created.steps[0].command_id, "user.inner");
    }

    /// A step naming a user command the caller does not have, or has disabled, is
    /// refused with the step named, and nothing is stored.
    #[tokio::test]
    async fn user_command_step_referencing_missing_or_disabled_command_is_refused() {
        let actor = new_actor();
        let mut repo = repo_with(vec![UserCommand {
            is_enabled: false,
            ..bump_command(actor.user_id, "user.off")
        }]);
        repo.expect_create_user_command().never();
        let svc = validating_service(repo);

        for step in ["user.ghost", "user.off"] {
            let err = svc
                .create_user_command(create_cmd(actor, "user.outer", &["state.list", step]))
                .await
                .unwrap_err();
            assert_eq!(
                validation_message(err),
                format!("unknown command step `{step}`")
            );
        }
    }

    /// A referenced user command that disappears between its definition lookup and its
    /// step validation is refused as unknown.
    #[tokio::test]
    async fn user_command_step_deleted_during_validation_is_refused() {
        let actor = new_actor();
        let inner = calling_command(actor.user_id, "user.inner", "state.list");
        let lookups = std::sync::atomic::AtomicUsize::new(0);
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user()
            .returning(move |_, command_key| {
                if command_key != "user.inner" {
                    return Ok(None);
                }
                let n = lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok((n == 0).then(|| inner.clone()))
            });
        repo.expect_create_user_command().never();
        let svc = validating_service(repo);

        let err = svc
            .create_user_command(create_cmd(actor, "user.outer", &["user.inner"]))
            .await
            .unwrap_err();

        assert_eq!(validation_message(err), "unknown command step `user.inner`");
    }

    /// Updating a user command so that it closes a loop through other user commands
    /// is refused as a cycle, and nothing is stored.
    #[tokio::test]
    async fn user_command_update_closing_a_cycle_is_refused() {
        let actor = new_actor();
        let mut repo = repo_with(vec![
            calling_command(actor.user_id, "user.a", "user.b"),
            calling_command(actor.user_id, "user.b", "user.c"),
            calling_command(actor.user_id, "user.c", "state.list"),
        ]);
        repo.expect_update_user_command().never();
        let svc = validating_service(repo);

        let err = svc
            .update_user_command(UpdateUserCommandCmd {
                actor,
                command_id: "user.c".to_string(),
                title: "C".to_string(),
                description: String::new(),
                steps: vec![UserCommandStepDto {
                    command_id: "user.a".to_string(),
                    args: None,
                }],
                is_enabled: true,
            })
            .await
            .unwrap_err();

        assert_eq!(
            validation_message(err),
            "user command recursion cycle detected"
        );
    }

    /// User commands compose backend commands only: a desktop-local step is refused,
    /// directly or inside a referenced user command.
    #[tokio::test]
    async fn user_command_with_a_desktop_local_step_is_refused() {
        let actor = new_actor();
        let mut repo = repo_with(vec![calling_command(
            actor.user_id,
            "user.scroll",
            "chat.scroll_to_bottom",
        )]);
        repo.expect_create_user_command().never();
        let svc = validating_service(repo);

        for step in ["chat.scroll_to_bottom", "user.scroll"] {
            let err = svc
                .create_user_command(create_cmd(actor, "user.outer", &[step]))
                .await
                .unwrap_err();
            assert_eq!(
                validation_message(err),
                "user commands may only compose backend commands in v1",
                "{step}"
            );
        }
    }

    /// A failing pipe stage ends the pipe with an Error result: the earlier stages and
    /// the failed one are reported, later stages never run, and a missing-arg message
    /// lists the keys the previous stage produced.
    #[tokio::test]
    async fn pipe_stage_missing_arg_reports_keys_of_previous_stage() {
        let mut workspace_state = MockWorkspaceStateService::new();
        expect_one_increment(&mut workspace_state, 2);
        expect_one_empty_list(&mut workspace_state);
        workspace_state.expect_get().never();
        workspace_state.expect_delete().never();
        let svc = state_service(repo_with(vec![]), workspace_state);

        let result = svc
            .execute_command(pipe_cmd(
                new_actor(),
                "state.incr key=visits | state.list | state.get | state.delete key=visits",
            ))
            .await
            .expect("a failing stage is reported in the result");

        let expected = "pipe stage 3 ('state.get') missing string arg `key` \
                        — the previous stage ('state.list') produced: {entries}";
        assert_eq!(result.status, CommandResultStatusDto::Error);
        assert_eq!(result.message, expected);
        let stages = result.pipeline_stages.expect("stages are reported");
        let summary: Vec<_> = stages
            .iter()
            .map(|s| (s.stage_index, s.command_id.as_str(), s.status))
            .collect();
        assert_eq!(
            summary,
            vec![
                (0, "state.incr", CommandResultStatusDto::Success),
                (1, "state.list", CommandResultStatusDto::Success),
                (2, "state.get", CommandResultStatusDto::Error),
            ]
        );
        assert_eq!(
            stages[1].payload,
            Some(serde_json::json!({ "entries": [] }))
        );
        assert_eq!(stages[2].message, expected);
        assert_eq!(stages[2].payload, None);
        let payload = result.payload.expect("successful stages are kept");
        let steps: Vec<_> = payload.as_object().expect("object").keys().collect();
        assert_eq!(steps, vec!["step_1", "step_2"]);
    }

    /// A missing arg in the first stage, or after a stage that produced no payload,
    /// lists "(none)" as the previous stage's keys.
    #[tokio::test]
    async fn pipe_stage_missing_arg_without_previous_payload_reports_none() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state
            .expect_get()
            .times(1)
            .returning(|_| Box::pin(async { Ok(None) }));
        workspace_state.expect_increment().never();
        workspace_state.expect_list().never();
        let svc = state_service(repo_with(vec![]), workspace_state);

        let first = svc
            .execute_command(pipe_cmd(actor, "state.get | state.list"))
            .await
            .expect("a failing stage is reported in the result");
        assert_eq!(first.status, CommandResultStatusDto::Error);
        assert_eq!(
            first.message,
            "pipe stage 1 ('state.get') missing string arg `key` \
             — the previous stage produced: {(none)}"
        );
        assert_eq!(first.pipeline_stages.map(|s| s.len()), Some(1));

        let after_empty = svc
            .execute_command(pipe_cmd(actor, "state.get key=absent | state.incr"))
            .await
            .expect("a failing stage is reported in the result");
        assert_eq!(after_empty.status, CommandResultStatusDto::Error);
        assert_eq!(
            after_empty.message,
            "pipe stage 2 ('state.incr') missing string arg `key` \
             — the previous stage ('state.get') produced: {(none)}"
        );
    }

    /// A service error in a pipe stage is reported with the stage index and command id.
    #[tokio::test]
    async fn pipe_stage_service_error_names_the_stage() {
        let mut workspace_state = MockWorkspaceStateService::new();
        expect_one_empty_list(&mut workspace_state);
        workspace_state.expect_increment().times(1).returning(|_| {
            Box::pin(async { Err(ServiceError::Conflict("version mismatch".to_string())) })
        });
        let svc = state_service(repo_with(vec![]), workspace_state);

        let result = svc
            .execute_command(pipe_cmd(new_actor(), "state.list | state.incr key=k"))
            .await
            .expect("a failing stage is reported in the result");

        assert_eq!(result.status, CommandResultStatusDto::Error);
        assert_eq!(
            result.message,
            "pipe stage 2 ('state.incr') failed: version mismatch"
        );
        assert_eq!(result.pipeline_stages.map(|s| s.len()), Some(2));
    }

    /// A pipe naming an unknown or desktop-local stage is refused before any stage runs.
    #[tokio::test]
    async fn pipe_with_unknown_or_desktop_local_stage_runs_nothing() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_list().never();
        let svc = state_service(repo_with(vec![]), workspace_state);

        let unknown = svc
            .execute_command(pipe_cmd(actor, "state.list | user.ghost"))
            .await
            .unwrap_err();
        assert_eq!(
            validation_message(unknown),
            "pipe stage 2 ('user.ghost') refers to an unknown command"
        );

        let desktop = svc
            .execute_command(pipe_cmd(actor, "state.list | chat.scroll_to_bottom"))
            .await
            .unwrap_err();
        assert!(
            validation_message(desktop)
                .ends_with("stage 2 ('chat.scroll_to_bottom') is desktop-local"),
        );
    }

    /// A user command can be a pipe stage; its merged payload feeds the next stage.
    #[tokio::test]
    async fn pipe_stage_may_be_a_user_command() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        expect_one_increment(&mut workspace_state, 1);
        workspace_state.expect_get().never();
        let svc = state_service(
            repo_with(vec![bump_command(actor.user_id, "user.bump")]),
            workspace_state,
        );

        let result = svc
            .execute_command(pipe_cmd(actor, "user.bump | state.get"))
            .await
            .expect("a failing stage is reported in the result");

        assert_eq!(
            result.message,
            "pipe stage 2 ('state.get') missing string arg `key` \
             — the previous stage ('user.bump') produced: {state.incr}"
        );
        let stages = result.pipeline_stages.expect("stages are reported");
        assert_eq!(stages[0].status, CommandResultStatusDto::Success);
        assert_eq!(stages[0].message, "user command `user.bump` executed");
    }

    /// A cycle stored in the repository is refused as a cycle wherever definitions are
    /// resolved: execution, its capability gate, listing and lookup.
    #[tokio::test]
    async fn stored_user_command_cycle_is_refused_when_resolving_definitions() {
        let actor = new_actor();
        let svc = validating_service(repo_with(vec![
            calling_command(actor.user_id, "user.a", "user.b"),
            calling_command(actor.user_id, "user.b", "user.a"),
        ]));
        let cycle = "user command recursion cycle detected";

        let executed = svc
            .execute_command(palette_cmd(actor, "user.a"))
            .await
            .unwrap_err();
        assert_eq!(validation_message(executed), cycle);

        let listed = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: None,
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .unwrap_err();
        assert_eq!(validation_message(listed), cycle);

        let looked_up = svc
            .get_command(GetCommandQuery {
                actor,
                command_id: "user.b".to_string(),
            })
            .await
            .unwrap_err();
        assert_eq!(validation_message(looked_up), cycle);
    }
}
