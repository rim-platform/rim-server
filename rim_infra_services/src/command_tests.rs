#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use async_trait::async_trait;

    use rim_core::command::model::{NewUserCommand, UpdateUserCommand, UserCommand, UserCommandId};
    use rim_core::command::repository::UserCommandRepository;
    use rim_core::repository::error::RepoResult;
    use rim_core::server_member::model::ServerMemberRole;
    use rim_services::command::dto::{
        CommandContextDto, CommandDefinitionDto, CommandExecutionTargetDto, CommandResultStatusDto,
        CommandSourceKindDto, CommandSurfaceDto, UserCommandStepDto,
    };
    use rim_services::command::initiator::CommandInitiator;
    use rim_services::command::request::{
        CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, GetCommandQuery,
        GetUserCommandQuery, ListCommandsQuery, ListUserCommandsQuery, UpdateUserCommandCmd,
    };
    use rim_services::command::sensitivity::CommandSensitivityDto;
    use rim_services::command::service::CommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;
    use rim_services::{Actor, ServiceError};

    use crate::CommandRepos;
    use crate::audit_capture::{capture_audit_lines, drain_buf};
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
        service_with_capability(repo, conversation_service, message_service, full_caps())
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
                    rim_services::server_member::service::MockServerMemberService::with_own_role(
                        Some(rim_core::server_member::model::ServerMemberRole::Owner),
                    ),
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
                context: Some(workspace_context(rim_core::server::model::ServerId::new())),
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
    async fn create_user_command_refuses_an_id_with_other_characters() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .create_user_command(CreateUserCommandCmd {
                actor,
                command_key: "user.Bad ID!".to_string(),
                title: "Bad".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.post_message".to_string(),
                    args: None,
                }],
            })
            .await
            .unwrap_err();

        assert!(
            matches!(
                &err,
                ServiceError::Validation(message)
                    if message == "a user command id may use only lowercase letters, numbers, dots, dashes or underscores after `user.`"
            ),
            "{err:?}"
        );
    }

    #[tokio::test]
    async fn update_user_command_keeps_working_for_a_stored_id_with_capitals() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let existing = UserCommand {
            id: rim_core::command::model::UserCommandId::new(),
            owner_user_id: actor.user_id,
            command_key: "user.Old".to_string(),
            title: "Old".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!([{ "command_id": "chat.create_group", "args": null }]),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        };
        let updated = UserCommand {
            title: "Old Updated".to_string(),
            ..existing.clone()
        };
        let mut repo = MockUserCommandRepository::new();
        let existing_for_lookup = existing.clone();
        repo.expect_get_by_key_for_user()
            .return_once(move |_, command_key| {
                assert_eq!(command_key, "user.Old");
                Ok(Some(existing_for_lookup))
            });
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let updated_for_return = updated.clone();
        repo.expect_update_user_command()
            .return_once(move |_, _| Ok(updated_for_return));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let result = svc
            .update_user_command(UpdateUserCommandCmd {
                actor,
                command_id: "user.Old".to_string(),
                title: "Old Updated".to_string(),
                description: "desc".to_string(),
                steps: vec![UserCommandStepDto {
                    command_id: "chat.create_group".to_string(),
                    args: None,
                }],
                is_enabled: true,
            })
            .await
            .unwrap();
        assert_eq!(result.title, "Old Updated");
        assert_eq!(result.command_key, "user.Old");
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
                Box::pin(async move { Ok(sent_message(cmd)) })
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
            .filter_ai_surface_test_only(
                &actor,
                Some(&workspace_context(rim_core::server::model::ServerId::new())),
                input,
            )
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
            .filter_ai_surface_test_only(
                &actor,
                Some(&workspace_context(rim_core::server::model::ServerId::new())),
                input,
            )
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
            .filter_ai_surface_test_only(
                &actor,
                Some(&workspace_context(rim_core::server::model::ServerId::new())),
                only_one_held,
            )
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
            .filter_ai_surface_test_only(
                &actor,
                Some(&workspace_context(rim_core::server::model::ServerId::new())),
                both,
            )
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
            .filter_ai_surface_test_only(
                &actor,
                Some(&workspace_context(rim_core::server::model::ServerId::new())),
                input,
            )
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
            full_caps(),
        );

        let palette = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(workspace_context(rim_core::server::model::ServerId::new())),
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
    async fn list_commands_ai_surface_returns_forty_two_commands_with_caps() {
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
                "chat.set_channel_posting",
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
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
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

    fn saved_user_command(
        owner_user_id: rim_core::user::model::UserId,
        command_key: &str,
        is_enabled: bool,
    ) -> UserCommand {
        UserCommand {
            id: UserCommandId::new(),
            owner_user_id,
            command_key: command_key.to_string(),
            title: "Stored".to_string(),
            description: "desc".to_string(),
            steps_json: serde_json::json!([
                { "command_id": "chat.create_group", "args": null },
                { "command_id": "chat.post_message", "args": { "body": "hi" } }
            ]),
            is_enabled,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn get_user_command_returns_own_command_with_steps_in_order() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let stored = saved_user_command(actor_user_id, "user.greet", false);
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, command_key| {
                assert_eq!(owner_user_id, actor_user_id);
                assert_eq!(command_key, "user.greet");
                Ok(Some(stored))
            });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let command = svc
            .get_user_command(GetUserCommandQuery {
                actor,
                command_id: " user.greet ".to_string(),
            })
            .await
            .unwrap()
            .expect("own user command is returned");

        assert_eq!(command.command_key, "user.greet");
        assert!(!command.is_enabled);
        let steps: Vec<&str> = command
            .steps
            .iter()
            .map(|step| step.command_id.as_str())
            .collect();
        assert_eq!(steps, ["chat.create_group", "chat.post_message"]);
        assert_eq!(
            command.steps[1].args,
            Some(serde_json::json!({ "body": "hi" }))
        );
    }

    #[tokio::test]
    async fn get_user_command_answers_none_for_another_users_command() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let mut repo = MockUserCommandRepository::new();
        // The repository scopes the lookup to the owner, so another user's command of the
        // same key is never found for the actor.
        repo.expect_get_by_key_for_user()
            .return_once(move |owner_user_id, _| {
                assert_eq!(owner_user_id, actor_user_id);
                Ok(None)
            });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let command = svc
            .get_user_command(GetUserCommandQuery {
                actor,
                command_id: "user.someone_elses".to_string(),
            })
            .await
            .unwrap();

        assert!(command.is_none());
    }

    #[tokio::test]
    async fn get_user_command_answers_none_for_builtin_id_without_lookup() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user().never();
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let command = svc
            .get_user_command(GetUserCommandQuery {
                actor,
                command_id: "chat.post_message".to_string(),
            })
            .await
            .unwrap();

        assert!(command.is_none());
    }

    #[tokio::test]
    async fn get_user_command_surfaces_unreadable_steps_as_internal() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut stored = saved_user_command(actor.user_id, "user.broken", true);
        stored.steps_json = serde_json::json!("not an array");
        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user()
            .return_once(move |_, _| Ok(Some(stored)));
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let err = svc
            .get_user_command(GetUserCommandQuery {
                actor,
                command_id: "user.broken".to_string(),
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Internal(_)));
    }

    #[tokio::test]
    async fn list_user_commands_returns_own_commands_sorted_including_disabled() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;
        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user()
            .return_once(move |owner_user_id| {
                assert_eq!(owner_user_id, actor_user_id);
                Ok(vec![
                    saved_user_command(owner_user_id, "user.zeta", true),
                    saved_user_command(owner_user_id, "user.alpha", false),
                ])
            });
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let commands = svc
            .list_user_commands(ListUserCommandsQuery { actor })
            .await
            .unwrap();

        let keys: Vec<&str> = commands
            .iter()
            .map(|command| command.command_key.as_str())
            .collect();
        assert_eq!(keys, ["user.alpha", "user.zeta"]);
        assert!(!commands[0].is_enabled);
        assert_eq!(commands[0].steps.len(), 2);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn execute_command_emits_audit_line_on_success() {
        let (_guard, buf) = capture_audit_lines();

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let actor_user_id = actor.user_id;

        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Ok(()));

        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| Box::pin(async move { Ok(sent_message(cmd)) }));

        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc =
            service_with_capability(repo, MockConversationService::new(), message_service, cap);

        let server_id = rim_core::server::model::ServerId::new();
        let conversation_id = rim_core::conversation::model::ConversationId::new();

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
        let (_guard, buf) = capture_audit_lines();

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
        let (_guard, buf) = capture_audit_lines();

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check()
            .withf(|_actor, _ws, cap| cap == "chat.write")
            .returning(|_, _, _| Ok(()));

        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .return_once(move |cmd| Box::pin(async move { Ok(sent_message(cmd)) }));

        let mut repo = MockUserCommandRepository::new();
        repo.expect_list_for_user().returning(|_| Ok(vec![]));
        let svc =
            service_with_capability(repo, MockConversationService::new(), message_service, cap);

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
        let svc = listing_service(
            rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                ServerMemberRole::Owner,
            )),
            cap,
            vec![],
        );
        if let Some(macro_svc) = macro_svc {
            let _ = svc.set_macro_service(macro_svc);
        }
        svc
    }

    /// A command service for listing: `member` answers the caller's lookup of their own
    /// membership, `cap` the capability checks, and the caller owns `user_commands`.
    fn listing_service(
        member: rim_services::server_member::service::MockServerMemberService,
        cap: rim_services::capability::service::MockCapabilityService,
        user_commands: Vec<UserCommand>,
    ) -> DefaultCommandService {
        let mut repo = MockUserCommandRepository::new();
        let listed = user_commands.clone();
        repo.expect_list_for_user()
            .returning(move |_| Ok(listed.clone()));
        repo.expect_get_by_key_for_user().returning(move |_, key| {
            Ok(user_commands.iter().find(|c| c.command_key == key).cloned())
        });

        DefaultCommandService::new(
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
                server_member_service: Arc::new(member),
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
        )
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
            .command_discover(
                &actor,
                None,
                None,
                Some(rim_core::server::model::ServerId::new()),
            )
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
            .command_discover(
                &actor,
                None,
                None,
                Some(rim_core::server::model::ServerId::new()),
            )
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
            .command_discover(
                &actor,
                None,
                None,
                Some(rim_core::server::model::ServerId::new()),
            )
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
            .command_discover(
                &actor,
                Some("post".to_string()),
                None,
                Some(rim_core::server::model::ServerId::new()),
            )
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

    /// `repo_with(commands)` that also stores every create and update it receives.
    fn saving_repo(commands: Vec<UserCommand>) -> MockUserCommandRepository {
        let owner = commands
            .first()
            .map_or_else(rim_core::user::model::UserId::new, |c| c.owner_user_id);
        let mut repo = repo_with(commands);
        repo.expect_create_user_command().returning(|new_command| {
            Ok(UserCommand {
                steps_json: new_command.steps_json,
                ..stored_user_command(new_command.owner_user_id, &new_command.command_key, vec![])
            })
        });
        repo.expect_update_user_command()
            .returning(move |_, update| {
                Ok(UserCommand {
                    steps_json: update.steps_json,
                    ..stored_user_command(owner, "user.updated", vec![])
                })
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

    fn step_dtos(step_ids: &[&str]) -> Vec<UserCommandStepDto> {
        step_ids
            .iter()
            .map(|id| UserCommandStepDto {
                command_id: id.to_string(),
                args: None,
            })
            .collect()
    }

    fn create_cmd(actor: Actor, key: &str, step_ids: &[&str]) -> CreateUserCommandCmd {
        CreateUserCommandCmd {
            actor,
            command_key: key.to_string(),
            title: "Composite".to_string(),
            description: String::new(),
            steps: step_dtos(step_ids),
        }
    }

    fn update_cmd(actor: Actor, key: &str, step_ids: &[&str]) -> UpdateUserCommandCmd {
        UpdateUserCommandCmd {
            actor,
            command_id: key.to_string(),
            title: "Composite".to_string(),
            description: String::new(),
            steps: step_dtos(step_ids),
            is_enabled: true,
        }
    }

    /// Asserts a create or update was accepted, or refused with the depth message.
    fn assert_depth_outcome<T: std::fmt::Debug>(result: Result<T, ServiceError>, accepted: bool) {
        match result {
            Ok(_) => assert!(accepted, "a chain over the depth limit was accepted"),
            Err(err) => {
                assert!(
                    !accepted,
                    "a chain within the depth limit was refused: {err:?}"
                );
                assert_eq!(
                    validation_message(err),
                    "user command recursion depth exceeded"
                );
            }
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

    /// A disabled user command does not run: it is NotFound, as an unknown id is.
    #[tokio::test]
    async fn disabled_user_command_does_not_run() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_increment().never();
        let svc = state_service(
            repo_with(vec![UserCommand {
                is_enabled: false,
                ..bump_command(actor.user_id, "user.off")
            }]),
            workspace_state,
        );

        let err = svc
            .execute_command(palette_cmd(actor, "user.off"))
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::NotFound), "{err:?}");
    }

    /// A user command may compose another of the caller's user commands.
    #[tokio::test]
    async fn user_command_may_reference_another_user_command() {
        let actor = new_actor();
        let svc = validating_service(saving_repo(vec![bump_command(actor.user_id, "user.inner")]));

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
            .update_user_command(update_cmd(actor, "user.c", &["user.a"]))
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

    /// A cycle stored in the repository is refused as a cycle wherever a definition is
    /// resolved, execution and lookup, and the listing leaves its commands out.
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
            .expect("the listing leaves the cycle out");
        assert!(listed.iter().all(|c| !c.id.starts_with("user.")));

        let looked_up = svc
            .get_command(GetCommandQuery {
                actor,
                command_id: "user.b".to_string(),
            })
            .await
            .unwrap_err();
        assert_eq!(validation_message(looked_up), cycle);
    }

    /// Creating a user command on top of a stored chain is refused once the chain would
    /// exceed eight user commands.
    #[tokio::test]
    async fn create_user_command_refuses_a_chain_deeper_than_eight_levels() {
        let actor = new_actor();
        for (stored_levels, accepted) in [(7, true), (8, false)] {
            let svc = validating_service(saving_repo(user_command_chain(
                actor.user_id,
                stored_levels,
            )));

            let result = svc
                .create_user_command(create_cmd(actor, "user.top", &["user.level0"]))
                .await;

            assert_depth_outcome(result, accepted);
        }
    }

    /// An update is refused when it would make any chain through the updated command
    /// exceed eight user commands, whether the chain grows below it or above it.
    #[tokio::test]
    async fn update_user_command_refuses_a_chain_deeper_than_eight_levels() {
        let actor = new_actor();
        for (stored_levels, accepted) in [(7, true), (8, false)] {
            let mut commands = user_command_chain(actor.user_id, stored_levels);
            commands.push(calling_command(actor.user_id, "user.top", "state.list"));
            commands.push(bump_command(actor.user_id, "user.extra"));
            let svc = validating_service(saving_repo(commands));

            let below = svc
                .update_user_command(update_cmd(actor, "user.top", &["user.level0"]))
                .await;
            assert_depth_outcome(below, accepted);

            let last_level = format!("user.level{}", stored_levels - 1);
            let above = svc
                .update_user_command(update_cmd(actor, &last_level, &["user.extra"]))
                .await;
            assert_depth_outcome(above, accepted);
        }
    }

    /// `repo_with(commands)` that counts every read: each lookup by key and each listing.
    fn counting_repo(
        commands: Vec<UserCommand>,
    ) -> (
        MockUserCommandRepository,
        Arc<std::sync::atomic::AtomicUsize>,
    ) {
        let reads = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let mut repo = MockUserCommandRepository::new();
        let (looked_up, listed) = (commands.clone(), commands);
        let lookups = Arc::clone(&reads);
        repo.expect_get_by_key_for_user()
            .returning(move |owner_user_id, command_key| {
                lookups.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                Ok(looked_up
                    .iter()
                    .find(|c| c.owner_user_id == owner_user_id && c.command_key == command_key)
                    .cloned())
            });
        let listings = Arc::clone(&reads);
        repo.expect_list_for_user().returning(move |owner_user_id| {
            listings.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            Ok(listed
                .iter()
                .filter(|c| c.owner_user_id == owner_user_id)
                .cloned()
                .collect())
        });
        (repo, reads)
    }

    fn read_count(reads: &std::sync::atomic::AtomicUsize) -> usize {
        reads.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A user command of `owner` with `copies` steps, each `target`.
    fn repeating_command(
        owner: rim_core::user::model::UserId,
        command_key: &str,
        target: &str,
        copies: usize,
    ) -> UserCommand {
        stored_user_command(
            owner,
            command_key,
            vec![step(target, serde_json::Value::Null); copies],
        )
    }

    /// `user.level0` .. `user.level{levels - 1}`, each naming the next `fan_out` times, the
    /// last one listing state `fan_out` times: `fan_out` to the power `levels` builtin steps.
    fn fanned_out_levels(
        owner: rim_core::user::model::UserId,
        levels: usize,
        fan_out: usize,
    ) -> Vec<UserCommand> {
        (0..levels)
            .map(|level| {
                let target = if level + 1 == levels {
                    "state.list".to_string()
                } else {
                    format!("user.level{}", level + 1)
                };
                repeating_command(owner, &format!("user.level{level}"), &target, fan_out)
            })
            .collect()
    }

    /// A user command named many times by the commands above it is read and derived once:
    /// four stored levels of ten references each cost four reads, not one per reference,
    /// and a run of the top level, a thousand builtin steps, is refused before any runs.
    #[tokio::test]
    async fn repeated_references_are_read_once_and_an_oversized_run_runs_nothing() {
        let actor = new_actor();
        let (repo, reads) = counting_repo(fanned_out_levels(actor.user_id, 4, 10));
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_list().never();
        let svc = state_service(repo, workspace_state);
        let too_many = "a user command expands to more than 100 steps";

        let looked_up = svc
            .get_command(GetCommandQuery {
                actor,
                command_id: "user.level0".to_string(),
            })
            .await
            .unwrap_err();
        assert_eq!(validation_message(looked_up), too_many);
        assert!(read_count(&reads) <= 4, "{} reads", read_count(&reads));

        let before = read_count(&reads);
        let executed = svc
            .execute_command(palette_cmd(actor, "user.level0"))
            .await
            .unwrap_err();
        assert_eq!(validation_message(executed), too_many);
        assert!(
            read_count(&reads) - before <= 4,
            "{} reads",
            read_count(&reads) - before
        );
    }

    /// A user command that expands to exactly the limit resolves with one read per stored
    /// command and runs every one of its builtin steps.
    #[tokio::test]
    async fn a_user_command_expanding_to_the_limit_runs_every_step() {
        let actor = new_actor();
        let (repo, reads) = counting_repo(fanned_out_levels(actor.user_id, 2, 10));
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state
            .expect_list()
            .times(100)
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        let svc = state_service(repo, workspace_state);

        let result = svc
            .execute_command(palette_cmd(actor, "user.level0"))
            .await
            .expect("a run of 100 builtin steps is allowed");

        assert_eq!(result.status, CommandResultStatusDto::Success);
        assert_eq!(read_count(&reads), 2);
    }

    /// Creating a user command over stored levels that would expand past the limit is
    /// refused after one lookup and one listing, and nothing is stored.
    #[tokio::test]
    async fn create_user_command_expanding_past_the_limit_is_refused_with_bounded_reads() {
        let actor = new_actor();
        let mut stored = fanned_out_levels(actor.user_id, 4, 10);
        stored.remove(0);
        let (mut repo, reads) = counting_repo(stored);
        repo.expect_create_user_command().never();
        let svc = validating_service(repo);

        let err = svc
            .create_user_command(create_cmd(actor, "user.top", &["user.level1"]))
            .await
            .unwrap_err();

        assert_eq!(
            validation_message(err),
            "a user command expands to more than 100 steps"
        );
        assert_eq!(read_count(&reads), 2);
    }

    /// A user command lists at most 20 steps; more is refused before the repository is read.
    #[tokio::test]
    async fn user_command_with_more_than_twenty_steps_is_refused_before_any_read() {
        let actor = new_actor();
        let svc = validating_service(MockUserCommandRepository::new());
        let steps = ["state.list"; 21];
        let too_many = "a user command may have at most 20 steps";

        let created = svc
            .create_user_command(create_cmd(actor, "user.long", &steps))
            .await
            .unwrap_err();
        assert_eq!(validation_message(created), too_many);

        let updated = svc
            .update_user_command(update_cmd(actor, "user.long", &steps))
            .await
            .unwrap_err();
        assert_eq!(validation_message(updated), too_many);

        let svc = validating_service(saving_repo(vec![]));
        svc.create_user_command(create_cmd(actor, "user.long", &steps[..20]))
            .await
            .expect("20 steps are allowed");
    }

    /// An update to a command another stored command calls is refused when it would push
    /// that caller past the limit, and nothing is stored.
    #[tokio::test]
    async fn update_pushing_a_stored_caller_past_the_limit_is_refused() {
        let actor = new_actor();
        let commands = vec![
            repeating_command(actor.user_id, "user.outer", "user.inner", 10),
            repeating_command(actor.user_id, "user.middle", "user.inner", 1),
            repeating_command(actor.user_id, "user.top", "user.middle", 1),
            repeating_command(actor.user_id, "user.inner", "state.list", 10),
        ];
        let mut repo = repo_with(commands.clone());
        repo.expect_update_user_command().never();
        let svc = validating_service(repo);

        let err = svc
            .update_user_command(update_cmd(actor, "user.inner", &["state.list"; 11]))
            .await
            .unwrap_err();
        assert_eq!(
            validation_message(err),
            "a user command expands to more than 100 steps"
        );

        let svc = validating_service(saving_repo(commands));
        svc.update_user_command(update_cmd(actor, "user.inner", &["state.get"; 10]))
            .await
            .expect("an update keeping every caller within the limit is accepted");
    }

    /// Two steps reaching one shared user command (A calls B and C, both call D) are not a
    /// cycle: D is read once, its definition reused, and the run performs D's step twice.
    #[tokio::test]
    async fn a_user_command_shared_by_two_steps_is_not_a_cycle() {
        let actor = new_actor();
        let commands = vec![
            stored_user_command(
                actor.user_id,
                "user.a",
                vec![
                    step("user.b", serde_json::Value::Null),
                    step("user.c", serde_json::Value::Null),
                ],
            ),
            calling_command(actor.user_id, "user.b", "user.d"),
            calling_command(actor.user_id, "user.c", "user.d"),
            bump_command(actor.user_id, "user.d"),
        ];
        let (repo, reads) = counting_repo(commands.clone());
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state
            .expect_increment()
            .times(2)
            .returning(|cmd| {
                Box::pin(async move { Ok(state_entry(cmd.server_id, &cmd.key, 1.into())) })
            });
        let svc = state_service(repo, workspace_state);

        let definition = svc
            .get_command(GetCommandQuery {
                actor,
                command_id: "user.a".to_string(),
            })
            .await
            .expect("a shared callee resolves")
            .expect("user.a is the caller's");
        assert_eq!(definition.id, "user.a");
        assert_eq!(read_count(&reads), 4);

        svc.execute_command(palette_cmd(actor, "user.a"))
            .await
            .expect("a shared callee runs once per step naming it");

        let svc = validating_service(saving_repo(commands));
        svc.create_user_command(create_cmd(actor, "user.e", &["user.b", "user.c"]))
            .await
            .expect("a new command sharing a callee is accepted");
    }

    /// A pipe whose stages together perform more builtin steps than the limit is refused
    /// before any stage runs, though each stage alone is within it.
    #[tokio::test]
    async fn pipe_expanding_past_the_limit_runs_no_stage() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        workspace_state.expect_list().never();
        workspace_state.expect_get().never();
        let svc = state_service(
            repo_with(fanned_out_levels(actor.user_id, 2, 10)),
            workspace_state,
        );

        let err = svc
            .execute_command(pipe_cmd(actor, "user.level0 | state.get key=k"))
            .await
            .unwrap_err();

        assert_eq!(
            validation_message(err),
            "a pipe expands to more than 100 steps"
        );
    }

    /// A user keeps at most 100 user commands: the 101st is refused, and editing one of the
    /// hundred still works.
    #[tokio::test]
    async fn a_user_with_a_hundred_user_commands_cannot_create_another() {
        let actor = new_actor();
        let commands: Vec<UserCommand> = (0..100)
            .map(|n| bump_command(actor.user_id, &format!("user.c{n}")))
            .collect();
        let svc = validating_service(saving_repo(commands));

        let err = svc
            .create_user_command(create_cmd(actor, "user.one_more", &["state.list"]))
            .await
            .unwrap_err();
        assert_eq!(
            validation_message(err),
            "a user may have at most 100 user commands"
        );

        svc.update_user_command(update_cmd(actor, "user.c7", &["state.list"]))
            .await
            .expect("an existing command can still be edited");
    }

    /// The listing leaves out a stored user command that no longer resolves, one naming an
    /// unknown step or expanding past the limit, and lists the others.
    #[tokio::test]
    async fn list_commands_leaves_out_a_user_command_that_does_not_resolve() {
        let actor = new_actor();
        let mut commands = fanned_out_levels(actor.user_id, 3, 10);
        commands.push(calling_command(actor.user_id, "user.ghostly", "user.ghost"));
        commands.push(bump_command(actor.user_id, "user.good"));
        let svc = service(
            repo_with(commands),
            MockConversationService::new(),
            MockMessageService::new(),
        );

        let listed = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(workspace_context(rim_core::server::model::ServerId::new())),
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .expect("one bad command does not fail the listing");

        let user_ids: Vec<&str> = listed
            .iter()
            .map(|c| c.id.as_str())
            .filter(|id| id.starts_with("user."))
            .collect();
        assert_eq!(user_ids, vec!["user.good", "user.level1", "user.level2"]);
    }

    // ── Moderation and selected-message commands ──

    const ADMIN_TARGET_COMMANDS: [&str; 5] = [
        "admin.kick",
        "admin.ban",
        "admin.unban",
        "admin.timeout",
        "admin.set_role",
    ];

    /// A command service whose moderation mock reports no timeouts and carries the
    /// expectations `expect` sets; every capability is granted.
    fn moderating(
        expect: impl FnOnce(&mut rim_services::moderation::service::MockModerationService),
    ) -> DefaultCommandService {
        let mut moderation =
            rim_services::moderation::service::MockModerationService::no_timeouts();
        expect(&mut moderation);
        service_with_moderation(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
            allow_all_caps(),
            moderation,
        )
    }

    /// A command service over `message_service`; every capability is granted.
    fn messaging(message_service: MockMessageService) -> DefaultCommandService {
        service_with_capability(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            message_service,
            allow_all_caps(),
        )
    }

    /// Runs `command_id` from the palette as a fresh actor and, when the command asks for
    /// confirmation, re-submits it with the issued token as the user confirming would.
    async fn run_confirmed(
        svc: &DefaultCommandService,
        command_id: &str,
        args: serde_json::Value,
        context: Option<CommandContextDto>,
    ) -> Result<rim_services::command::dto::CommandResultDto, ServiceError> {
        run_confirmed_as(svc, new_actor(), command_id, args, context).await
    }

    /// [`run_confirmed`] as `actor`.
    async fn run_confirmed_as(
        svc: &DefaultCommandService,
        actor: Actor,
        command_id: &str,
        args: serde_json::Value,
        context: Option<CommandContextDto>,
    ) -> Result<rim_services::command::dto::CommandResultDto, ServiceError> {
        let cmd = ExecuteCommandCmd {
            actor,
            command_id: command_id.to_string(),
            args: Some(args),
            context,
            confirmation: None,
            is_pipe: false,
            initiator: CommandInitiator::Palette,
        };
        let first = svc.execute_command(cmd.clone()).await?;
        let Some(challenge) = first.confirmation_required else {
            return Ok(first);
        };
        svc.execute_command(ExecuteCommandCmd {
            confirmation: Some(
                rim_services::command::confirmation::ConfirmationProof::InBandToken {
                    token: challenge.token,
                    args_hash: challenge.args_hash,
                },
            ),
            ..cmd
        })
        .await
    }

    /// A context with a workspace, a focused conversation and a selected message.
    fn selected_message_context(
        workspace: rim_core::server::model::ServerId,
        conversation: rim_core::conversation::model::ConversationId,
        message: rim_core::message::model::MessageId,
    ) -> CommandContextDto {
        CommandContextDto {
            conversation_id: Some(conversation),
            selected_message_id: Some(message),
            ..workspace_context(workspace)
        }
    }

    /// A live text message in `conversation_id` with fresh message and author ids.
    fn message_in(
        server_id: rim_core::server::model::ServerId,
        conversation_id: rim_core::conversation::model::ConversationId,
    ) -> MessageDto {
        MessageDto {
            id: rim_core::message::model::MessageId::new(),
            server_id,
            conversation_id,
            thread_id: None,
            root_thread_id: None,
            author_id: rim_core::user::model::UserId::new(),
            text: Some("hello".to_string()),
            has_attachments: false,
            attachments: vec![],
            parent_message_id: None,
            forwarded_from_message_id: None,
            thread_root_message_id: None,
            is_deleted: false,
            sent_at: chrono::Utc::now(),
            encryption: None,
        }
    }

    /// The message `send_message` stores for `cmd`.
    fn sent_message(cmd: rim_services::message::command::SendMessageCmd) -> MessageDto {
        MessageDto {
            author_id: cmd.actor.user_id,
            text: cmd.text,
            parent_message_id: cmd.parent_message_id,
            ..message_in(cmd.server_id, cmd.conversation_id)
        }
    }

    /// Kick, ban and set-role stop at a confirmation challenge before reaching the
    /// moderation service.
    #[tokio::test]
    async fn kick_ban_and_set_role_commands_ask_for_confirmation_first() {
        let svc = moderating(|_| {});
        for command_id in ["admin.kick", "admin.ban", "admin.set_role"] {
            let result = svc
                .execute_command(ExecuteCommandCmd {
                    args: Some(serde_json::json!({
                        "user_id": rim_core::user::model::UserId::new().to_string(),
                        "role": "member",
                    })),
                    ..palette_cmd(new_actor(), command_id)
                })
                .await
                .expect(command_id);
            assert_eq!(
                result.status,
                CommandResultStatusDto::ConfirmationRequired,
                "{command_id}"
            );
        }
    }

    /// The workspace comes from the context, the target from `user_id`, and the
    /// optional reason is passed through.
    #[tokio::test]
    async fn kick_command_kicks_the_user_arg_from_the_context_workspace() {
        let workspace = rim_core::server::model::ServerId::new();
        let target = rim_core::user::model::UserId::new();
        let svc = moderating(|moderation| {
            moderation
                .expect_kick()
                .withf(move |cmd| {
                    cmd.server_id == workspace
                        && cmd.user_id == target
                        && cmd.reason.as_deref() == Some("spam")
                })
                .times(1)
                .returning(|_| Box::pin(async { Ok(()) }));
        });

        let result = run_confirmed(
            &svc,
            "admin.kick",
            serde_json::json!({ "user_id": target.to_string(), "reason": "spam" }),
            Some(workspace_context(workspace)),
        )
        .await
        .expect("kick");

        assert_eq!(result.message, "member kicked");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({ "user_id": target.to_string() }))
        );
    }

    /// `expires_at` is parsed as RFC 3339 and normalised to UTC; an absent reason stays
    /// absent; the payload echoes the expiry the service stored.
    #[tokio::test]
    async fn ban_command_passes_the_expiry_in_utc_and_returns_it() {
        let workspace = rim_core::server::model::ServerId::new();
        let target = rim_core::user::model::UserId::new();
        let expires_at = chrono::DateTime::parse_from_rfc3339("2026-12-01T10:00:00Z")
            .expect("timestamp")
            .with_timezone(&chrono::Utc);
        let svc = moderating(|moderation| {
            moderation
                .expect_ban()
                .withf(move |cmd| {
                    cmd.server_id == workspace
                        && cmd.user_id == target
                        && cmd.reason.is_none()
                        && cmd.expires_at == Some(expires_at)
                })
                .times(1)
                .returning(|cmd| {
                    Box::pin(async move {
                        Ok(rim_services::moderation::dto::ServerBanDto {
                            server_id: cmd.server_id,
                            user_id: cmd.user_id,
                            reason: cmd.reason,
                            banned_by: Some(cmd.actor.user_id),
                            created_at: chrono::Utc::now(),
                            expires_at: cmd.expires_at,
                        })
                    })
                });
        });

        let result = run_confirmed(
            &svc,
            "admin.ban",
            serde_json::json!({
                "user_id": target.to_string(),
                "expires_at": "2026-12-01T12:00:00+02:00",
            }),
            Some(workspace_context(workspace)),
        )
        .await
        .expect("ban");

        assert_eq!(result.message, "member banned");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "user_id": target.to_string(),
                "expires_at": expires_at.to_rfc3339(),
            }))
        );
    }

    /// The message and `was_banned` say whether a ban was lifted.
    #[tokio::test]
    async fn unban_command_reports_whether_a_ban_existed() {
        let workspace = rim_core::server::model::ServerId::new();
        let target = rim_core::user::model::UserId::new();
        for (existed, message) in [(true, "ban lifted"), (false, "user was not banned")] {
            let svc = moderating(|moderation| {
                moderation
                    .expect_unban()
                    .withf(move |cmd| cmd.server_id == workspace && cmd.user_id == target)
                    .times(1)
                    .returning(move |_| Box::pin(async move { Ok(existed) }));
            });

            let result = run_confirmed(
                &svc,
                "admin.unban",
                serde_json::json!({ "user_id": target.to_string() }),
                Some(workspace_context(workspace)),
            )
            .await
            .expect("unban");

            assert_eq!(result.message, message);
            assert_eq!(
                result.payload,
                Some(serde_json::json!({
                    "user_id": target.to_string(),
                    "was_banned": existed,
                }))
            );
        }
    }

    /// The reason is passed through and the payload carries the end the service stored.
    #[tokio::test]
    async fn timeout_command_passes_the_reason_and_returns_the_stored_end() {
        let workspace = rim_core::server::model::ServerId::new();
        let target = rim_core::user::model::UserId::new();
        let stored_until = chrono::Utc::now() + chrono::Duration::hours(3);
        let svc = moderating(|moderation| {
            moderation
                .expect_timeout()
                .withf(move |cmd| {
                    cmd.server_id == workspace
                        && cmd.user_id == target
                        && cmd.reason.as_deref() == Some("flooding")
                })
                .times(1)
                .returning(move |cmd| {
                    Box::pin(async move {
                        Ok(rim_services::moderation::dto::MemberTimeoutDto {
                            server_id: cmd.server_id,
                            user_id: cmd.user_id,
                            until: stored_until,
                            reason: cmd.reason,
                            set_by: Some(cmd.actor.user_id),
                            created_at: chrono::Utc::now(),
                        })
                    })
                });
        });

        let result = run_confirmed(
            &svc,
            "admin.timeout",
            serde_json::json!({
                "user_id": target.to_string(),
                "minutes": 10,
                "reason": "flooding",
            }),
            Some(workspace_context(workspace)),
        )
        .await
        .expect("timeout");

        assert_eq!(result.message, "member timed out");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "user_id": target.to_string(),
                "until": stored_until.to_rfc3339(),
            }))
        );
    }

    /// The role arg is trimmed and matched case-insensitively, and the payload names the
    /// role the service stored.
    #[tokio::test]
    async fn set_role_command_accepts_any_case_and_returns_the_stored_role() {
        use rim_core::server_member::model::ServerMemberRole;

        let workspace = rim_core::server::model::ServerId::new();
        let target = rim_core::user::model::UserId::new();
        let svc = moderating(|moderation| {
            moderation
                .expect_set_member_role()
                .withf(move |cmd| {
                    cmd.server_id == workspace
                        && cmd.user_id == target
                        && cmd.role == ServerMemberRole::Moderator
                })
                .times(1)
                .returning(|cmd| {
                    Box::pin(async move {
                        Ok(rim_services::server_member::dto::ServerMemberDto {
                            server_id: cmd.server_id,
                            user_id: cmd.user_id,
                            username: "sam".to_string(),
                            display_name: None,
                            avatar_url: None,
                            status_text: None,
                            bio: None,
                            member_role: cmd.role,
                            joined_at: chrono::Utc::now(),
                            timeout_until: None,
                            principal_id: None,
                        })
                    })
                });
        });

        let result = run_confirmed(
            &svc,
            "admin.set_role",
            serde_json::json!({ "user_id": target.to_string(), "role": " Moderator " }),
            Some(workspace_context(workspace)),
        )
        .await
        .expect("set role");

        assert_eq!(result.message, "role changed");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({ "user_id": target.to_string(), "role": "moderator" }))
        );
    }

    /// Missing or malformed arguments are refused with a message naming the argument,
    /// before the moderation service is called.
    #[tokio::test]
    async fn admin_commands_refuse_bad_arguments_before_reaching_the_service() {
        let svc = moderating(|_| {});
        let workspace = Some(workspace_context(rim_core::server::model::ServerId::new()));
        let user_id = rim_core::user::model::UserId::new().to_string();
        let mut cases: Vec<(&str, serde_json::Value, Option<CommandContextDto>, &str)> =
            ADMIN_TARGET_COMMANDS
                .iter()
                .map(|command_id| {
                    (
                        *command_id,
                        serde_json::json!({ "minutes": 10, "role": "member" }),
                        workspace.clone(),
                        "missing string arg `user_id`",
                    )
                })
                .collect();
        cases.extend([
            (
                "admin.kick",
                serde_json::json!({ "user_id": "not-a-uuid" }),
                workspace.clone(),
                "invalid uuid arg `user_id`",
            ),
            (
                "admin.kick",
                serde_json::json!({ "user_id": user_id }),
                None,
                "missing workspace id: provide `server_id` arg or `context.workspace_id`",
            ),
            (
                "admin.ban",
                serde_json::json!({ "user_id": user_id, "expires_at": "next week" }),
                workspace.clone(),
                "invalid RFC 3339 timestamp arg `expires_at`",
            ),
            (
                "admin.timeout",
                serde_json::json!({ "user_id": user_id }),
                workspace.clone(),
                "arg `minutes` must be an integer from 1 to 43200",
            ),
        ]);

        for (command_id, args, context, expected) in cases {
            let err = run_confirmed(&svc, command_id, args, context)
                .await
                .expect_err(command_id);
            assert_eq!(validation_message(err), expected, "{command_id}");
        }
    }

    /// A refusal from the moderation service reaches the caller as the same error.
    #[tokio::test]
    async fn admin_commands_surface_a_moderation_refusal_unchanged() {
        let svc = moderating(|moderation| {
            moderation
                .expect_kick()
                .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
            moderation
                .expect_ban()
                .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
            moderation
                .expect_unban()
                .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
            moderation
                .expect_timeout()
                .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
            moderation
                .expect_set_member_role()
                .returning(|_| Box::pin(async { Err(ServiceError::PermissionDenied) }));
        });
        let args = serde_json::json!({
            "user_id": rim_core::user::model::UserId::new().to_string(),
            "minutes": 10,
            "role": "member",
        });

        for command_id in ADMIN_TARGET_COMMANDS {
            let err = run_confirmed(
                &svc,
                command_id,
                args.clone(),
                Some(workspace_context(rim_core::server::model::ServerId::new())),
            )
            .await
            .expect_err(command_id);
            assert!(
                matches!(err, ServiceError::PermissionDenied),
                "{command_id}: {err:?}"
            );
        }
    }

    /// The reply goes to the context's conversation with the selected message as parent.
    #[tokio::test]
    async fn reply_to_selected_message_command_replies_to_the_selected_message() {
        let workspace = rim_core::server::model::ServerId::new();
        let conversation = rim_core::conversation::model::ConversationId::new();
        let selected = rim_core::message::model::MessageId::new();
        let reply = rim_core::message::model::MessageId::new();
        let mut message_service = MockMessageService::new();
        message_service
            .expect_send_message()
            .withf(move |cmd| {
                cmd.server_id == workspace
                    && cmd.conversation_id == conversation
                    && cmd.parent_message_id == Some(selected)
                    && cmd.forwarded_from_message_id.is_none()
                    && cmd.text.as_deref() == Some("on it")
            })
            .times(1)
            .returning(move |cmd| {
                Box::pin(async move {
                    Ok(MessageDto {
                        id: reply,
                        ..sent_message(cmd)
                    })
                })
            });

        let result = run_confirmed(
            &messaging(message_service),
            "chat.reply_to_selected_message",
            serde_json::json!({ "text": "on it" }),
            Some(selected_message_context(workspace, conversation, selected)),
        )
        .await
        .expect("reply");

        assert_eq!(result.message, "reply posted");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "message_id": reply.to_string(),
                "parent_message_id": selected.to_string(),
            }))
        );
    }

    /// The edit targets the selected message in the context's conversation.
    #[tokio::test]
    async fn edit_message_command_edits_the_selected_message() {
        let workspace = rim_core::server::model::ServerId::new();
        let conversation = rim_core::conversation::model::ConversationId::new();
        let selected = rim_core::message::model::MessageId::new();
        let mut message_service = MockMessageService::new();
        message_service
            .expect_edit_message()
            .withf(move |cmd| {
                cmd.server_id == workspace
                    && cmd.conversation_id == conversation
                    && cmd.message_id == selected
                    && cmd.text == "fixed typo"
            })
            .times(1)
            .returning(|cmd| {
                Box::pin(async move {
                    Ok(MessageDto {
                        id: cmd.message_id,
                        text: Some(cmd.text),
                        ..message_in(cmd.server_id, cmd.conversation_id)
                    })
                })
            });

        let result = run_confirmed(
            &messaging(message_service),
            "chat.edit_message",
            serde_json::json!({ "text": "fixed typo" }),
            Some(selected_message_context(workspace, conversation, selected)),
        )
        .await
        .expect("edit");

        assert_eq!(result.message, "message edited");
        assert_eq!(
            result.payload,
            Some(serde_json::json!({
                "message_id": selected.to_string(),
                "conversation_id": conversation.to_string(),
            }))
        );
    }

    /// Reply and edit need a context with a workspace, a conversation and a selected
    /// message, and a non-blank `text`; each gap is refused by name without calling the
    /// message service.
    #[tokio::test]
    async fn selected_message_commands_refuse_an_incomplete_context_or_blank_text() {
        let full = selected_message_context(
            rim_core::server::model::ServerId::new(),
            rim_core::conversation::model::ConversationId::new(),
            rim_core::message::model::MessageId::new(),
        );
        let text = serde_json::json!({ "text": "hi" });
        let cases = [
            (None, text.clone(), "command requires context"),
            (
                Some(CommandContextDto {
                    workspace_id: None,
                    ..full.clone()
                }),
                text.clone(),
                "command requires workspace context",
            ),
            (
                Some(CommandContextDto {
                    conversation_id: None,
                    ..full.clone()
                }),
                text.clone(),
                "command requires conversation context",
            ),
            (
                Some(CommandContextDto {
                    selected_message_id: None,
                    ..full.clone()
                }),
                text,
                "command requires selected message context",
            ),
            (
                Some(full.clone()),
                serde_json::json!({}),
                "missing string arg `text`",
            ),
            (
                Some(full),
                serde_json::json!({ "text": "  " }),
                "missing string arg `text`",
            ),
        ];
        let svc = messaging(MockMessageService::new());

        for command_id in ["chat.reply_to_selected_message", "chat.edit_message"] {
            for (context, args, expected) in cases.clone() {
                let err = run_confirmed(&svc, command_id, args, context)
                    .await
                    .expect_err(command_id);
                assert_eq!(validation_message(err), expected, "{command_id}");
            }
        }
    }

    /// A user command takes the highest tier among its steps, through nested user commands;
    /// a dangerous step still makes it Restricted.
    #[tokio::test]
    async fn user_command_sensitivity_is_the_highest_of_its_steps() {
        let actor = new_actor();
        let svc = validating_service(repo_with(vec![
            calling_command(actor.user_id, "user.peek", "state.list"),
            bump_command(actor.user_id, "user.bump"),
            calling_command(actor.user_id, "user.outer", "user.bump"),
            stored_user_command(
                actor.user_id,
                "user.mixed",
                vec![
                    step("state.incr", serde_json::json!({ "key": "k" })),
                    step("state.list", serde_json::Value::Null),
                ],
            ),
            calling_command(actor.user_id, "user.purge", "chat.delete_message"),
        ]));

        for (command_id, expected) in [
            ("user.peek", CommandSensitivityDto::Safe),
            ("user.bump", CommandSensitivityDto::Mutating),
            ("user.outer", CommandSensitivityDto::Mutating),
            ("user.mixed", CommandSensitivityDto::Mutating),
            ("user.purge", CommandSensitivityDto::Restricted),
        ] {
            let definition = svc
                .get_command(GetCommandQuery {
                    actor,
                    command_id: command_id.to_string(),
                })
                .await
                .expect("lookup succeeds")
                .expect("the user command resolves");
            assert_eq!(definition.sensitivity, expected, "{command_id}");
        }
    }

    /// An agent must confirm a user command wrapping a Mutating step, as it would the step
    /// alone; the palette runs it directly.
    #[tokio::test]
    async fn agent_must_confirm_a_user_command_wrapping_a_mutating_step() {
        let actor = new_actor();
        let mut workspace_state = MockWorkspaceStateService::new();
        expect_one_increment(&mut workspace_state, 1);
        let svc = state_service(
            repo_with(vec![bump_command(actor.user_id, "user.bump")]),
            workspace_state,
        );

        let agent = svc
            .execute_command(ExecuteCommandCmd {
                initiator: CommandInitiator::Agent,
                ..palette_cmd(actor, "user.bump")
            })
            .await
            .expect("the agent is asked to confirm");
        assert_eq!(agent.status, CommandResultStatusDto::ConfirmationRequired);
        assert!(agent.confirmation_required.is_some());

        let palette = svc
            .execute_command(palette_cmd(actor, "user.bump"))
            .await
            .expect("the palette runs it");
        assert_eq!(palette.status, CommandResultStatusDto::Success);
    }

    // ── Command visibility ──

    /// The palette ids `svc` lists to `actor` in `context`.
    async fn palette_ids(
        svc: &DefaultCommandService,
        actor: Actor,
        context: Option<CommandContextDto>,
    ) -> std::collections::BTreeSet<String> {
        svc.list_commands(ListCommandsQuery {
            actor,
            context,
            surface: CommandSurfaceDto::Palette,
        })
        .await
        .unwrap()
        .into_iter()
        .map(|c| c.id)
        .collect()
    }

    /// The palette builtins the client runs itself.
    fn client_palette_ids() -> std::collections::BTreeSet<String> {
        registry::builtin_commands()
            .into_iter()
            .filter(|c| {
                c.visible_in_palette && c.execution_target != CommandExecutionTargetDto::Backend
            })
            .map(|c| c.id)
            .collect()
    }

    fn no_capability_checks() -> rim_services::capability::service::MockCapabilityService {
        let mut cap = rim_services::capability::service::MockCapabilityService::new();
        cap.expect_check_many().never();
        cap
    }

    /// A user command `key` owned by `owner` that runs `steps` in order.
    fn user_command_running(
        owner: rim_core::user::model::UserId,
        key: &str,
        steps: &[&str],
    ) -> UserCommand {
        UserCommand {
            id: UserCommandId::new(),
            owner_user_id: owner,
            command_key: key.to_string(),
            title: key.to_string(),
            description: String::new(),
            steps_json: serde_json::Value::Array(
                steps
                    .iter()
                    .map(|step| serde_json::json!({ "command_id": step, "args": null }))
                    .collect(),
            ),
            is_enabled: true,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
        }
    }

    #[tokio::test]
    async fn without_a_workspace_membership_only_client_commands_are_listed() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let no_workspace = listing_service(
            rim_services::server_member::service::MockServerMemberService::new(),
            no_capability_checks(),
            vec![],
        );
        let outsider = listing_service(
            rim_services::server_member::service::MockServerMemberService::with_own_role(None),
            no_capability_checks(),
            vec![],
        );
        let workspace = Some(workspace_context(rim_core::server::model::ServerId::new()));

        assert_eq!(
            palette_ids(&no_workspace, actor, None).await,
            client_palette_ids()
        );
        assert_eq!(
            palette_ids(&outsider, actor, workspace).await,
            client_palette_ids()
        );
    }

    #[tokio::test]
    async fn listed_workspace_commands_follow_the_callers_role() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let member_level = [
            "chat.create_channel",
            "chat.create_group",
            "chat.invite_to_conversation",
            "chat.join_channel",
        ];
        let moderator_level = [
            "admin.ban",
            "admin.kick",
            "admin.set_role",
            "admin.timeout",
            "admin.unban",
            "chat.archive_channel",
            "chat.set_channel_posting",
            "chat.set_slow_mode",
            "chat.unarchive_channel",
        ];
        let role_bound: Vec<&str> = member_level
            .iter()
            .chain(moderator_level.iter())
            .copied()
            .chain(["admin.invite_member"])
            .collect();
        let expected = |levels: &[&[&str]]| -> std::collections::BTreeSet<String> {
            levels
                .iter()
                .flat_map(|ids| ids.iter().map(|id| id.to_string()))
                .collect()
        };

        for (role, sees) in [
            (ServerMemberRole::Guest, expected(&[])),
            (ServerMemberRole::Member, expected(&[&member_level])),
            (
                ServerMemberRole::Moderator,
                expected(&[&member_level, &moderator_level]),
            ),
            (
                ServerMemberRole::Owner,
                expected(&[&member_level, &moderator_level, &["admin.invite_member"]]),
            ),
        ] {
            let svc = listing_service(
                rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                    role,
                )),
                full_caps(),
                vec![],
            );
            let ids = palette_ids(
                &svc,
                actor,
                Some(workspace_context(rim_core::server::model::ServerId::new())),
            )
            .await;

            let listed_bound: std::collections::BTreeSet<String> = role_bound
                .iter()
                .filter(|id| ids.contains(**id))
                .map(|id| id.to_string())
                .collect();
            assert_eq!(listed_bound, sees, "{role}");
            assert!(ids.contains("chat.post_message"), "{role}");
            assert!(client_palette_ids().is_subset(&ids), "{role}");
        }
    }

    #[tokio::test]
    async fn a_direct_space_guest_sees_the_commands_guests_run_there() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let direct = rim_core::server::model::ServerId::new();
        let svc = listing_service(
            rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                ServerMemberRole::Guest,
            )),
            full_caps(),
            vec![],
        )
        .with_direct_space(direct);

        let ids = palette_ids(&svc, actor, Some(workspace_context(direct))).await;

        for id in ["chat.join_channel", "chat.post_message"] {
            assert!(ids.contains(id), "{id}");
        }
        for id in [
            "chat.create_channel",
            "chat.create_group",
            "chat.invite_to_conversation",
            "chat.archive_channel",
            "admin.kick",
        ] {
            assert!(!ids.contains(id), "{id}");
        }
    }

    #[tokio::test]
    async fn a_user_command_is_listed_only_to_a_role_every_step_admits() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let commands = vec![
            user_command_running(actor.user_id, "user.greet", &["chat.post_message"]),
            user_command_running(
                actor.user_id,
                "user.kick_and_greet",
                &["chat.post_message", "admin.kick"],
            ),
            user_command_running(actor.user_id, "user.wrapped", &["user.kick_and_greet"]),
        ];

        for (role, sees_kick) in [
            (ServerMemberRole::Member, false),
            (ServerMemberRole::Moderator, true),
        ] {
            let svc = listing_service(
                rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                    role,
                )),
                full_caps(),
                commands.clone(),
            );
            let ids = palette_ids(
                &svc,
                actor,
                Some(workspace_context(rim_core::server::model::ServerId::new())),
            )
            .await;

            assert!(ids.contains("user.greet"), "{role}");
            assert_eq!(ids.contains("user.kick_and_greet"), sees_kick, "{role}");
            assert_eq!(ids.contains("user.wrapped"), sees_kick, "{role}");
        }
    }

    #[tokio::test]
    async fn a_failed_membership_lookup_fails_the_listing() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut member = rim_services::server_member::service::MockServerMemberService::new();
        member
            .expect_get_member()
            .returning(|_| Box::pin(async { Err(ServiceError::Internal(None)) }));
        let svc = listing_service(member, no_capability_checks(), vec![]);

        let err = svc
            .list_commands(ListCommandsQuery {
                actor,
                context: Some(workspace_context(rim_core::server::model::ServerId::new())),
                surface: CommandSurfaceDto::Palette,
            })
            .await
            .unwrap_err();

        assert!(matches!(err, ServiceError::Internal(_)), "{err:?}");
    }

    #[tokio::test]
    async fn command_discover_leaves_out_what_the_callers_role_cannot_run() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = listing_service(
            rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                ServerMemberRole::Member,
            )),
            full_caps(),
            vec![],
        );

        let ids: Vec<String> = svc
            .command_discover(
                &actor,
                None,
                Some(100),
                Some(rim_core::server::model::ServerId::new()),
            )
            .await
            .unwrap()
            .into_iter()
            .map(|c| c.id)
            .collect();

        assert!(ids.contains(&"chat.post_message".to_string()));
        for id in [
            "admin.timeout",
            "chat.archive_channel",
            "chat.set_channel_posting",
            "chat.set_slow_mode",
            "chat.unarchive_channel",
        ] {
            assert!(!ids.contains(&id.to_string()), "{id}");
        }
    }

    // ── Execution role gate ──

    /// `moderating(expect)` with the caller holding `role` in every workspace.
    fn moderating_as(
        role: ServerMemberRole,
        expect: impl FnOnce(&mut rim_services::moderation::service::MockModerationService),
    ) -> DefaultCommandService {
        let mut svc = moderating(expect);
        svc.services.server_member_service = Arc::new(
            rim_services::server_member::service::MockServerMemberService::with_own_role(Some(
                role,
            )),
        );
        svc
    }

    /// An auth service whose `create_invite` is never called.
    fn no_invites() -> Arc<rim_services::auth::service::MockAuthService> {
        let mut auth = rim_services::auth::service::MockAuthService::new();
        auth.expect_create_invite().never();
        Arc::new(auth)
    }

    /// Asserts `result` is a `Forbidden` refusal naming `command_id`.
    fn assert_forbidden<T: std::fmt::Debug>(result: Result<T, ServiceError>, command_id: &str) {
        match result {
            Err(ServiceError::Forbidden(msg)) => {
                assert!(
                    msg.contains(&format!("`{command_id}`")),
                    "{command_id}: {msg}"
                )
            }
            other => panic!("{command_id}: expected Forbidden, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn a_builtin_above_the_callers_role_is_refused_before_its_service_runs() {
        let args = serde_json::json!({
            "user_id": rim_core::user::model::UserId::new().to_string(),
            "conversation_id": rim_core::conversation::model::ConversationId::new().to_string(),
            "minutes": 10,
            "seconds": 10,
            "role": "member",
        });
        let mut member_refused: Vec<&str> = ADMIN_TARGET_COMMANDS.to_vec();
        member_refused.extend([
            "chat.set_slow_mode",
            "chat.set_channel_posting",
            "admin.invite_member",
        ]);

        for (role, refused) in [
            (ServerMemberRole::Member, member_refused),
            (ServerMemberRole::Moderator, vec!["admin.invite_member"]),
        ] {
            let mut svc = moderating_as(role, |moderation| {
                moderation.expect_kick().never();
                moderation.expect_ban().never();
                moderation.expect_unban().never();
                moderation.expect_timeout().never();
                moderation.expect_set_member_role().never();
                moderation.expect_set_slow_mode().never();
                moderation.expect_set_channel_posting().never();
            });
            svc.services.auth_service = no_invites();

            for command_id in refused {
                let result = run_confirmed(
                    &svc,
                    command_id,
                    args.clone(),
                    Some(workspace_context(rim_core::server::model::ServerId::new())),
                )
                .await;
                assert_forbidden(result, command_id);
            }
        }
    }

    #[tokio::test]
    async fn a_direct_space_guest_is_refused_a_member_builtin_before_its_service_runs() {
        let direct = rim_core::server::model::ServerId::new();
        let mut conversation = MockConversationService::new();
        conversation.expect_create_conversation().never();
        let mut svc = moderating_as(ServerMemberRole::Guest, |moderation| {
            moderation.expect_kick().never();
        })
        .with_direct_space(direct);
        svc.services.conversation_service = Arc::new(conversation);
        let args = serde_json::json!({
            "name": "general",
            "user_id": rim_core::user::model::UserId::new().to_string(),
        });

        for command_id in ["chat.create_channel", "admin.kick"] {
            let result = run_confirmed(
                &svc,
                command_id,
                args.clone(),
                Some(workspace_context(direct)),
            )
            .await;
            assert_forbidden(result, command_id);
        }
    }

    /// The role is the one held in the workspace the command acts on: the `server_id` arg
    /// over the context's workspace.
    #[tokio::test]
    async fn an_owner_runs_the_owner_only_builtin_in_the_workspace_they_own() {
        let owned = rim_core::server::model::ServerId::new();
        let other = rim_core::server::model::ServerId::new();
        let mut member = rim_services::server_member::service::MockServerMemberService::new();
        member.expect_get_member().returning(move |query| {
            let role = if query.server_id == owned {
                ServerMemberRole::Owner
            } else {
                ServerMemberRole::Member
            };
            Box::pin(async move {
                Ok(rim_services::server_member::dto::ServerMemberDto {
                    server_id: query.server_id,
                    user_id: query.user_id,
                    username: "member".to_string(),
                    display_name: None,
                    avatar_url: None,
                    status_text: None,
                    bio: None,
                    member_role: role,
                    joined_at: chrono::Utc::now(),
                    timeout_until: None,
                    principal_id: None,
                })
            })
        });
        let mut auth = rim_services::auth::service::MockAuthService::new();
        auth.expect_create_invite()
            .withf(move |cmd| cmd.server_id == owned)
            .times(1)
            .returning(|cmd| {
                Box::pin(async move {
                    Ok(rim_services::auth::dto::CreateInviteResultDto {
                        invite: rim_services::auth::dto::InviteDto {
                            id: rim_core::auth::model::InviteId::new(),
                            server_id: cmd.server_id,
                            created_by_user_id: cmd.actor.user_id,
                            created_at: chrono::Utc::now(),
                            expires_at: None,
                            max_uses: cmd.max_uses,
                            use_count: 0,
                            revoked_at: None,
                            role: cmd.role,
                        },
                        invite_token: "token".to_string(),
                    })
                })
            });
        let mut svc = moderating(|_| {});
        svc.services.server_member_service = Arc::new(member);
        svc.services.auth_service = Arc::new(auth);

        let invited = run_confirmed(
            &svc,
            "admin.invite_member",
            serde_json::json!({ "server_id": owned.to_string() }),
            Some(workspace_context(other)),
        )
        .await
        .expect("the owner invites");
        assert_eq!(invited.status, CommandResultStatusDto::Success);

        let refused = run_confirmed(
            &svc,
            "admin.invite_member",
            serde_json::json!({ "server_id": other.to_string() }),
            Some(workspace_context(owned)),
        )
        .await;
        assert_forbidden(refused, "admin.invite_member");
    }

    #[tokio::test]
    async fn a_user_command_step_above_the_callers_role_is_refused_before_its_service_runs() {
        let actor = new_actor();
        let mut svc = moderating_as(ServerMemberRole::Member, |moderation| {
            moderation.expect_kick().never();
        });
        svc.repos.user_command_repo = Arc::new(repo_with(vec![user_command_running(
            actor.user_id,
            "user.kick",
            &["admin.kick"],
        )]));

        let result = run_confirmed_as(
            &svc,
            actor,
            "user.kick",
            serde_json::json!({}),
            Some(workspace_context(rim_core::server::model::ServerId::new())),
        )
        .await;

        assert_forbidden(result, "admin.kick");
    }

    #[tokio::test]
    async fn execute_command_refuses_oversized_calls_before_auditing() {
        use rim_services::command::bounds::MAX_EXECUTE_ARGS_BYTES;

        let actor = Actor::new(rim_core::user::model::UserId::new());
        let mut audit_svc = rim_services::audit::service::MockAuditLogService::new();
        audit_svc.expect_write_entry().never();
        let svc = service_with_rate_limited_cmd(
            rate_limited_cmd_def("test.bounded", 100),
            Some(Arc::new(audit_svc)),
        );

        let mut oversized_args = palette_cmd(actor, "test.bounded");
        oversized_args.args =
            Some(serde_json::json!({ "text": "x".repeat(MAX_EXECUTE_ARGS_BYTES) }));
        let mut long_pipe =
            palette_cmd(actor, &format!("test.bounded {}", "key=value ".repeat(500)));
        long_pipe.is_pipe = true;
        for (cmd, refusal) in [
            (
                oversized_args,
                "command arguments may be at most 65536 bytes",
            ),
            (
                palette_cmd(actor, &format!("user.{}", "x".repeat(300))),
                "a command id may be at most 256 bytes",
            ),
            (long_pipe, "a pipe expression may be at most 4096 bytes"),
        ] {
            let err = svc.execute_command(cmd).await.unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == refusal),
                "{err:?}"
            );
        }
    }

    #[tokio::test]
    async fn execute_command_audits_large_args_truncated() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let written: Arc<std::sync::Mutex<Vec<rim_services::audit::dto::AuditLogEntryDto>>> =
            Arc::new(std::sync::Mutex::new(vec![]));
        let written_clone = written.clone();
        let mut audit_svc = rim_services::audit::service::MockAuditLogService::new();
        audit_svc
            .expect_write_entry()
            .times(1)
            .returning(move |entry| {
                written_clone.lock().unwrap().push(entry);
                Box::pin(async { Ok(()) })
            });
        let svc = service_with_rate_limited_cmd(
            rate_limited_cmd_def("test.bounded", 100),
            Some(Arc::new(audit_svc)),
        );
        let args = serde_json::json!({ "text": "x".repeat(10_000) });
        let size = args.to_string().len();
        let mut cmd = palette_cmd(actor, "test.bounded");
        cmd.args = Some(args);

        svc.execute_command(cmd).await.expect("runs");

        let entries = written.lock().unwrap();
        let kept = &entries[0].args_redacted;
        assert_eq!(kept["truncated"], serde_json::json!(true), "{kept}");
        assert_eq!(kept["bytes"], serde_json::json!(size));
        assert!(kept.to_string().len() <= 4096, "{kept}");
    }

    #[tokio::test]
    async fn user_command_text_and_step_args_are_bounded() {
        let actor = Actor::new(rim_core::user::model::UserId::new());
        let svc = service(
            MockUserCommandRepository::new(),
            MockConversationService::new(),
            MockMessageService::new(),
        );
        let step = |args| UserCommandStepDto {
            command_id: "chat.post_message".to_string(),
            args,
        };
        for (title, description, args, refusal) in [
            (
                "t".repeat(129),
                String::new(),
                None,
                "a user command title may be at most 128 characters",
            ),
            (
                "Title".to_string(),
                "d".repeat(1001),
                None,
                "a user command description may be at most 1000 characters",
            ),
            (
                "Title".to_string(),
                String::new(),
                Some(serde_json::json!({ "text": "x".repeat(4096) })),
                "a user command step's args may be at most 4096 bytes",
            ),
        ] {
            let err = svc
                .create_user_command(CreateUserCommandCmd {
                    actor,
                    command_key: "user.bounded".to_string(),
                    title,
                    description,
                    steps: vec![step(args)],
                })
                .await
                .unwrap_err();
            assert!(
                matches!(&err, ServiceError::Validation(m) if m == refusal),
                "{err:?}"
            );
        }

        let mut repo = MockUserCommandRepository::new();
        repo.expect_get_by_key_for_user()
            .returning(move |owner, key| Ok(Some(saved_user_command(owner, key, true))));
        repo.expect_update_user_command().never();
        let svc = service(
            repo,
            MockConversationService::new(),
            MockMessageService::new(),
        );
        let err = svc
            .update_user_command(UpdateUserCommandCmd {
                actor,
                command_id: "user.bounded".to_string(),
                title: "t".repeat(129),
                description: String::new(),
                steps: vec![step(None)],
                is_enabled: true,
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::Validation(_)), "{err:?}");
    }
}
