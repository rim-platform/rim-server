//! Resolver unit tests against a fully mocked `AppServiceProvider`.
//!
//! This module holds the shared fixtures (mock provider, schema builders,
//! DTO factories, request helpers); the tests themselves live in one
//! submodule per domain, mirroring `crate::query` / `crate::mutation`.

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
use rim_services::workspace_feature::service::MockWorkspaceFeatureService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

use crate::auth_context::AuthActorCtx;
use crate::broadcast::InMemoryBroadcaster;
use crate::build_schema;
use crate::export_sdl;

mod attachment;
mod audit;
mod auth;
mod command;
mod command_execution;
mod conversation;
mod escalation;
mod incident;
mod ingest_token;
mod keybind;
mod message;
mod oncall;
mod platform;
mod presence;
mod profile;
mod schema;
mod thread;
mod workspace;

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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
        is_private: false,
        archived_at: None,
        created_by: None,
        slow_mode_seconds: 0,
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
        member_role: rim_core::server_member::model::ServerMemberRole::Member,
        joined_at: chrono::Utc::now(),
        timeout_until: None,
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
    workspace_feature: Arc<dyn rim_services::workspace_feature::service::WorkspaceFeatureService>,
    moderation: Arc<dyn rim_services::moderation::service::ModerationService>,
    notification: Arc<dyn rim_services::notification::service::NotificationService>,
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

    fn capability_service(&self) -> Arc<dyn rim_services::capability::service::CapabilityService> {
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

    fn read_state_service(&self) -> Arc<dyn rim_services::read_state::service::ReadStateService> {
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

    fn request_authenticator(&self) -> Arc<dyn rim_services::auth::service::RequestAuthenticator> {
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

    fn workspace_feature_service(
        &self,
    ) -> Arc<dyn rim_services::workspace_feature::service::WorkspaceFeatureService> {
        Arc::clone(&self.workspace_feature)
    }
    fn moderation_service(&self) -> Arc<dyn rim_services::moderation::service::ModerationService> {
        Arc::clone(&self.moderation)
    }

    fn notification_service(
        &self,
    ) -> Arc<dyn rim_services::notification::service::NotificationService> {
        Arc::clone(&self.notification)
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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

// ── listAuditLog schema builder (tests in `audit`) ───────────────────────

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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
        workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
        moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
        notification: Arc::new(rim_services::notification::service::MockNotificationService::new()),
        pipeline: Arc::new(MockPipelineService::new()),
        oncall: Arc::new(rim_services::oncall::service::MockOnCallService::new()),
        escalation_policy: Arc::new(
            rim_services::escalation::service::MockEscalationPolicyService::new(),
        ),
    })
}

// ── Ingest token (Track B2) schema builder (tests in `ingest_token`) ──────

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
            workspace_feature: Arc::new(MockWorkspaceFeatureService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
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
