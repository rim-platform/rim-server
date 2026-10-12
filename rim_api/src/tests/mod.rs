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
use rim_services::auth::service::{MockAuthService, MockRequestAuthenticator};
use rim_services::channel_lens::service::MockChannelLensService;
use rim_services::command::service::MockCommandService;
use rim_services::conversation::service::MockConversationService;
use rim_services::custom_emoji::service::MockCustomEmojiService;
use rim_services::key_auth::service::MockKeyAuthService;
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
use rim_services::user::service::MockUserService;
use rim_services::workspace_state::service::MockWorkspaceStateService;

use crate::auth_context::AuthActorCtx;
use crate::broadcast::InMemoryBroadcaster;
use crate::build_schema;

mod attachment;
mod audit;
mod auth;
mod command;
mod command_execution;
mod conversation;
mod features;
mod keybind;
mod loaders;
mod message;
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
        audit_retention_days: None,
        instance_policy: Default::default(),
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
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(MockAuthService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(user),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
}

fn schema_with_server_mocks(
    server: MockServerService,
    server_member: MockServerMemberService,
) -> crate::AppSchema {
    build_schema(
        Arc::new(TestAppServiceProvider {
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(MockAuthService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(server),
            server_member: Arc::new(server_member),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
}

fn schema_with_keybind_mock(keybind: MockKeybindService) -> crate::AppSchema {
    build_schema(
        Arc::new(TestAppServiceProvider {
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(MockAuthService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            keybind: Arc::new(keybind),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
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
        encrypted_at: None,
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
        encryption: None,
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
        principal_id: None,
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
        principal_id: None,
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
        blob_sha256: None,
    }
}

// ── Provider / schema builders ────────────────────────────────────────

struct TestAppServiceProvider {
    audit_log: Arc<dyn rim_services::audit::service::AuditLogService>,
    auth: Arc<dyn rim_services::auth::service::AuthService>,
    command: Arc<dyn rim_services::command::service::CommandService>,
    conversation: Arc<dyn rim_services::conversation::service::ConversationService>,
    keybind: Arc<dyn rim_services::keybind::service::KeybindService>,
    macro_def: Arc<dyn rim_services::macro_def::service::MacroService>,
    mention: Arc<dyn rim_services::mention::service::MentionService>,
    message: Arc<dyn rim_services::message::service::MessageService>,
    attachment: Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
    pin: Arc<dyn rim_services::pin::service::PinService>,
    notification_preference:
        Arc<dyn rim_services::notification_preference::service::NotificationPreferenceService>,
    password_reset: Arc<dyn rim_services::password_reset::service::PasswordResetService>,
    key_auth: Arc<dyn rim_services::key_auth::service::KeyAuthService>,
    reaction: Arc<dyn rim_services::message_reaction::service::MessageReactionService>,
    read_state: Arc<dyn rim_services::read_state::service::ReadStateService>,
    server: Arc<dyn rim_services::server::service::ServerService>,
    server_member: Arc<dyn rim_services::server_member::service::ServerMemberService>,
    thread: Arc<dyn rim_services::message_thread::service::ThreadService>,
    user: Arc<dyn rim_services::user::service::UserService>,
    request_auth: Arc<dyn rim_services::auth::service::RequestAuthenticator>,
    link_preview: Arc<dyn rim_services::link_preview::LinkPreviewService>,
    custom_emoji: Arc<dyn rim_services::custom_emoji::service::CustomEmojiService>,
    channel_lens: Arc<dyn rim_services::channel_lens::service::ChannelLensService>,
    workspace_state: Arc<dyn rim_services::workspace_state::service::WorkspaceStateService>,
    moderation: Arc<dyn rim_services::moderation::service::ModerationService>,
    notification: Arc<dyn rim_services::notification::service::NotificationService>,
    pipeline: Arc<dyn rim_services::pipeline::service::PipelineService>,
}

impl AppServiceProvider for TestAppServiceProvider {
    fn audit_log_service(&self) -> Arc<dyn rim_services::audit::service::AuditLogService> {
        Arc::clone(&self.audit_log)
    }

    fn auth_service(&self) -> Arc<dyn rim_services::auth::service::AuthService> {
        Arc::clone(&self.auth)
    }

    fn command_service(&self) -> Arc<dyn rim_services::command::service::CommandService> {
        Arc::clone(&self.command)
    }

    fn conversation_service(
        &self,
    ) -> Arc<dyn rim_services::conversation::service::ConversationService> {
        Arc::clone(&self.conversation)
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

    fn key_auth_service(&self) -> Arc<dyn rim_services::key_auth::service::KeyAuthService> {
        Arc::clone(&self.key_auth)
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

    fn workspace_state_service(
        &self,
    ) -> Arc<dyn rim_services::workspace_state::service::WorkspaceStateService> {
        Arc::clone(&self.workspace_state)
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

    fn device_link_service(
        &self,
    ) -> Arc<dyn rim_services::device_link::service::DeviceLinkService> {
        // No resolver test here calls it, so an unprimed mock is enough.
        Arc::new(rim_services::device_link::service::MockDeviceLinkService::new())
    }

    fn e2ee_service(&self) -> Arc<dyn rim_services::e2ee::service::E2eeService> {
        // No resolver test here calls it, so an unprimed mock is enough.
        Arc::new(rim_services::e2ee::service::MockE2eeService::new())
    }

    fn workspace_plugin_service(
        &self,
    ) -> Arc<dyn rim_services::workspace_plugin::service::WorkspacePluginService> {
        Arc::new(rim_services::workspace_plugin::service::MockWorkspacePluginService::new())
    }

    fn plugin_bundle_service(
        &self,
    ) -> Arc<dyn rim_services::workspace_plugin::service::PluginBundleService> {
        Arc::new(rim_services::workspace_plugin::service::MockPluginBundleService::new())
    }

    fn feature_gate(&self) -> Arc<dyn rim_services::feature::service::FeatureGate> {
        // Every feature on; a test turns one off with `with_features_disabled`.
        Arc::new(rim_infra_services::feature_gate::InstanceFeatureGate::all_enabled())
    }
}

/// `request` on an instance where `disabled` are switched off: request data takes precedence
/// over the schema's.
fn with_features_disabled(
    request: Request,
    disabled: &[rim_core::server_feature::model::ServerFeature],
) -> Request {
    let gate: Arc<dyn rim_services::feature::service::FeatureGate> = Arc::new(
        rim_infra_services::feature_gate::InstanceFeatureGate::new(disabled, &[]),
    );
    request.data(gate)
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
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(auth),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(conv),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(msg),
            attachment: Arc::new(attachment),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            thread: Arc::new(thread),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
}

fn schema_with_command_mock(command: MockCommandService) -> crate::AppSchema {
    build_schema(
        Arc::new(TestAppServiceProvider {
            audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
            auth: Arc::new(MockAuthService::new()),
            command: Arc::new(command),
            conversation: Arc::new(MockConversationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
}

// ── listAuditLog schema builder (tests in `audit`) ───────────────────────

fn schema_with_audit_mock(
    audit_log: rim_services::audit::service::MockAuditLogService,
) -> crate::AppSchema {
    build_schema(
        Arc::new(TestAppServiceProvider {
            audit_log: Arc::new(audit_log),
            auth: Arc::new(MockAuthService::new()),
            command: Arc::new(MockCommandService::new()),
            conversation: Arc::new(MockConversationService::new()),
            keybind: Arc::new(MockKeybindService::new()),
            macro_def: Arc::new(MockMacroService::new()),
            mention: Arc::new(MockMentionService::new()),
            message: Arc::new(MockMessageService::new()),
            attachment: Arc::new(MockMessageAttachmentService::new()),
            notification_preference: Arc::new(MockNotificationPreferenceService::new()),
            password_reset: Arc::new(MockPasswordResetService::new()),
            key_auth: Arc::new(MockKeyAuthService::new()),
            pin: Arc::new(MockPinService::new()),
            reaction: Arc::new(MockMessageReactionService::new()),
            read_state: Arc::new(MockReadStateService::new()),
            server: Arc::new(MockServerService::new()),
            server_member: Arc::new(MockServerMemberService::new()),
            thread: Arc::new(MockThreadService::new()),
            user: Arc::new(MockUserService::new()),
            request_auth: Arc::new(MockRequestAuthenticator::new()),
            link_preview: Arc::new(MockLinkPreviewService::new()),
            custom_emoji: Arc::new(MockCustomEmojiService::new()),
            channel_lens: Arc::new(MockChannelLensService::new()),
            workspace_state: Arc::new(MockWorkspaceStateService::new()),
            moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
            notification: Arc::new(
                rim_services::notification::service::MockNotificationService::new(),
            ),
            pipeline: Arc::new(MockPipelineService::new()),
        }),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
}

/// The SDL of a schema built on [`test_service_provider`].
fn test_sdl() -> String {
    crate::build_schema(
        test_service_provider(),
        InMemoryBroadcaster::new(16),
        test_presence_manager(),
        test_typing_tracker(),
        test_observers(),
        test_registration_context(),
        None,
    )
    .sdl()
}

/// Fully-mocked `AppServiceProvider` for tests that only need the schema to build (no resolver
/// is actually invoked, so every mock is empty).
fn test_service_provider() -> Arc<dyn AppServiceProvider> {
    Arc::new(TestAppServiceProvider {
        audit_log: Arc::new(rim_services::audit::service::MockAuditLogService::new()),
        auth: Arc::new(MockAuthService::new()),
        command: Arc::new(MockCommandService::new()),
        conversation: Arc::new(MockConversationService::new()),
        keybind: Arc::new(MockKeybindService::new()),
        macro_def: Arc::new(MockMacroService::new()),
        mention: Arc::new(MockMentionService::new()),
        message: Arc::new(MockMessageService::new()),
        attachment: Arc::new(MockMessageAttachmentService::new()),
        notification_preference: Arc::new(MockNotificationPreferenceService::new()),
        password_reset: Arc::new(MockPasswordResetService::new()),
        key_auth: Arc::new(MockKeyAuthService::new()),
        pin: Arc::new(MockPinService::new()),
        reaction: Arc::new(MockMessageReactionService::new()),
        read_state: Arc::new(MockReadStateService::new()),
        server: Arc::new(MockServerService::new()),
        server_member: Arc::new(MockServerMemberService::new()),
        thread: Arc::new(MockThreadService::new()),
        user: Arc::new(MockUserService::new()),
        request_auth: Arc::new(MockRequestAuthenticator::new()),
        link_preview: Arc::new(MockLinkPreviewService::new()),
        custom_emoji: Arc::new(MockCustomEmojiService::new()),
        channel_lens: Arc::new(MockChannelLensService::new()),
        workspace_state: Arc::new(MockWorkspaceStateService::new()),
        moderation: Arc::new(rim_services::moderation::service::MockModerationService::new()),
        notification: Arc::new(rim_services::notification::service::MockNotificationService::new()),
        pipeline: Arc::new(MockPipelineService::new()),
    })
}
