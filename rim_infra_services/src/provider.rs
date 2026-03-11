use std::sync::Arc;

use chrono::Duration;
use rim_services::action::service::ActionService;
use rim_services::auth::service::{AuthService, RequestAuthenticator};
use rim_services::capability::service::CapabilityService;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::event::service::EventService;
use rim_services::integration::service::IntegrationService;
use rim_services::keybind::service::KeybindService;
use rim_services::message::service::MessageService;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_thread::service::ThreadService;
use rim_services::provider::AppServiceProvider;
use rim_services::server::service::ServerService;
use rim_services::server_member::service::ServerMemberService;
use rim_services::setting::service::SettingService;
use rim_services::user::service::UserService;

use crate::repos::AppRepos;

#[derive(Clone)]
pub struct DefaultAppServiceProvider {
    action_service: Arc<dyn ActionService>,
    auth_service: Arc<dyn AuthService>,
    capability_service: Arc<dyn CapabilityService>,
    command_service: Arc<dyn CommandService>,
    conversation_service: Arc<dyn ConversationService>,
    event_service: Arc<dyn EventService>,
    integration_service: Arc<dyn IntegrationService>,
    keybind_service: Arc<dyn KeybindService>,
    message_service: Arc<dyn MessageService>,
    message_attachment_service: Arc<dyn MessageAttachmentService>,
    server_service: Arc<dyn ServerService>,
    server_member_service: Arc<dyn ServerMemberService>,
    setting_service: Arc<dyn SettingService>,
    thread_service: Arc<dyn ThreadService>,
    user_service: Arc<dyn UserService>,
    request_authenticator: Arc<dyn RequestAuthenticator>,
}

impl DefaultAppServiceProvider {
    pub fn new(repos: &AppRepos, bootstrap_secret: Option<String>, session_ttl: Duration) -> Self {
        let action_service: Arc<dyn ActionService> = Arc::new(crate::action::DefaultActionService);
        let auth_service: Arc<dyn AuthService> = Arc::new(crate::auth::DefaultAuthService::new(
            repos.for_auth(),
            bootstrap_secret,
            session_ttl,
        ));
        let capability_service: Arc<dyn CapabilityService> =
            Arc::new(crate::capability::DefaultCapabilityService);
        let conversation_service: Arc<dyn ConversationService> = Arc::new(
            crate::conversation::DefaultConversationService::new(repos.for_conversation()),
        );
        let event_service: Arc<dyn EventService> = Arc::new(crate::event::DefaultEventService);
        let integration_service: Arc<dyn IntegrationService> =
            Arc::new(crate::integration::DefaultIntegrationService);
        let message_service: Arc<dyn MessageService> = Arc::new(
            crate::message::DefaultMessageService::new(repos.for_message()),
        );
        let message_attachment_service: Arc<dyn MessageAttachmentService> = Arc::new(
            crate::message_attachment::DefaultMessageAttachmentService::new(repos.for_attachment()),
        );
        let server_service: Arc<dyn ServerService> =
            Arc::new(crate::server::DefaultServerService::new(repos.for_server()));
        let server_member_service: Arc<dyn ServerMemberService> = Arc::new(
            crate::server_member::DefaultServerMemberService::new(repos.for_server_member()),
        );
        let setting_service: Arc<dyn SettingService> =
            Arc::new(crate::setting::DefaultSettingService);
        let thread_service: Arc<dyn ThreadService> =
            Arc::new(crate::thread::DefaultThreadService::new(repos.for_thread()));
        let user_service: Arc<dyn UserService> =
            Arc::new(crate::user::DefaultUserService::new(repos.for_user()));
        let command_service: Arc<dyn CommandService> =
            Arc::new(crate::command::DefaultCommandService::new(
                repos.for_command(),
                crate::command::CommandServices {
                    conversation_service: Arc::clone(&conversation_service),
                    message_service: Arc::clone(&message_service),
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

        Self {
            action_service,
            auth_service,
            capability_service,
            command_service,
            conversation_service,
            event_service,
            integration_service,
            keybind_service,
            message_service,
            message_attachment_service,
            server_service,
            server_member_service,
            setting_service,
            thread_service,
            user_service,
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
    fn command_service(&self) -> Arc<dyn CommandService> {
        Arc::clone(&self.command_service)
    }
    fn conversation_service(&self) -> Arc<dyn ConversationService> {
        Arc::clone(&self.conversation_service)
    }
    fn event_service(&self) -> Arc<dyn EventService> {
        Arc::clone(&self.event_service)
    }
    fn integration_service(&self) -> Arc<dyn IntegrationService> {
        Arc::clone(&self.integration_service)
    }
    fn keybind_service(&self) -> Arc<dyn KeybindService> {
        Arc::clone(&self.keybind_service)
    }
    fn message_service(&self) -> Arc<dyn MessageService> {
        Arc::clone(&self.message_service)
    }
    fn message_attachment_service(&self) -> Arc<dyn MessageAttachmentService> {
        Arc::clone(&self.message_attachment_service)
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
    fn thread_service(&self) -> Arc<dyn ThreadService> {
        Arc::clone(&self.thread_service)
    }
    fn user_service(&self) -> Arc<dyn UserService> {
        Arc::clone(&self.user_service)
    }
    fn request_authenticator(&self) -> Arc<dyn RequestAuthenticator> {
        Arc::clone(&self.request_authenticator)
    }
}
