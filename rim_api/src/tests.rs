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
    use rim_services::channel_lens::service::MockChannelLensService;
    use rim_services::command::service::MockCommandService;
    use rim_services::conversation::service::MockConversationService;
    use rim_services::custom_emoji::service::MockCustomEmojiService;
    use rim_services::event::service::MockEventService;
    use rim_services::incident::service::MockIncidentService;
    use rim_services::ingest_token::service::MockIngestTokenService;
    use rim_services::integration::service::MockIntegrationService;
    use rim_services::keybind::service::MockKeybindService;
    use rim_services::link_preview::service::MockLinkPreviewService;
    use rim_services::macro_def::service::MockMacroService;
    use rim_services::mention::service::MockMentionService;
    use rim_services::message::service::MockMessageService;
    use rim_services::message_attachment::service::MockMessageAttachmentService;
    use rim_services::message_reaction::service::MockMessageReactionService;
    use rim_services::message_thread::service::MockThreadService;
    use rim_services::notification_preference::service::MockNotificationPreferenceService;
    use rim_services::password_reset::service::MockPasswordResetService;
    use rim_services::pin::service::MockPinService;
    use rim_services::pipeline::service::MockPipelineService;
    use rim_services::provider::AppServiceProvider;
    use rim_services::read_state::service::MockReadStateService;
    use rim_services::server::service::MockServerService;
    use rim_services::server_member::service::MockServerMemberService;
    use rim_services::setting::service::MockSettingService;
    use rim_services::skill_profile::service::MockSkillProfileService;
    use rim_services::storage::service::MockStorageService;
    use rim_services::user::service::MockUserService;
    use rim_services::workspace_state::service::MockWorkspaceStateService;

    use crate::auth_context::AuthActorCtx;
    use crate::broadcast::InMemoryBroadcaster;
    use crate::build_schema;
    use crate::export_sdl;

    fn test_presence_manager() -> Arc<dyn rim_services::presence::PresenceService> {
        let broadcaster: Arc<dyn rim_services::events::Broadcaster> =
            Arc::new(rim_services::events::NoopBroadcaster);
        let observers = Arc::new(rim_services::events::Observers::new(broadcaster));
        Arc::new(rim_infra_services::presence::PresenceManager::new(
            observers,
        ))
    }

    fn test_typing_tracker() -> Arc<dyn rim_services::typing::TypingService> {
        Arc::new(rim_infra_services::typing::TypingTracker::new())
    }

    fn test_observers() -> Arc<rim_services::events::Observers> {
        let broadcaster: Arc<dyn rim_services::events::Broadcaster> =
            Arc::new(rim_services::events::NoopBroadcaster);
        Arc::new(rim_services::events::Observers::new(broadcaster))
    }

    fn test_registration_context() -> crate::RegistrationContext {
        crate::RegistrationContext {
            registration_mode: Arc::new(rim_services::auth::dto::RegistrationMode::InviteOnly),
            preview_rate_limiter: Arc::new(rim_services::auth::dto::PreviewInviteRateLimiterToken),
            signup_rate_limiter: Arc::new(rim_services::auth::dto::SignupRateLimiterToken),
        }
    }

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
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(user),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    fn schema_with_server_mocks(
        server: MockServerService,
        server_member: MockServerMemberService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(server),
                server_member: Arc::new(server_member),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    fn schema_with_keybind_mock(keybind: MockKeybindService) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(keybind),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    /// Builds a schema with a custom `IncidentService` mock and (since `Incident.
    /// acknowledgedByUser`/`escalationRun` need it) a custom `UserService` mock for the shared
    /// `User` DataLoader.
    fn schema_with_incident_mocks(
        incident: MockIncidentService,
        user: MockUserService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(user),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(incident),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    fn schema_with_oncall_mock(
        oncall: rim_services::oncall::service::MockOnCallService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(oncall),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    fn schema_with_escalation_policy_mock(
        escalation_policy: rim_services::escalation::service::MockEscalationPolicyService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(escalation_policy),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    // ── DTO factory helpers ───────────────────────────────────────────────

    fn test_conversation_dto() -> rim_services::conversation::dto::ConversationDto {
        rim_services::conversation::dto::ConversationDto {
            id: ConversationId::new(),
            server_id: ServerId::new(),
            conversation_type: ConversationType::Group,
            title: None,
            topic: None,
            member_ids: vec![],
        }
    }

    fn test_incident_dto() -> rim_services::incident::dto::IncidentDto {
        let now = chrono::Utc::now();
        rim_services::incident::dto::IncidentDto {
            id: rim_core::incident::model::IncidentId::new(),
            server_id: ServerId::new(),
            conversation_id: ConversationId::new(),
            started_by: UserId::new(),
            status: rim_core::incident::model::IncidentStatus::Active,
            severity: Some(rim_core::incident::model::IncidentSeverity::Sev2),
            title: "Test incident".to_string(),
            original_topic: None,
            started_at: now,
            mitigated_at: None,
            resolved_at: None,
            postmortem_draft: None,
            created_at: now,
            updated_at: now,
            alert_fingerprint: None,
            source: "manual".to_string(),
            escalation_policy_id: None,
            acknowledged_at: None,
            acknowledged_by: None,
        }
    }

    fn test_escalation_policy_dto() -> rim_services::escalation::dto::EscalationPolicyDto {
        let now = chrono::Utc::now();
        rim_services::escalation::dto::EscalationPolicyDto {
            id: rim_core::escalation::model::EscalationPolicyId::new(),
            server_id: ServerId::new(),
            name: "Primary".to_string(),
            created_by: UserId::new(),
            created_at: now,
            updated_at: now,
            steps: vec![test_escalation_step_dto()],
        }
    }

    fn test_escalation_step_dto() -> rim_services::escalation::dto::EscalationStepDto {
        rim_services::escalation::dto::EscalationStepDto {
            id: rim_core::escalation::model::EscalationStepId::new(),
            policy_id: rim_core::escalation::model::EscalationPolicyId::new(),
            position: 0,
            delay_seconds: 0,
            target: rim_core::escalation::model::EscalationTarget::User(UserId::new()),
            actions: vec![rim_core::escalation::model::EscalationAction::Email],
        }
    }

    fn test_on_call_schedule_dto() -> rim_services::oncall::dto::OnCallScheduleDto {
        let now = chrono::Utc::now();
        rim_services::oncall::dto::OnCallScheduleDto {
            id: rim_core::oncall::model::OnCallScheduleId::new(),
            server_id: ServerId::new(),
            name: "Primary".to_string(),
            timezone: "UTC".to_string(),
            created_by: UserId::new(),
            created_at: now,
            updated_at: now,
        }
    }

    fn test_rotation_layer_dto() -> rim_services::oncall::dto::RotationLayerDto {
        rim_services::oncall::dto::RotationLayerDto {
            id: rim_core::oncall::model::RotationLayerId::new(),
            schedule_id: rim_core::oncall::model::OnCallScheduleId::new(),
            name: "Primary rotation".to_string(),
            priority: 0,
            kind: rim_core::oncall::model::RotationKind::Custom {
                interval_seconds: 3600,
            },
            anchor_at: chrono::Utc::now(),
            created_at: chrono::Utc::now(),
            participants: vec![],
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
            forwarded_from_message_id: None,
            thread_root_message_id: None,
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
            bio: None,
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
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
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
            storage_key: Some("uploads/test/file.txt".to_string()),
            created_at: chrono::Utc::now(),
        }
    }

    // ── Provider / schema builders ────────────────────────────────────────

    struct TestAppServiceProvider {
        action: Arc<dyn rim_services::action::service::ActionService>,
        audit_log: Arc<dyn rim_services::audit::service::AuditLogService>,
        auth: Arc<dyn rim_services::auth::service::AuthService>,
        capability: Arc<dyn rim_services::capability::service::CapabilityService>,
        command: Arc<dyn rim_services::command::service::CommandService>,
        conversation: Arc<dyn rim_services::conversation::service::ConversationService>,
        event: Arc<dyn rim_services::event::service::EventService>,
        integration: Arc<dyn rim_services::integration::service::IntegrationService>,
        keybind: Arc<dyn rim_services::keybind::service::KeybindService>,
        macro_def: Arc<dyn rim_services::macro_def::service::MacroService>,
        mention: Arc<dyn rim_services::mention::service::MentionService>,
        message: Arc<dyn rim_services::message::service::MessageService>,
        attachment: Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
        pin: Arc<dyn rim_services::pin::service::PinService>,
        notification_preference:
            Arc<dyn rim_services::notification_preference::service::NotificationPreferenceService>,
        password_reset: Arc<dyn rim_services::password_reset::service::PasswordResetService>,
        reaction: Arc<dyn rim_services::message_reaction::service::MessageReactionService>,
        read_state: Arc<dyn rim_services::read_state::service::ReadStateService>,
        server: Arc<dyn rim_services::server::service::ServerService>,
        server_member: Arc<dyn rim_services::server_member::service::ServerMemberService>,
        setting: Arc<dyn rim_services::setting::service::SettingService>,
        storage: Arc<dyn rim_services::storage::service::StorageService>,
        thread: Arc<dyn rim_services::message_thread::service::ThreadService>,
        user: Arc<dyn rim_services::user::service::UserService>,
        request_auth: Arc<dyn rim_services::auth::service::RequestAuthenticator>,
        link_preview: Arc<dyn rim_services::link_preview::LinkPreviewService>,
        custom_emoji: Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>,
        channel_lens: Arc<dyn rim_services::channel_lens::service::ChannelLensService>,
        incident: Arc<dyn rim_services::incident::service::IncidentService>,
        ingest_token: Arc<dyn rim_services::ingest_token::service::IngestTokenService>,
        skill_profile: Arc<dyn rim_services::skill_profile::service::SkillProfileService>,
        workspace_state: Arc<dyn rim_services::workspace_state::service::WorkspaceStateService>,
        pipeline: Arc<dyn rim_services::pipeline::service::PipelineService>,
        oncall: Arc<dyn rim_services::oncall::service::OnCallService>,
        escalation_policy: Arc<dyn rim_services::escalation::service::EscalationPolicyService>,
    }

    impl AppServiceProvider for TestAppServiceProvider {
        fn action_service(&self) -> Arc<dyn rim_services::action::service::ActionService> {
            Arc::clone(&self.action)
        }

        fn audit_log_service(&self) -> Arc<dyn rim_services::audit::service::AuditLogService> {
            Arc::clone(&self.audit_log)
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

        fn macro_service(&self) -> Arc<dyn rim_services::macro_def::service::MacroService> {
            Arc::clone(&self.macro_def)
        }

        fn mention_service(&self) -> Arc<dyn rim_services::mention::service::MentionService> {
            Arc::clone(&self.mention)
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

        fn notification_preference_service(
            &self,
        ) -> Arc<dyn rim_services::notification_preference::service::NotificationPreferenceService>
        {
            Arc::clone(&self.notification_preference)
        }

        fn password_reset_service(
            &self,
        ) -> Arc<dyn rim_services::password_reset::service::PasswordResetService> {
            Arc::clone(&self.password_reset)
        }

        fn pin_service(&self) -> Arc<dyn rim_services::pin::service::PinService> {
            Arc::clone(&self.pin)
        }

        fn read_state_service(
            &self,
        ) -> Arc<dyn rim_services::read_state::service::ReadStateService> {
            Arc::clone(&self.read_state)
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

        fn storage_service(&self) -> Arc<dyn rim_services::storage::service::StorageService> {
            Arc::clone(&self.storage)
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

        fn link_preview_service(&self) -> Arc<dyn rim_services::link_preview::LinkPreviewService> {
            Arc::clone(&self.link_preview)
        }

        fn custom_emoji_service(
            &self,
        ) -> Arc<dyn rim_services::custom_emoji::service::CustomEmojiService> {
            Arc::clone(&self.custom_emoji)
        }

        fn channel_lens_service(
            &self,
        ) -> Arc<dyn rim_services::channel_lens::service::ChannelLensService> {
            Arc::clone(&self.channel_lens)
        }

        fn incident_service(&self) -> Arc<dyn rim_services::incident::service::IncidentService> {
            Arc::clone(&self.incident)
        }

        fn escalation_policy_service(
            &self,
        ) -> Arc<dyn rim_services::escalation::service::EscalationPolicyService> {
            Arc::clone(&self.escalation_policy)
        }

        fn ingest_token_service(
            &self,
        ) -> Arc<dyn rim_services::ingest_token::service::IngestTokenService> {
            Arc::clone(&self.ingest_token)
        }

        fn skill_profile_service(
            &self,
        ) -> Arc<dyn rim_services::skill_profile::service::SkillProfileService> {
            Arc::clone(&self.skill_profile)
        }

        fn workspace_state_service(
            &self,
        ) -> Arc<dyn rim_services::workspace_state::service::WorkspaceStateService> {
            Arc::clone(&self.workspace_state)
        }

        fn pipeline_service(&self) -> Arc<dyn rim_services::pipeline::service::PipelineService> {
            Arc::clone(&self.pipeline)
        }

        fn oncall_service(&self) -> Arc<dyn rim_services::oncall::service::OnCallService> {
            Arc::clone(&self.oncall)
        }
    }

    fn schema_with_mocks(
        auth: MockAuthService,
        conv: MockConversationService,
        msg: MockMessageService,
        attachment: MockMessageAttachmentService,
        thread: MockThreadService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(auth),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(conv),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(msg),
                attachment: Arc::new(attachment),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(thread),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
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
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(capability),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(event),
                integration: Arc::new(integration),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(setting),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    fn schema_with_command_mock(command: MockCommandService) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(command),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
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
                    nl_description: None,
                    intent_samples: vec![],
                    ai_callable: false,
                    sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                    typed_verb: rim_services::command::verb::TypedVerb::Other,
                    rate_limit_per_minute: None,
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
                    nl_description: None,
                    intent_samples: vec![],
                    ai_callable: false,
                    sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                    typed_verb: rim_services::command::verb::TypedVerb::Other,
                    rate_limit_per_minute: None,
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

    /// **P3-A1 closure** — `Command.arguments` is the wire contract the
    /// frontend AI tool-schema generator (Phase 3 §4.B) reads to emit
    /// `input_schema`. Round-trip every per-arg field (incl. `defaultValue`
    /// JSON scalar and `allowedValues` Vec<String>) so a regression that
    /// drops the conversion silently surfaces here, not at the agent.
    #[tokio::test]
    async fn commands_query_returns_arguments_with_full_schema() {
        let actor = UserId::new();
        let mut command = MockCommandService::new();
        command.expect_list_commands().returning(|_query| {
            Box::pin(async {
                Ok(vec![rim_services::command::dto::CommandDefinitionDto {
                    id: "state.set".to_string(),
                    title: "Set State".to_string(),
                    description: "desc".to_string(),
                    category: "State".to_string(),
                    source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                    execution_target:
                        rim_services::command::dto::CommandExecutionTargetDto::Backend,
                    arguments: vec![
                        rim_services::command::dto::CommandArgumentDto {
                            name: "key".to_string(),
                            title: "Key".to_string(),
                            description: "State key.".to_string(),
                            argument_type:
                                rim_services::command::dto::CommandArgumentTypeDto::String,
                            required: true,
                            default_value: None,
                            allowed_values: None,
                        },
                        rim_services::command::dto::CommandArgumentDto {
                            name: "scope".to_string(),
                            title: "Scope".to_string(),
                            description: "Server/user.".to_string(),
                            argument_type:
                                rim_services::command::dto::CommandArgumentTypeDto::String,
                            required: false,
                            default_value: Some(serde_json::json!("server")),
                            allowed_values: Some(vec!["server".to_string(), "user".to_string()]),
                        },
                    ],
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
                    sensitivity: rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                    typed_verb: rim_services::command::verb::TypedVerb::Other,
                    rate_limit_per_minute: None,
                }])
            })
        });

        let schema = schema_with_command_mock(command);
        let req = authed_request(
            r#"query {
                commands {
                    id
                    arguments {
                        name
                        title
                        description
                        argumentType
                        required
                        defaultValue
                        allowedValues
                    }
                }
            }"#,
            actor,
        );

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        let args = &json["commands"][0]["arguments"];
        assert_eq!(args.as_array().map(|a| a.len()), Some(2));

        // Required arg with no default / no allowed_values.
        assert_eq!(args[0]["name"], serde_json::json!("key"));
        assert_eq!(args[0]["argumentType"], serde_json::json!("STRING"));
        assert_eq!(args[0]["required"], serde_json::json!(true));
        assert_eq!(args[0]["defaultValue"], serde_json::Value::Null);
        assert_eq!(args[0]["allowedValues"], serde_json::Value::Null);

        // Optional arg with JSON default + enum-style allowed_values.
        assert_eq!(args[1]["name"], serde_json::json!("scope"));
        assert_eq!(args[1]["required"], serde_json::json!(false));
        // `Json` scalar serializes the inner JSON value verbatim — the
        // wire shape the frontend will consume.
        assert_eq!(args[1]["defaultValue"], serde_json::json!("server"));
        assert_eq!(
            args[1]["allowedValues"],
            serde_json::json!(["server", "user"])
        );
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
                let hit = rim_services::message::dto::MessageSearchHitDto {
                    message: dto,
                    rank: 0.5,
                    headline: Some("<mark>deploy</mark> failed".to_string()),
                };
                Ok(Page::new(vec![hit], None, None))
            })
        });

        let mut conv = MockConversationService::new();
        conv.expect_get_conversations_by_ids()
            .returning(move |_ids| {
                Box::pin(async move {
                    Ok(vec![rim_services::conversation::dto::ConversationDto {
                        id: conversation_id,
                        server_id,
                        conversation_type: rim_core::conversation::model::ConversationType::Channel,
                        title: Some("general".to_string()),
                        topic: None,
                        member_ids: vec![],
                    }])
                })
            });

        let schema = schema_with_mocks(
            MockAuthService::new(),
            conv,
            msg,
            MockMessageAttachmentService::new(),
            MockThreadService::new(),
        );

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) {
                searchMessages(input: { serverId: $sid, conversationId: $cid, query: "deploy", limit: 10 }) {
                    items { id text conversationTitle conversationType rank headline }
                    pageInfo { hasNext hasPrev }
                }
            }"#,
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

    // ── updateMyProfile mutation ───────────────────────────────────────────

    #[tokio::test]
    async fn update_my_profile_mutation_returns_updated_user() {
        let actor = UserId::new();
        let mut user = MockUserService::new();
        user.expect_update_my_profile().returning(move |cmd| {
            Box::pin(async move {
                let mut dto = test_user_dto();
                dto.id = cmd.actor.user_id;
                dto.display_name = cmd.display_name;
                dto.avatar_url = cmd.avatar_url;
                dto.status_text = cmd.status_text;
                dto.bio = cmd.bio;
                Ok(dto)
            })
        });

        let schema = schema_with_user_mock(user);

        let req = authed_request(
            r#"mutation($input: UpdateMyProfileInput!) {
                updateMyProfile(input: $input) {
                    id displayName avatarUrl statusText bio
                }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "input": {
                "displayName": "Alice",
                "avatarUrl": "/api/files/avatar/abc",
                "statusText": "busy",
                "bio": "hello"
            }
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);

        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["updateMyProfile"]["id"],
            serde_json::json!(actor.to_string())
        );
        assert_eq!(
            json["updateMyProfile"]["displayName"],
            serde_json::json!("Alice")
        );
        assert_eq!(
            json["updateMyProfile"]["avatarUrl"],
            serde_json::json!("/api/files/avatar/abc")
        );
    }

    #[tokio::test]
    async fn update_my_profile_mutation_requires_auth() {
        let schema = schema_with_user_mock(MockUserService::new());

        let res = schema
            .execute(Request::new(
                r#"mutation { updateMyProfile(input: {}) { id } }"#,
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
    async fn update_my_profile_mutation_maps_validation_error() {
        let actor = UserId::new();
        let mut user = MockUserService::new();
        user.expect_update_my_profile().returning(|_cmd| {
            Box::pin(async move {
                Err(ServiceError::Validation(
                    "avatar_url must start with /api/files/".to_string(),
                ))
            })
        });

        let schema = schema_with_user_mock(user);

        let req = authed_request(
            r#"mutation($input: UpdateMyProfileInput!) {
                updateMyProfile(input: $input) { id }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "input": { "avatarUrl": "https://evil.com/x.png" }
        })));

        let res = schema.execute(req).await;
        assert_eq!(res.errors.len(), 1);
        assert!(res.errors[0].message.contains("avatar_url"));
        assert_eq!(
            res.errors[0]
                .extensions
                .as_ref()
                .and_then(|e| e.get("code")),
            Some(&Value::from("VALIDATION"))
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

    // ── Server.viewerRole (docs/specs/server-viewer-role.md) ──────────────

    /// Builds a schema + query for the `viewerRole` sub-field test cases below, differing only
    /// in what `get_member` returns for the requesting actor.
    async fn run_viewer_role_query(
        actor: UserId,
        server_id: ServerId,
        get_member_result: impl Fn() -> rim_services::ServiceResult<
            rim_services::server_member::dto::ServerMemberDto,
        > + Send
        + Sync
        + 'static,
    ) -> async_graphql::Response {
        let mut server = MockServerService::new();
        server.expect_get_server().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_server_dto();
                dto.id = server_id;
                Ok(dto)
            })
        });

        let mut server_member = MockServerMemberService::new();
        server_member
            .expect_get_member()
            .returning(move |_q| Box::pin(std::future::ready(get_member_result())));

        let schema = schema_with_server_mocks(server, server_member);

        let req = authed_request(
            r#"query($sid: ID!) { server(serverId: $sid) { id viewerRole } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        schema.execute(req).await
    }

    #[tokio::test]
    async fn viewer_role_resolves_owner() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let res = run_viewer_role_query(actor, server_id, move || {
            let mut dto = test_server_member_dto();
            dto.server_id = server_id;
            dto.user_id = actor;
            dto.member_role = "owner".to_string();
            Ok(dto)
        })
        .await;

        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["server"]["viewerRole"], serde_json::json!("OWNER"));
    }

    #[tokio::test]
    async fn viewer_role_resolves_moderator() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let res = run_viewer_role_query(actor, server_id, move || {
            let mut dto = test_server_member_dto();
            dto.server_id = server_id;
            dto.user_id = actor;
            dto.member_role = "moderator".to_string();
            Ok(dto)
        })
        .await;

        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["server"]["viewerRole"], serde_json::json!("MODERATOR"));
    }

    #[tokio::test]
    async fn viewer_role_resolves_guest() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let res = run_viewer_role_query(actor, server_id, move || {
            let mut dto = test_server_member_dto();
            dto.server_id = server_id;
            dto.user_id = actor;
            dto.member_role = "guest".to_string();
            Ok(dto)
        })
        .await;

        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["server"]["viewerRole"], serde_json::json!("GUEST"));
    }

    /// Non-member path: `viewerRole` is non-null in the schema, so a caller who isn't a member
    /// must get a GraphQL error (matching existing server-visibility behavior), never a fake
    /// role.
    #[tokio::test]
    async fn viewer_role_non_member_returns_error_not_a_fake_role() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let res = run_viewer_role_query(actor, server_id, || {
            Err(rim_services::ServiceError::PermissionDenied)
        })
        .await;

        assert!(
            !res.errors.is_empty(),
            "non-member viewerRole query must error, not resolve a fake role"
        );
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

    // ── listAuditLog resolver tests ──────────────────────────────────────────

    fn schema_with_audit_mock(
        audit_log: rim_services::audit::service::MockAuditLogService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(audit_log),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(MockIngestTokenService::new()),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    #[tokio::test]
    async fn list_audit_log_query_requires_auth() {
        let schema =
            schema_with_audit_mock(rim_services::audit::service::MockAuditLogService::new());
        let res = schema
            .execute(Request::new(
                "{ listAuditLog { items { id } pageInfo { hasNextPage } } }",
            ))
            .await;
        assert!(
            !res.errors.is_empty(),
            "unauthenticated listAuditLog must return errors"
        );
    }

    #[tokio::test]
    async fn list_audit_log_query_returns_page() {
        use rim_services::audit::dto::{
            AuditLogEntryDto, AuditLogPage, AuditLogPageInfo, AuditOutcome,
        };

        let actor = UserId::new();
        let entry_id = uuid::Uuid::new_v4();
        let server_id = ServerId::new();

        let mut audit_mock = rim_services::audit::service::MockAuditLogService::new();
        audit_mock.expect_list_entries().returning(move |_, _| {
            Box::pin(async move {
                Ok(AuditLogPage {
                    items: vec![AuditLogEntryDto {
                        id: entry_id,
                        actor_user_id: entry_id,
                        server_id: server_id.into(),
                        command_id: "/ping".to_string(),
                        initiator: "palette".to_string(),
                        args_redacted: serde_json::Value::Null,
                        sensitivity: "low".to_string(),
                        dangerous: false,
                        confirmation_kind: "none".to_string(),
                        typed_verb: None,
                        outcome: AuditOutcome::Success,
                        error_kind: None,
                        capability_check_outcome: None,
                        confirmation_outcome: None,
                        parent_audit_id: None,
                        created_at: chrono::Utc::now(),
                    }],
                    next_cursor: None,
                    page_info: AuditLogPageInfo {
                        has_next_page: false,
                        has_prev_page: false,
                    },
                })
            })
        });

        let schema = schema_with_audit_mock(audit_mock);
        let req = authed_request(
            "{ listAuditLog { items { id commandId outcome } pageInfo { hasNextPage hasPrevPage } } }",
            actor,
        );
        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let data = res.data.into_json().unwrap();
        let items = &data["listAuditLog"]["items"];
        assert_eq!(items.as_array().unwrap().len(), 1);
        assert_eq!(items[0]["commandId"], "/ping");
        assert_eq!(items[0]["outcome"], "SUCCESS");
        assert_eq!(data["listAuditLog"]["pageInfo"]["hasNextPage"], false);
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

    // ── P5-A7 command_discover resolver tests ──────────────────────────────

    /// Full round-trip: GQL query → resolver → MockCommandService →
    /// returns `[Command!]!` with expected shape.
    #[tokio::test]
    async fn command_discover_query_returns_ai_callable_commands() {
        let actor = UserId::new();
        let mut command = MockCommandService::new();
        command
            .expect_command_discover()
            .returning(|_actor, _phrase, _limit, _ws_id| {
                Box::pin(async {
                    Ok(vec![
                        rim_services::command::dto::CommandDefinitionDto {
                            id: "chat.list_channels".to_string(),
                            title: "List Channels".to_string(),
                            description: "List all channels in the workspace".to_string(),
                            category: "Chat".to_string(),
                            source_kind: rim_services::command::dto::CommandSourceKindDto::BuiltIn,
                            execution_target:
                                rim_services::command::dto::CommandExecutionTargetDto::Backend,
                            arguments: vec![],
                            required_context: vec![],
                            required_capabilities: vec!["chat.read".to_string()],
                            visible_in_palette: true,
                            visible_in_context_menus: false,
                            keybind_capable: false,
                            dangerous: false,
                            user_command_id: None,
                            nl_description: Some(
                                "Lists all channels the user has access to".to_string(),
                            ),
                            intent_samples: vec!["show channels".to_string()],
                            ai_callable: true,
                            sensitivity:
                                rim_services::command::sensitivity::CommandSensitivityDto::Safe,
                            typed_verb: rim_services::command::verb::TypedVerb::Other,
                            rate_limit_per_minute: None,
                        },
                        rim_services::command::dto::CommandDefinitionDto {
                            id: "chat.post_message".to_string(),
                            title: "Post Message".to_string(),
                            description: "Send a message to a channel".to_string(),
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
                            nl_description: Some("Sends a message to a channel".to_string()),
                            intent_samples: vec!["send message".to_string()],
                            ai_callable: true,
                            sensitivity:
                                rim_services::command::sensitivity::CommandSensitivityDto::Mutating,
                            typed_verb: rim_services::command::verb::TypedVerb::Other,
                            rate_limit_per_minute: None,
                        },
                    ])
                })
            });

        let schema = schema_with_command_mock(command);
        let req = authed_request(
            r#"query {
                commandDiscover {
                    id
                    title
                    aiCallable
                    requiredCapabilities
                    nlDescription
                }
            }"#,
            actor,
        );

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let data = res.data.into_json().unwrap();
        let items = data["commandDiscover"].as_array().unwrap();
        assert_eq!(items.len(), 2, "expected 2 AI-callable commands");
        assert_eq!(items[0]["id"], "chat.list_channels");
        assert_eq!(items[0]["aiCallable"], true);
        assert_eq!(items[1]["id"], "chat.post_message");
        assert_eq!(items[1]["aiCallable"], true);
        // nlDescription must be forwarded through (key field for the AI agent).
        assert_eq!(
            items[0]["nlDescription"],
            "Lists all channels the user has access to"
        );
    }

    /// Verify that `commandDiscover` requires authentication — same gate as all
    /// other actor-gated queries. An unauthenticated call must return a GQL error.
    #[tokio::test]
    async fn command_discover_requires_auth() {
        let command = MockCommandService::new();
        let schema = schema_with_command_mock(command);

        let res = schema
            .execute(Request::new("{ commandDiscover { id } }"))
            .await;

        assert!(
            !res.errors.is_empty(),
            "unauthenticated commandDiscover must return an error"
        );
        assert!(
            res.errors[0].message.contains("unauthenticated"),
            "expected unauthenticated error, got: {:?}",
            res.errors[0].message
        );
    }

    /// Empty result is a valid response — actor with no AI-callable caps.
    #[tokio::test]
    async fn command_discover_returns_empty_list_for_actor_with_no_caps() {
        let actor = UserId::new();
        let mut command = MockCommandService::new();
        command
            .expect_command_discover()
            .returning(|_actor, _phrase, _limit, _ws_id| Box::pin(async { Ok(vec![]) }));

        let schema = schema_with_command_mock(command);
        let req = authed_request("{ commandDiscover { id } }", actor);

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let data = res.data.into_json().unwrap();
        let items = data["commandDiscover"].as_array().unwrap();
        assert!(
            items.is_empty(),
            "actor with no caps must get empty commandDiscover"
        );
    }

    /// Hawkeye gate: 6th `commandDiscover` call in the same 60-second window must
    /// return a `RATE_LIMITED` GQL error. The mock returns `ServiceError::RateLimited`
    /// on the 6th invocation; the resolver must surface it as a GQL error whose
    /// message contains "rate" (case-insensitive).
    #[tokio::test]
    async fn command_discover_rate_limited_on_sixth_call() {
        use std::sync::Arc as StdArc;
        use std::sync::atomic::{AtomicU32, Ordering};

        let actor = UserId::new();
        let call_count = StdArc::new(AtomicU32::new(0));
        let call_count2 = call_count.clone();

        let mut command = MockCommandService::new();
        command
            .expect_command_discover()
            .returning(move |_actor, _phrase, _limit, _ws_id| {
                let count = call_count2.fetch_add(1, Ordering::SeqCst) + 1;
                Box::pin(async move {
                    if count <= 5 {
                        Ok(vec![])
                    } else {
                        Err(rim_services::ServiceError::RateLimited {
                            retry_after_seconds: 42,
                        })
                    }
                })
            });

        let schema = schema_with_command_mock(command);
        let gql = "{ commandDiscover { id } }";

        // First 5 calls must succeed.
        for i in 1..=5 {
            let res = schema.execute(authed_request(gql, actor)).await;
            assert!(
                res.errors.is_empty(),
                "call {i} should succeed, got errors: {:?}",
                res.errors
            );
        }

        // 6th call must return an error.
        let res = schema.execute(authed_request(gql, actor)).await;
        assert!(
            !res.errors.is_empty(),
            "6th commandDiscover call must return a rate-limit error"
        );
        let msg = res.errors[0].message.to_lowercase();
        assert!(
            msg.contains("rate"),
            "expected rate-limit error message, got: {:?}",
            res.errors[0].message
        );
    }

    // Keep these IDs referenced so dead-code elimination doesn't hide imports in coverage builds.
    #[test]
    fn _keep_ids_referenced() {
        let _ = (MessageId::new(), ConversationId::new(), ServerId::new());
    }

    /// Fully-mocked `AppServiceProvider` for tests that only need `export_sdl`
    /// to build (no resolver is actually invoked, so every mock is empty).
    fn test_service_provider() -> Arc<dyn AppServiceProvider> {
        Arc::new(TestAppServiceProvider {
            action: Arc::new(MockActionService::new()),
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(MockAuthService::new()),
            capability: Arc::new(MockCapabilityService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            event: Arc::new(MockEventService::new()),
            integration: Arc::new(MockIntegrationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            setting: Arc::new(MockSettingService::new()),
            storage: Arc::new(MockStorageService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            incident: Arc::new(MockIncidentService::new()),
            ingest_token: Arc::new(MockIngestTokenService::new()),
            skill_profile: Arc::new(MockSkillProfileService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            pipeline: Arc::new(MockPipelineService::new()),
            oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
            escalation_policy: Arc::new(
                rim_services::escalation::service::MockEscalationPolicyService::new(),
            ),
        })
    }

    /// Guards the `wire_common_schema_data` extraction (Track A item 3a,
    /// 2026-07-08): `export_sdl` had no test before this, so the dedup between
    /// it and `build_schema` was unverified. Asserts the SDL is non-empty and
    /// contains a few well-known type markers.
    #[test]
    fn sdl_export_is_nonempty_and_has_core_types() {
        let sdl = export_sdl(
            test_service_provider(),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
        );

        assert!(
            !sdl.is_empty(),
            "export_sdl must return a non-empty SDL string"
        );
        assert!(
            sdl.contains("type Query"),
            "SDL must declare the Query type"
        );
        assert!(
            sdl.contains("type Mutation"),
            "SDL must declare the Mutation type"
        );
    }

    // ── Ingest token (Track B2) resolver tests ────────────────────────────────

    fn schema_with_ingest_token_mocks(
        ingest_token: rim_services::ingest_token::service::MockIngestTokenService,
    ) -> crate::AppSchema {
        build_schema(
            Arc::new(TestAppServiceProvider {
                action: Arc::new(MockActionService::new()),
                audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
                auth: Arc::new(MockAuthService::new()),
                capability: Arc::new(MockCapabilityService::new()),
                command: Arc::new(MockCommandService::new()),
                conversation: Arc::new(MockConversationService::new()),
                event: Arc::new(MockEventService::new()),
                integration: Arc::new(MockIntegrationService::new()),
                keybind: Arc::new(MockKeybindService::new()),
                macro_def: Arc::new(MockMacroService::new()),
                mention: Arc::new(MockMentionService::new()),
                message: Arc::new(MockMessageService::new()),
                attachment: Arc::new(MockMessageAttachmentService::new()),
                notification_preference: Arc::new(MockNotificationPreferenceService::new()),
                password_reset: Arc::new(MockPasswordResetService::new()),
                pin: Arc::new(MockPinService::new()),
                reaction: Arc::new(MockMessageReactionService::new()),
                read_state: Arc::new(MockReadStateService::new()),
                server: Arc::new(MockServerService::new()),
                server_member: Arc::new(MockServerMemberService::new()),
                setting: Arc::new(MockSettingService::new()),
                storage: Arc::new(MockStorageService::new()),
                thread: Arc::new(MockThreadService::new()),
                user: Arc::new(MockUserService::new()),
                request_auth: Arc::new(MockRequestAuthenticator::new()),
                link_preview: Arc::new(MockLinkPreviewService::new()),
                custom_emoji: Arc::new(MockCustomEmojiService::new()),
                channel_lens: Arc::new(MockChannelLensService::new()),
                incident: Arc::new(MockIncidentService::new()),
                ingest_token: Arc::new(ingest_token),
                skill_profile: Arc::new(MockSkillProfileService::new()),
                workspace_state: Arc::new(MockWorkspaceStateService::new()),
                pipeline: Arc::new(MockPipelineService::new()),
                oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
                escalation_policy: Arc::new(
                    rim_services::escalation::service::MockEscalationPolicyService::new(),
                ),
            }),
            InMemoryBroadcaster::new(16),
            test_presence_manager(),
            test_typing_tracker(),
            test_observers(),
            test_registration_context(),
        )
    }

    #[tokio::test]
    async fn create_ingest_token_mutation_requires_auth() {
        let schema = schema_with_ingest_token_mocks(
            rim_services::ingest_token::service::MockIngestTokenService::new(),
        );
        let res = schema
            .execute(Request::new(
                r#"mutation($sid: ID!, $cid: ID!) { createIngestToken(input: { serverId: $sid, conversationId: $cid, label: "prom" }) { rawToken } }"#,
            ).variables(Variables::from_json(serde_json::json!({
                "sid": ServerId::new().to_string(),
                "cid": ConversationId::new().to_string(),
            }))))
            .await;
        assert!(!res.errors.is_empty(), "expected an auth error");
    }

    #[tokio::test]
    async fn create_ingest_token_mutation_returns_raw_token() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let token_id = rim_core::ingest_token::model::IngestTokenId::new();

        let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
        ingest_token.expect_create_token().returning(move |cmd| {
            Box::pin(async move {
                Ok(
                    rim_services::ingest_token::dto::CreateIngestTokenResultDto {
                        token: rim_services::ingest_token::dto::IngestTokenDto {
                            id: token_id,
                            server_id: cmd.server_id,
                            conversation_id: cmd.conversation_id,
                            created_by: cmd.actor.user_id,
                            label: cmd.label,
                            severity_default: cmd.severity_default,
                            created_at: chrono::Utc::now(),
                            revoked_at: None,
                            last_used_at: None,
                        },
                        raw_token: "rim_ingest_test_raw_token".to_string(),
                    },
                )
            })
        });

        let schema = schema_with_ingest_token_mocks(ingest_token);

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!) { createIngestToken(input: { serverId: $sid, conversationId: $cid, label: "prom" }) { rawToken token { id label } } }"#,
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
            json["createIngestToken"]["rawToken"],
            "rim_ingest_test_raw_token"
        );
        assert_eq!(json["createIngestToken"]["token"]["label"], "prom");
    }

    #[tokio::test]
    async fn revoke_ingest_token_mutation_returns_true() {
        let actor = UserId::new();
        let token_id = rim_core::ingest_token::model::IngestTokenId::new();

        let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
        ingest_token
            .expect_revoke_token()
            .returning(|_| Box::pin(async { Ok(()) }));

        let schema = schema_with_ingest_token_mocks(ingest_token);

        let req = authed_request(
            r#"mutation($id: ID!) { revokeIngestToken(tokenId: $id) }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "id": token_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["revokeIngestToken"], true);
    }

    #[tokio::test]
    async fn ingest_tokens_query_returns_list() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let token_id = rim_core::ingest_token::model::IngestTokenId::new();

        let mut ingest_token = rim_services::ingest_token::service::MockIngestTokenService::new();
        ingest_token.expect_list_tokens().returning(move |_| {
            Box::pin(async move {
                Ok(vec![rim_services::ingest_token::dto::IngestTokenDto {
                    id: token_id,
                    server_id,
                    conversation_id: ConversationId::new(),
                    created_by: UserId::new(),
                    label: "prom".to_string(),
                    severity_default: None,
                    created_at: chrono::Utc::now(),
                    revoked_at: None,
                    last_used_at: None,
                }])
            })
        });

        let schema = schema_with_ingest_token_mocks(ingest_token);

        let req = authed_request(
            r#"query($sid: ID!) { ingestTokens(serverId: $sid) { id label } }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["ingestTokens"].as_array().unwrap().len(), 1);
        assert_eq!(json["ingestTokens"][0]["label"], "prom");
    }

    // ── On-call gap-fill: OnCallShift.isOverride + OnCallSchedule.rotationLayers ───────────

    #[tokio::test]
    async fn on_call_shifts_query_exposes_is_override() {
        let actor = UserId::new();
        let schedule_id = rim_core::oncall::model::OnCallScheduleId::new();
        let rotation_user = UserId::new();
        let override_user = UserId::new();
        let from = chrono::Utc::now();
        let to = from + chrono::Duration::hours(2);

        let mut oncall = rim_services::oncall::service::MockOnCallService::new();
        oncall.expect_list_shifts().returning(move |_q| {
            Box::pin(async move {
                Ok(vec![
                    rim_services::oncall::dto::OnCallShiftDto {
                        user_id: Some(rotation_user),
                        start_at: from,
                        end_at: from + chrono::Duration::hours(1),
                        is_override: false,
                    },
                    rim_services::oncall::dto::OnCallShiftDto {
                        user_id: Some(override_user),
                        start_at: from + chrono::Duration::hours(1),
                        end_at: to,
                        is_override: true,
                    },
                ])
            })
        });

        let schema = schema_with_oncall_mock(oncall);

        let req = authed_request(
            r#"query($sid: ID!, $from: DateTime!, $to: DateTime!) {
                onCallShifts(scheduleId: $sid, from: $from, to: $to) { isOverride }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": schedule_id.to_string(),
            "from": from.to_rfc3339(),
            "to": to.to_rfc3339(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        let shifts = json["onCallShifts"].as_array().unwrap();
        assert_eq!(shifts.len(), 2);
        assert_eq!(shifts[0]["isOverride"], serde_json::json!(false));
        assert_eq!(shifts[1]["isOverride"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn on_call_schedule_rotation_layers_field_resolves_nested_layers() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let participant_id = UserId::new();

        let schedule_dto = test_on_call_schedule_dto();
        let schedule_id = schedule_dto.id;

        let mut oncall = rim_services::oncall::service::MockOnCallService::new();
        oncall.expect_list_schedules().returning(move |_q| {
            let dto = schedule_dto.clone();
            Box::pin(async move { Ok(vec![dto]) })
        });
        oncall
            .expect_list_rotation_layers()
            .withf(move |q| q.schedule_id == schedule_id)
            .returning(move |_q| {
                Box::pin(async move {
                    let mut layer = test_rotation_layer_dto();
                    layer.schedule_id = schedule_id;
                    layer.participants = vec![participant_id];
                    Ok(vec![layer])
                })
            });

        let schema = schema_with_oncall_mock(oncall);

        let req = authed_request(
            r#"query($sid: ID!) {
                onCallSchedules(serverId: $sid) {
                    rotationLayers { name participants }
                }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        let layers = json["onCallSchedules"][0]["rotationLayers"]
            .as_array()
            .unwrap();
        assert_eq!(layers.len(), 1);
        assert_eq!(layers[0]["name"], serde_json::json!("Primary rotation"));
        assert_eq!(
            layers[0]["participants"],
            serde_json::json!([participant_id.to_string()])
        );
    }

    // ── Escalation policies (Track C1c) ────────────────────────────────────

    #[tokio::test]
    async fn escalation_policies_query_returns_policies() {
        let actor = UserId::new();
        let server_id = ServerId::new();

        let mut escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        escalation_policy
            .expect_list_policies()
            .returning(move |_q| {
                Box::pin(async move {
                    let mut dto = test_escalation_policy_dto();
                    dto.server_id = server_id;
                    Ok(vec![dto])
                })
            });

        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"query($sid: ID!) {
                escalationPolicies(serverId: $sid) {
                    id name steps { position delaySeconds targetKind targetUserId actions }
                }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        let policies = json["escalationPolicies"].as_array().unwrap();
        assert_eq!(policies.len(), 1);
        assert_eq!(policies[0]["name"], serde_json::json!("Primary"));
        assert_eq!(
            policies[0]["steps"][0]["targetKind"],
            serde_json::json!("USER")
        );
        assert_eq!(
            policies[0]["steps"][0]["actions"][0],
            serde_json::json!("EMAIL")
        );
    }

    #[tokio::test]
    async fn create_escalation_policy_mutation_creates_policy() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let target_user_id = UserId::new();

        let mut escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        escalation_policy
            .expect_create_policy()
            .withf(move |cmd| {
                cmd.server_id == server_id
                    && cmd.name == "Primary"
                    && cmd.steps.len() == 1
                    && cmd.steps[0].target
                        == rim_core::escalation::model::EscalationTarget::User(target_user_id)
            })
            .returning(move |cmd| {
                Box::pin(async move {
                    let mut dto = test_escalation_policy_dto();
                    dto.server_id = server_id;
                    dto.name = cmd.name;
                    Ok(dto)
                })
            });

        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"mutation($sid: ID!, $uid: ID!) {
                createEscalationPolicy(
                    serverId: $sid
                    name: "Primary"
                    steps: [{ delaySeconds: 0, target: { kind: USER, userId: $uid }, actions: [EMAIL] }]
                ) { name }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "uid": target_user_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["createEscalationPolicy"]["name"],
            serde_json::json!("Primary")
        );
    }

    /// The flattened `EscalationTargetInput` rejects a field set for the wrong kind before it
    /// ever reaches the service — mirrors `RotationKindInputGql`'s strict-rejection tests.
    #[tokio::test]
    async fn create_escalation_policy_mutation_rejects_mismatched_target_fields() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let schedule_id = rim_core::oncall::model::OnCallScheduleId::new();

        // No expectations set — the input must be rejected before the service is ever called.
        let escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"mutation($sid: ID!, $schedId: ID!) {
                createEscalationPolicy(
                    serverId: $sid
                    name: "Primary"
                    steps: [{ delaySeconds: 0, target: { kind: ON_CALL, scheduleId: $schedId, userId: $schedId }, actions: [EMAIL] }]
                ) { name }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "schedId": schedule_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert!(
            !res.errors.is_empty(),
            "userId set alongside kind: ON_CALL must be rejected"
        );
    }

    #[tokio::test]
    async fn update_escalation_policy_mutation_renames_policy() {
        let actor = UserId::new();
        let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

        let mut escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        escalation_policy
            .expect_update_policy()
            .withf(move |cmd| {
                cmd.policy_id == policy_id
                    && cmd.name.as_deref() == Some("Renamed")
                    && cmd.steps.is_none()
            })
            .returning(move |cmd| {
                Box::pin(async move {
                    let mut dto = test_escalation_policy_dto();
                    dto.id = cmd.policy_id;
                    dto.name = cmd.name.unwrap();
                    Ok(dto)
                })
            });

        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"mutation($pid: ID!) {
                updateEscalationPolicy(policyId: $pid, name: "Renamed") { name }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "pid": policy_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(
            json["updateEscalationPolicy"]["name"],
            serde_json::json!("Renamed")
        );
    }

    #[tokio::test]
    async fn delete_escalation_policy_mutation_deletes() {
        let actor = UserId::new();
        let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

        let mut escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        escalation_policy
            .expect_delete_policy()
            .withf(move |cmd| cmd.policy_id == policy_id)
            .returning(|_| Box::pin(async { Ok(()) }));

        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"mutation($pid: ID!) { deleteEscalationPolicy(policyId: $pid) }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "pid": policy_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["deleteEscalationPolicy"], serde_json::json!(true));
    }

    #[tokio::test]
    async fn set_channel_escalation_policy_mutation_sets_default() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let policy_id = rim_core::escalation::model::EscalationPolicyId::new();

        let mut escalation_policy =
            rim_services::escalation::service::MockEscalationPolicyService::new();
        escalation_policy
            .expect_set_channel_policy()
            .withf(move |cmd| {
                cmd.server_id == server_id
                    && cmd.conversation_id == conversation_id
                    && cmd.policy_id == policy_id
            })
            .returning(|_| Box::pin(async { Ok(()) }));

        let schema = schema_with_escalation_policy_mock(escalation_policy);

        let req = authed_request(
            r#"mutation($sid: ID!, $cid: ID!, $pid: ID!) {
                setChannelEscalationPolicy(serverId: $sid, conversationId: $cid, policyId: $pid)
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
            "pid": policy_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert_eq!(json["setChannelEscalationPolicy"], serde_json::json!(true));
    }

    // ── acknowledgeIncident + Incident escalation fields (Track C1c) ───────

    #[tokio::test]
    async fn acknowledge_incident_mutation_acknowledges() {
        let actor = UserId::new();
        let incident_id = rim_core::incident::model::IncidentId::new();

        let mut incident = MockIncidentService::new();
        incident
            .expect_acknowledge_incident()
            .withf(move |cmd| cmd.incident_id == incident_id)
            .returning(move |cmd| {
                Box::pin(async move {
                    let mut dto = test_incident_dto();
                    dto.id = cmd.incident_id;
                    dto.acknowledged_at = Some(chrono::Utc::now());
                    dto.acknowledged_by = Some(cmd.actor.user_id);
                    Ok(dto)
                })
            });

        let schema = schema_with_incident_mocks(incident, MockUserService::new());

        let req = authed_request(
            r#"mutation($iid: ID!) {
                acknowledgeIncident(incidentId: $iid) { id acknowledgedAt acknowledgedBy }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "iid": incident_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert!(!json["acknowledgeIncident"]["acknowledgedAt"].is_null());
        assert_eq!(
            json["acknowledgeIncident"]["acknowledgedBy"],
            serde_json::json!(actor.to_string())
        );
    }

    #[tokio::test]
    async fn incident_escalation_run_field_resolves_active_run() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let incident_id = rim_core::incident::model::IncidentId::new();

        let mut incident = MockIncidentService::new();
        incident.expect_get_active_incident().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_incident_dto();
                dto.id = incident_id;
                Ok(Some(dto))
            })
        });
        incident
            .expect_get_active_escalation_run()
            .withf(move |id| *id == incident_id)
            .returning(move |_| {
                Box::pin(async move {
                    Ok(Some(rim_services::escalation::dto::EscalationRunDto {
                        id: rim_core::escalation::model::EscalationRunId::new(),
                        incident_id,
                        policy_id: rim_core::escalation::model::EscalationPolicyId::new(),
                        current_step_index: 1,
                        next_fire_at: Some(chrono::Utc::now()),
                        status: rim_core::escalation::model::EscalationRunStatus::Active,
                    }))
                })
            });

        let schema = schema_with_incident_mocks(incident, MockUserService::new());

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    escalationRun { currentStepIndex status }
                }
            }"#,
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
            json["activeIncident"]["escalationRun"]["currentStepIndex"],
            serde_json::json!(1)
        );
        assert_eq!(
            json["activeIncident"]["escalationRun"]["status"],
            serde_json::json!("ACTIVE")
        );
    }

    /// No active run (never armed, or already completed/cancelled) is `null`, not an error.
    #[tokio::test]
    async fn incident_escalation_run_field_is_null_when_no_active_run() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();

        let mut incident = MockIncidentService::new();
        incident
            .expect_get_active_incident()
            .returning(move |_q| Box::pin(async move { Ok(Some(test_incident_dto())) }));
        incident
            .expect_get_active_escalation_run()
            .returning(|_| Box::pin(async { Ok(None) }));

        let schema = schema_with_incident_mocks(incident, MockUserService::new());

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) { escalationRun { status } }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert!(json["activeIncident"]["escalationRun"].is_null());
    }

    #[tokio::test]
    async fn incident_acknowledged_by_user_field_resolves_nested_user() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();
        let acker_id = UserId::new();

        let mut incident = MockIncidentService::new();
        incident.expect_get_active_incident().returning(move |_q| {
            Box::pin(async move {
                let mut dto = test_incident_dto();
                dto.acknowledged_at = Some(chrono::Utc::now());
                dto.acknowledged_by = Some(acker_id);
                Ok(Some(dto))
            })
        });

        let mut user = MockUserService::new();
        user.expect_get_users_by_ids().returning(move |ids| {
            let ids = ids.to_vec();
            Box::pin(async move {
                Ok(ids
                    .into_iter()
                    .map(|id| {
                        let mut dto = test_user_dto();
                        dto.id = id;
                        dto.username = "acker".to_string();
                        dto
                    })
                    .collect())
            })
        });

        let schema = schema_with_incident_mocks(incident, user);

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    acknowledgedBy
                    acknowledgedByUser { id username }
                }
            }"#,
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
            json["activeIncident"]["acknowledgedBy"],
            serde_json::json!(acker_id.to_string())
        );
        assert_eq!(
            json["activeIncident"]["acknowledgedByUser"]["username"],
            serde_json::json!("acker")
        );
    }

    /// `acknowledgedByUser` must be `null`, not an error, when the incident hasn't been
    /// acknowledged — the loader must not even be invoked (zero expectations on `user`).
    #[tokio::test]
    async fn incident_acknowledged_by_user_field_is_null_when_not_acknowledged() {
        let actor = UserId::new();
        let server_id = ServerId::new();
        let conversation_id = ConversationId::new();

        let mut incident = MockIncidentService::new();
        incident
            .expect_get_active_incident()
            .returning(move |_q| Box::pin(async move { Ok(Some(test_incident_dto())) }));

        let schema = schema_with_incident_mocks(incident, MockUserService::new());

        let req = authed_request(
            r#"query($sid: ID!, $cid: ID!) {
                activeIncident(serverId: $sid, conversationId: $cid) {
                    acknowledgedBy
                    acknowledgedByUser { id }
                }
            }"#,
            actor,
        )
        .variables(Variables::from_json(serde_json::json!({
            "sid": server_id.to_string(),
            "cid": conversation_id.to_string(),
        })));

        let res = schema.execute(req).await;
        assert_no_errors(&res);
        let json = res.data.into_json().unwrap();
        assert!(json["activeIncident"]["acknowledgedBy"].is_null());
        assert!(json["activeIncident"]["acknowledgedByUser"].is_null());
    }
}
