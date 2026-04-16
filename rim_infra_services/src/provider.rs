use std::sync::Arc;

use chrono::Duration;
use rim_services::action::service::ActionService;
use rim_services::auth::service::{AuthService, RequestAuthenticator};
use rim_services::capability::service::CapabilityService;
use rim_services::channel_lens::service::ChannelLensService;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::custom_emoji::service::CustomEmojiService;
use rim_services::event::service::EventService;
use rim_services::events::EventObserver;
use rim_services::events::Observers;
use rim_services::incident::service::IncidentService;
use rim_services::integration::service::IntegrationService;
use rim_services::keybind::service::KeybindService;
use rim_services::link_preview::service::LinkPreviewService;
use rim_services::macro_def::service::MacroService;
use rim_services::mention::service::MentionService;
use rim_services::message::service::MessageService;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::message_thread::service::ThreadService;
use rim_services::notification_preference::service::NotificationPreferenceService;
use rim_services::password_reset::service::PasswordResetService;
use rim_services::pin::service::PinService;
use rim_services::pipeline::service::PipelineService;
use rim_services::provider::AppServiceProvider;
use rim_services::read_state::service::ReadStateService;
use rim_services::server::service::ServerService;
use rim_services::server_member::service::ServerMemberService;
use rim_services::setting::service::SettingService;
use rim_services::skill_profile::service::SkillProfileService;
use rim_services::storage::service::StorageService;
use rim_services::user::service::UserService;
use rim_services::workspace_state::service::WorkspaceStateService;
use sea_orm::DatabaseConnection;

use crate::repos::AppRepos;

#[derive(Clone)]
pub struct DefaultAppServiceProvider {
    action_service: Arc<dyn ActionService>,
    auth_service: Arc<dyn AuthService>,
    capability_service: Arc<dyn CapabilityService>,
    channel_lens_service: Arc<dyn ChannelLensService>,
    command_service: Arc<dyn CommandService>,
    conversation_service: Arc<dyn ConversationService>,
    custom_emoji_service: Arc<dyn CustomEmojiService>,
    event_service: Arc<dyn EventService>,
    incident_service: Arc<dyn IncidentService>,
    integration_service: Arc<dyn IntegrationService>,
    keybind_service: Arc<dyn KeybindService>,
    link_preview_service: Arc<dyn LinkPreviewService>,
    macro_service: Arc<dyn MacroService>,
    mention_service: Arc<dyn MentionService>,
    message_service: Arc<dyn MessageService>,
    message_attachment_service: Arc<dyn MessageAttachmentService>,
    message_reaction_service: Arc<dyn MessageReactionService>,
    notification_preference_service: Arc<dyn NotificationPreferenceService>,
    password_reset_service: Arc<dyn PasswordResetService>,
    pin_service: Arc<dyn PinService>,
    read_state_service: Arc<dyn ReadStateService>,
    server_service: Arc<dyn ServerService>,
    server_member_service: Arc<dyn ServerMemberService>,
    setting_service: Arc<dyn SettingService>,
    skill_profile_service: Arc<dyn SkillProfileService>,
    storage_service: Arc<dyn StorageService>,
    thread_service: Arc<dyn ThreadService>,
    user_service: Arc<dyn UserService>,
    workspace_state_service: Arc<dyn WorkspaceStateService>,
    pipeline_service: Arc<dyn PipelineService>,
    request_authenticator: Arc<dyn RequestAuthenticator>,
}

impl DefaultAppServiceProvider {
    pub fn new(
        repos: &AppRepos,
        db: DatabaseConnection,
        bootstrap_secret: Option<String>,
        session_ttl: Duration,
        observers: Observers,
        storage_service: Arc<dyn StorageService>,
    ) -> Self {
        let action_service: Arc<dyn ActionService> = Arc::new(crate::action::DefaultActionService);
        let auth_service: Arc<dyn AuthService> = Arc::new(crate::auth::DefaultAuthService::new(
            repos.for_auth(),
            Some(db.clone()),
            bootstrap_secret,
            session_ttl,
            observers.clone(),
        ));
        let capability_service: Arc<dyn CapabilityService> =
            Arc::new(crate::capability::DefaultCapabilityService);
        let conversation_service: Arc<dyn ConversationService> =
            Arc::new(crate::conversation::DefaultConversationService::new(
                repos.for_conversation(),
                observers.clone(),
            ));
        let event_service: Arc<dyn EventService> = Arc::new(crate::event::DefaultEventService);
        let integration_service: Arc<dyn IntegrationService> =
            Arc::new(crate::integration::DefaultIntegrationService);
        let message_service: Arc<dyn MessageService> =
            Arc::new(crate::message::DefaultMessageService::new(
                repos.for_message(),
                Some(db.clone()),
                observers.clone(),
            ));
        let message_attachment_service: Arc<dyn MessageAttachmentService> = Arc::new(
            crate::message_attachment::DefaultMessageAttachmentService::new(
                repos.for_attachment(),
                Arc::clone(&storage_service),
            ),
        );
        let mention_service: Arc<dyn MentionService> =
            Arc::new(crate::mention::DefaultMentionService::new(
                repos.for_mention(),
                Arc::clone(&message_service),
            ));
        let pin_observers = observers.clone();
        let server_observers = observers.clone();
        let read_state_observers = observers.clone();
        let ws_state_broadcaster = Arc::clone(observers.workspace.broadcaster());
        let pipeline_broadcaster_clone = Arc::clone(observers.workspace.broadcaster());
        let message_reaction_service: Arc<dyn MessageReactionService> =
            Arc::new(crate::message_reaction::DefaultMessageReactionService::new(
                repos.for_reaction(),
                Some(db.clone()),
                observers,
            ));
        let notification_preference_service: Arc<dyn NotificationPreferenceService> = Arc::new(
            crate::notification_preference::DefaultNotificationPreferenceService::new(
                repos.for_notification_preference(),
            ),
        );
        let password_reset_service: Arc<dyn PasswordResetService> =
            Arc::new(crate::password_reset::DefaultPasswordResetService::new(
                crate::password_reset::PasswordResetRepos {
                    user_repo: Arc::clone(&repos.user_repo),
                    password_reset_token_repo: Arc::clone(&repos.password_reset_token_repo),
                },
                Arc::new(crate::email::LogEmailSender),
            ));
        let pin_service: Arc<dyn PinService> = Arc::new(crate::pin::DefaultPinService::new(
            repos.for_pin(),
            pin_observers,
            Arc::clone(&message_service),
        ));
        let read_state_service: Arc<dyn ReadStateService> =
            Arc::new(crate::read_state::DefaultReadStateService::new(
                repos.for_read_state(),
                read_state_observers,
            ));
        let server_service: Arc<dyn ServerService> =
            Arc::new(crate::server::DefaultServerService::new(
                repos.for_server(),
                Some(db.clone()),
                server_observers,
            ));
        let server_member_service: Arc<dyn ServerMemberService> = Arc::new(
            crate::server_member::DefaultServerMemberService::new(repos.for_server_member()),
        );
        let setting_service: Arc<dyn SettingService> =
            Arc::new(crate::setting::DefaultSettingService);
        let thread_service: Arc<dyn ThreadService> =
            Arc::new(crate::thread::DefaultThreadService::new(repos.for_thread()));
        let user_service: Arc<dyn UserService> =
            Arc::new(crate::user::DefaultUserService::new(repos.for_user()));
        let workspace_state_service: Arc<dyn WorkspaceStateService> =
            Arc::new(crate::workspace_state::DefaultWorkspaceStateService::new(
                repos.for_workspace_state(),
                ws_state_broadcaster,
            ));
        let command_service: Arc<dyn CommandService> =
            Arc::new(crate::command::DefaultCommandService::new(
                repos.for_command(),
                crate::command::CommandServices {
                    auth_service: Arc::clone(&auth_service),
                    conversation_service: Arc::clone(&conversation_service),
                    message_service: Arc::clone(&message_service),
                    read_state_service: Arc::clone(&read_state_service),
                    workspace_state_service: Arc::clone(&workspace_state_service),
                },
            ));
        let keybind_service: Arc<dyn KeybindService> =
            Arc::new(crate::keybind::DefaultKeybindService::new(
                repos.for_keybind(),
                crate::keybind::KeybindServices {
                    command_service: Arc::clone(&command_service),
                },
            ));
        let request_authenticator: Arc<dyn RequestAuthenticator> = Arc::new(
            crate::request_auth::DefaultRequestAuthenticator::new(repos.for_request_auth()),
        );
        let link_preview_service: Arc<dyn LinkPreviewService> = Arc::new(
            crate::link_preview::DefaultLinkPreviewService::new(repos.for_link_preview()),
        );
        let custom_emoji_service: Arc<dyn CustomEmojiService> =
            Arc::new(crate::custom_emoji::DefaultCustomEmojiService::new(
                repos.for_custom_emoji(),
                Arc::clone(&storage_service),
            ));
        let macro_service: Arc<dyn MacroService> =
            Arc::new(crate::macro_def::DefaultMacroService::new(
                repos.for_macro(),
                crate::macro_def::MacroServices {
                    command_service: Arc::clone(&command_service),
                },
            ));
        let channel_lens_service: Arc<dyn ChannelLensService> = Arc::new(
            crate::channel_lens::DefaultChannelLensService::new(repos.for_channel_lens()),
        );

        let skill_profile_service: Arc<dyn SkillProfileService> =
            Arc::new(crate::skill_profile::DefaultSkillProfileService::new(
                Arc::clone(&repos.skill_profile_repo),
            ));

        let incident_service: Arc<dyn IncidentService> = Arc::new(
            crate::incident::DefaultIncidentService::new(repos.for_incident(), Some(db.clone())),
        );

        let pipeline_service: Arc<dyn PipelineService> =
            Arc::new(crate::pipeline::DefaultPipelineService::new(
                repos.for_pipeline(),
                Arc::clone(&command_service),
                pipeline_broadcaster_clone,
            ));

        Self {
            action_service,
            auth_service,
            capability_service,
            channel_lens_service,
            command_service,
            conversation_service,
            custom_emoji_service,
            event_service,
            incident_service,
            integration_service,
            keybind_service,
            link_preview_service,
            macro_service,
            mention_service,
            message_service,
            message_attachment_service,
            message_reaction_service,
            notification_preference_service,
            password_reset_service,
            pin_service,
            read_state_service,
            server_service,
            server_member_service,
            setting_service,
            skill_profile_service,
            storage_service,
            thread_service,
            user_service,
            workspace_state_service,
            pipeline_service,
            request_authenticator,
        }
    }
}

impl AppServiceProvider for DefaultAppServiceProvider {
    fn action_service(&self) -> Arc<dyn ActionService> {
        Arc::clone(&self.action_service)
    }
    fn auth_service(&self) -> Arc<dyn AuthService> {
        Arc::clone(&self.auth_service)
    }
    fn capability_service(&self) -> Arc<dyn CapabilityService> {
        Arc::clone(&self.capability_service)
    }
    fn channel_lens_service(&self) -> Arc<dyn ChannelLensService> {
        Arc::clone(&self.channel_lens_service)
    }
    fn command_service(&self) -> Arc<dyn CommandService> {
        Arc::clone(&self.command_service)
    }
    fn conversation_service(&self) -> Arc<dyn ConversationService> {
        Arc::clone(&self.conversation_service)
    }
    fn custom_emoji_service(&self) -> Arc<dyn CustomEmojiService> {
        Arc::clone(&self.custom_emoji_service)
    }
    fn link_preview_service(&self) -> Arc<dyn LinkPreviewService> {
        Arc::clone(&self.link_preview_service)
    }
    fn event_service(&self) -> Arc<dyn EventService> {
        Arc::clone(&self.event_service)
    }
    fn incident_service(&self) -> Arc<dyn IncidentService> {
        Arc::clone(&self.incident_service)
    }
    fn integration_service(&self) -> Arc<dyn IntegrationService> {
        Arc::clone(&self.integration_service)
    }
    fn keybind_service(&self) -> Arc<dyn KeybindService> {
        Arc::clone(&self.keybind_service)
    }
    fn macro_service(&self) -> Arc<dyn MacroService> {
        Arc::clone(&self.macro_service)
    }
    fn mention_service(&self) -> Arc<dyn MentionService> {
        Arc::clone(&self.mention_service)
    }
    fn message_service(&self) -> Arc<dyn MessageService> {
        Arc::clone(&self.message_service)
    }
    fn message_attachment_service(&self) -> Arc<dyn MessageAttachmentService> {
        Arc::clone(&self.message_attachment_service)
    }
    fn message_reaction_service(&self) -> Arc<dyn MessageReactionService> {
        Arc::clone(&self.message_reaction_service)
    }
    fn notification_preference_service(&self) -> Arc<dyn NotificationPreferenceService> {
        Arc::clone(&self.notification_preference_service)
    }
    fn password_reset_service(&self) -> Arc<dyn PasswordResetService> {
        Arc::clone(&self.password_reset_service)
    }
    fn pin_service(&self) -> Arc<dyn PinService> {
        Arc::clone(&self.pin_service)
    }
    fn read_state_service(&self) -> Arc<dyn ReadStateService> {
        Arc::clone(&self.read_state_service)
    }
    fn server_service(&self) -> Arc<dyn ServerService> {
        Arc::clone(&self.server_service)
    }
    fn server_member_service(&self) -> Arc<dyn ServerMemberService> {
        Arc::clone(&self.server_member_service)
    }
    fn setting_service(&self) -> Arc<dyn SettingService> {
        Arc::clone(&self.setting_service)
    }
    fn skill_profile_service(&self) -> Arc<dyn SkillProfileService> {
        Arc::clone(&self.skill_profile_service)
    }
    fn storage_service(&self) -> Arc<dyn StorageService> {
        Arc::clone(&self.storage_service)
    }
    fn thread_service(&self) -> Arc<dyn ThreadService> {
        Arc::clone(&self.thread_service)
    }
    fn user_service(&self) -> Arc<dyn UserService> {
        Arc::clone(&self.user_service)
    }
    fn workspace_state_service(&self) -> Arc<dyn WorkspaceStateService> {
        Arc::clone(&self.workspace_state_service)
    }
    fn pipeline_service(&self) -> Arc<dyn PipelineService> {
        Arc::clone(&self.pipeline_service)
    }
    fn request_authenticator(&self) -> Arc<dyn RequestAuthenticator> {
        Arc::clone(&self.request_authenticator)
    }
}
