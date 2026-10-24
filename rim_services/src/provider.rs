use std::sync::Arc;

use crate::account_deletion::service::AccountDeletionService;
use crate::audit::service::AuditLogService;
use crate::auth::service::{AuthService, RequestAuthenticator};
use crate::channel_lens::service::ChannelLensService;
use crate::command::service::CommandService;
use crate::conversation::service::ConversationService;
use crate::custom_emoji::service::CustomEmojiService;
use crate::device_link::service::DeviceLinkService;
use crate::feature::service::FeatureGate;
use crate::ip_block::service::IpBlockService;
use crate::key_auth::service::KeyAuthService;
use crate::keybind::service::KeybindService;
use crate::link_preview::service::LinkPreviewService;
use crate::macro_def::service::MacroService;
use crate::mention::service::MentionService;
use crate::message::service::MessageService;
use crate::message_attachment::service::MessageAttachmentService;
use crate::message_reaction::service::MessageReactionService;
use crate::message_thread::service::ThreadService;
use crate::notification_preference::service::NotificationPreferenceService;
use crate::operator::InstanceOperatorService;
use crate::password_reset::service::PasswordResetService;
use crate::pin::service::PinService;
use crate::pipeline::service::PipelineService;
use crate::read_state::service::ReadStateService;
use crate::relay::service::RelayAbuseService;
use crate::server::service::ServerService;
use crate::server_member::service::ServerMemberService;
use crate::user::service::UserService;
use crate::workspace_state::service::WorkspaceStateService;

pub trait AppServiceProvider: Send + Sync {
    fn audit_log_service(&self) -> Arc<dyn AuditLogService>;
    fn auth_service(&self) -> Arc<dyn AuthService>;
    fn channel_lens_service(&self) -> Arc<dyn ChannelLensService>;
    fn command_service(&self) -> Arc<dyn CommandService>;
    fn conversation_service(&self) -> Arc<dyn ConversationService>;
    fn custom_emoji_service(&self) -> Arc<dyn CustomEmojiService>;
    fn link_preview_service(&self) -> Arc<dyn LinkPreviewService>;
    fn key_auth_service(&self) -> Arc<dyn KeyAuthService>;
    fn keybind_service(&self) -> Arc<dyn KeybindService>;
    fn macro_service(&self) -> Arc<dyn MacroService>;
    fn mention_service(&self) -> Arc<dyn MentionService>;
    fn message_service(&self) -> Arc<dyn MessageService>;
    fn message_attachment_service(&self) -> Arc<dyn MessageAttachmentService>;
    fn message_reaction_service(&self) -> Arc<dyn MessageReactionService>;
    fn notification_preference_service(&self) -> Arc<dyn NotificationPreferenceService>;
    fn password_reset_service(&self) -> Arc<dyn PasswordResetService>;
    fn pin_service(&self) -> Arc<dyn PinService>;
    fn read_state_service(&self) -> Arc<dyn ReadStateService>;
    fn server_service(&self) -> Arc<dyn ServerService>;
    fn server_member_service(&self) -> Arc<dyn ServerMemberService>;
    fn thread_service(&self) -> Arc<dyn ThreadService>;
    fn user_service(&self) -> Arc<dyn UserService>;
    fn moderation_service(&self) -> Arc<dyn crate::moderation::service::ModerationService>;
    fn notification_service(&self) -> Arc<dyn crate::notification::service::NotificationService>;
    fn workspace_state_service(&self) -> Arc<dyn WorkspaceStateService>;
    fn pipeline_service(&self) -> Arc<dyn PipelineService>;
    fn request_authenticator(&self) -> Arc<dyn RequestAuthenticator>;
    fn device_link_service(&self) -> Arc<dyn DeviceLinkService>;
    fn e2ee_service(&self) -> Arc<dyn crate::e2ee::service::E2eeService>;
    fn workspace_plugin_service(
        &self,
    ) -> Arc<dyn crate::workspace_plugin::service::WorkspacePluginService>;
    fn plugin_bundle_service(
        &self,
    ) -> Arc<dyn crate::workspace_plugin::service::PluginBundleService>;
    /// The instance's feature switches.
    fn feature_gate(&self) -> Arc<dyn FeatureGate>;
    fn account_deletion_service(&self) -> Arc<dyn AccountDeletionService>;
    /// The instance operator's abuse tools: principal blocks and abuse reports.
    fn relay_abuse_service(&self) -> Arc<dyn RelayAbuseService>;
    /// The instance operator's IP deny list.
    fn ip_block_service(&self) -> Arc<dyn IpBlockService>;
    /// Who is an instance operator.
    fn instance_operator_service(&self) -> Arc<dyn InstanceOperatorService>;
    /// Workspaces' DM rules and each person's choice of who may start a DM with them.
    fn dm_policy_service(&self) -> Arc<dyn crate::dm_policy::service::DmPolicyService>;
    /// The workspace new accounts join.
    fn default_workspace_service(
        &self,
    ) -> Arc<dyn crate::default_workspace::DefaultWorkspaceService>;
    /// Workspaces' public join links and the join requests filed through them.
    fn public_join_link_service(
        &self,
    ) -> Arc<dyn crate::public_join_link::service::PublicJoinLinkService>;
    /// The password of local accounts: registration, login start, change, reset and setting
    /// a first one.
    fn account_password_service(
        &self,
    ) -> Arc<dyn crate::account_password::service::AccountPasswordService>;
    /// The keep keys and reload keys of sessions, and the reload window.
    fn session_key_service(&self) -> Arc<dyn crate::session_key::service::SessionKeyService>;
}
