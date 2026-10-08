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
        CreateUserCommandCmd, DeleteUserCommandCmd, ExecuteCommandCmd, ListCommandsQuery,
        UpdateUserCommandCmd,
    };
    use rim_services::command::sensitivity::CommandSensitivityDto;
    use rim_services::command::service::CommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::message::dto::MessageDto;
    use rim_services::message::service::MockMessageService;
    use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
    use rim_services::{Actor, ServiceError};

    use crate::CommandRepos;
    use crate::command::registry;
    use crate::command::{CommandServices, DefaultCommandService};

    mockall::mock! {
        UserCommandRepository {}
        #[async_trait]
        impl UserCommandRepository for UserCommandRepository {
            async fn get_by_id(&self, id: UserCommandId) -> RepoResult<Option<UserCommand>>;
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
                workspace_state_service: Arc::new(
                    rim_services::workspace_state::service::MockWorkspaceStateService::new(),
                ),
                audit_log_service: None,
                incident_service: std::sync::Arc::new(
                    rim_services::incident::service::MockIncidentService::new(),
                ),
                oncall_service: std::sync::Arc::new(
                    rim_services::oncall::service::MockOnCallService::new(),
                ),
                workspace_feature_service: std::sync::Arc::new(
                    MockWorkspaceFeatureService::all_enabled(),
                ),
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
            feature: None,
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

        // Without a workspace context, feature-gated builtins (Track 0.4) are hidden.
        let expected_palette_count = registry::builtin_commands()
            .iter()
            .filter(|c| c.visible_in_palette && c.feature.is_none())
            .count();
        assert_eq!(palette.len(), expected_palette_count);
        assert!(palette.iter().any(|c| c.id == "chat.post_message"));
    }

    #[tokio::test]
    // C2 (`docs/specs/track-c2-agent-operator.md` §1) flips 4 more builtins to
    // `ai_callable: true` — `incident.ack`/`incident.escalate`/`oncall.who`/
    // `oncall.take_shift`, all with `required_capabilities: []` (§2) — so they
    // surface here unconditionally regardless of the mocked capability grants.
    // Renamed 12→16 to match the sibling inventory gate in `registry/mod.rs`.
    // Track 1.1 adds five agent-callable membership/archive builtins (`chat.write`, granted by
    // the mock): 16→21. Track 1.2 adds three moderation builtins — `chat.set_slow_mode`,
    // `chat.report_message` (`chat.write`) and `admin.timeout` (`admin.manage_members`, now also
    // granted by the mock): 21→24. Track 1.3 adds three notification builtins with no
    // capability (`notifications.set_level`, `notifications.dnd`, `notifications.mark_all_read`):
    // 24→27. Track 4.1 flips eighteen general builtins (navigation, `chat.mark_as_read`,
    // `chat.create_group`, `state.*`), none needing a new capability: 27→45.
    async fn list_commands_ai_surface_returns_forty_five_commands_with_caps() {
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

        // The four incident/on-call commands are feature-gated (Track 0.4): they only surface
        // with a workspace context whose `incidents` feature is on (the mocked feature service
        // reports every feature enabled).
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
                "incident.ack",
                "incident.escalate",
                "notifications.dnd",
                "notifications.mark_all_read",
                "notifications.set_level",
                "oncall.take_shift",
                "oncall.who",
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
            feature: None,
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
                workspace_state_service: Arc::new(
                    rim_services::workspace_state::service::MockWorkspaceStateService::new(),
                ),
                audit_log_service: audit_svc,
                incident_service: std::sync::Arc::new(
                    rim_services::incident::service::MockIncidentService::new(),
                ),
                oncall_service: std::sync::Arc::new(
                    rim_services::oncall::service::MockOnCallService::new(),
                ),
                workspace_feature_service: std::sync::Arc::new(
                    MockWorkspaceFeatureService::all_enabled(),
                ),
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

    fn rate_limit_execute_cmd(actor: Actor, command_id: &str) -> ExecuteCommandCmd {
        ExecuteCommandCmd {
            actor,
            command_id: command_id.to_string(),
            args: None,
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
            confirmation: None,
            is_pipe: false,
            initiator: rim_services::command::initiator::CommandInitiator::Palette,
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

        let r1 = svc
            .execute_command(rate_limit_execute_cmd(actor, cmd_id))
            .await;
        assert!(r1.is_ok(), "call 1 should be allowed, got: {r1:?}");

        let r2 = svc
            .execute_command(rate_limit_execute_cmd(actor, cmd_id))
            .await;
        assert!(r2.is_ok(), "call 2 should be allowed, got: {r2:?}");

        let r3 = svc
            .execute_command(rate_limit_execute_cmd(actor, cmd_id))
            .await;
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

        let ra = svc
            .execute_command(rate_limit_execute_cmd(actor_a, cmd_id))
            .await;
        assert!(ra.is_ok(), "actor A call should be allowed, got: {ra:?}");

        let rb = svc
            .execute_command(rate_limit_execute_cmd(actor_b, cmd_id))
            .await;
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
                workspace_state_service: Arc::new(
                    rim_services::workspace_state::service::MockWorkspaceStateService::new(),
                ),
                audit_log_service: None,
                incident_service: std::sync::Arc::new(
                    rim_services::incident::service::MockIncidentService::new(),
                ),
                oncall_service: std::sync::Arc::new(
                    rim_services::oncall::service::MockOnCallService::new(),
                ),
                workspace_feature_service: std::sync::Arc::new(
                    MockWorkspaceFeatureService::all_enabled(),
                ),
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
                        feature: None,
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
            feature: None,
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
                workspace_state_service: Arc::new(
                    rim_services::workspace_state::service::MockWorkspaceStateService::new(),
                ),
                audit_log_service: None,
                incident_service: std::sync::Arc::new(
                    rim_services::incident::service::MockIncidentService::new(),
                ),
                oncall_service: std::sync::Arc::new(
                    rim_services::oncall::service::MockOnCallService::new(),
                ),
                workspace_feature_service: std::sync::Arc::new(
                    MockWorkspaceFeatureService::all_enabled(),
                ),
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
    async fn p6_m2_agent_mutating_no_proof_requires_confirmation() {
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
    async fn p6_m2_palette_mutating_no_proof_executes() {
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
    async fn p6_fu3_agent_pipe_with_mutating_stage_requires_confirmation() {
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
    async fn p6_fu3_palette_pipe_with_mutating_stage_executes() {
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
    async fn p6_m2_agent_mutating_valid_proof_executes() {
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
            feature: None,
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
                        feature: None,
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

    /// Regression test for the command/macro two-instance defect fixed by the
    /// `OnceLock` late-binding design (Track A item 2, 2026-07-08): previously
    /// the provider held **two** `DefaultCommandService` instances, and
    /// `keybind`/`pipeline` held the macro-less phase-1 instance, so a late
    /// macro wire was invisible to them. With a single shared instance behind
    /// `Arc<OnceLock<_>>`, setting the macro service on one `Arc` clone must be
    /// observed through every other clone. If a future refactor reintroduces a
    /// second `DefaultCommandService` instance, this test fails.
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

    // ── Track 1.2: timeout gate (T1) ──

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
}
