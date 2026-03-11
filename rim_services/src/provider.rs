use std::sync::Arc;

use crate::action::service::ActionService;
use crate::auth::service::{AuthService, RequestAuthenticator};
use crate::capability::service::CapabilityService;
use crate::command::service::CommandService;
use crate::conversation::service::ConversationService;
use crate::event::service::EventService;
use crate::integration::service::IntegrationService;
use crate::keybind::service::KeybindService;
use crate::message::service::MessageService;
use crate::message_attachment::service::MessageAttachmentService;
use crate::message_thread::service::ThreadService;
use crate::server::service::ServerService;
use crate::server_member::service::ServerMemberService;
use crate::setting::service::SettingService;
use crate::user::service::UserService;

pub trait AppServiceProvider: Send + Sync {
    fn action_service(&self) -> Arc<dyn ActionService>;
    fn auth_service(&self) -> Arc<dyn AuthService>;
    fn capability_service(&self) -> Arc<dyn CapabilityService>;
    fn command_service(&self) -> Arc<dyn CommandService>;
    fn conversation_service(&self) -> Arc<dyn ConversationService>;
    fn event_service(&self) -> Arc<dyn EventService>;
    fn integration_service(&self) -> Arc<dyn IntegrationService>;
    fn keybind_service(&self) -> Arc<dyn KeybindService>;
    fn message_service(&self) -> Arc<dyn MessageService>;
    fn message_attachment_service(&self) -> Arc<dyn MessageAttachmentService>;
    fn server_service(&self) -> Arc<dyn ServerService>;
    fn server_member_service(&self) -> Arc<dyn ServerMemberService>;
    fn setting_service(&self) -> Arc<dyn SettingService>;
    fn thread_service(&self) -> Arc<dyn ThreadService>;
    fn user_service(&self) -> Arc<dyn UserService>;
    fn request_authenticator(&self) -> Arc<dyn RequestAuthenticator>;
}
