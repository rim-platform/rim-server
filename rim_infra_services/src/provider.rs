use std::sync::Arc;

use crate::email::EmailSender;
use chrono::Duration;
use rim_services::action::service::ActionService;
use rim_services::audit::service::AuditLogService;
use rim_services::auth::service::{AuthService, RequestAuthenticator};
use rim_services::capability::service::CapabilityService;
use rim_services::channel_lens::service::ChannelLensService;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::custom_emoji::service::CustomEmojiService;
use rim_services::escalation::scheduler::EscalationScheduler;
use rim_services::escalation::service::EscalationPolicyService;
use rim_services::event::service::EventService;
use rim_services::events::EventObserver;
use rim_services::events::Observers;
use rim_services::incident::service::IncidentService;
use rim_services::ingest_token::service::IngestTokenService;
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
use rim_services::oncall::service::OnCallService;
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
use rim_services::workspace_feature::service::WorkspaceFeatureService;
use rim_services::workspace_state::service::WorkspaceStateService;
use sea_orm::DatabaseConnection;

use crate::repos::AppRepos;

#[derive(Clone)]
pub struct DefaultAppServiceProvider {
    action_service: Arc<dyn ActionService>,
    audit_log_service: Arc<dyn AuditLogService>,
    auth_service: Arc<dyn AuthService>,
    capability_service: Arc<dyn CapabilityService>,
    channel_lens_service: Arc<dyn ChannelLensService>,
    command_service: Arc<dyn CommandService>,
    conversation_service: Arc<dyn ConversationService>,
    custom_emoji_service: Arc<dyn CustomEmojiService>,
    escalation_policy_service: Arc<dyn EscalationPolicyService>,
    event_service: Arc<dyn EventService>,
    incident_service: Arc<dyn IncidentService>,
    ingest_token_service: Arc<dyn IngestTokenService>,
    integration_service: Arc<dyn IntegrationService>,
    keybind_service: Arc<dyn KeybindService>,
    link_preview_service: Arc<dyn LinkPreviewService>,
    macro_service: Arc<dyn MacroService>,
    mention_service: Arc<dyn MentionService>,
    message_service: Arc<dyn MessageService>,
    message_attachment_service: Arc<dyn MessageAttachmentService>,
    message_reaction_service: Arc<dyn MessageReactionService>,
    notification_preference_service: Arc<dyn NotificationPreferenceService>,
    oncall_service: Arc<dyn OnCallService>,
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
    workspace_feature_service: Arc<dyn WorkspaceFeatureService>,
    workspace_state_service: Arc<dyn WorkspaceStateService>,
    pipeline_service: Arc<dyn PipelineService>,
    request_authenticator: Arc<dyn RequestAuthenticator>,
}

impl DefaultAppServiceProvider {
    /// Composition-root constructor — wires every service impl from shared repos/config. Its
    /// argument count grows with each cross-cutting concern (observers, storage, ingest-token
    /// TTL, email); it has exactly one call site (`rim_server/src/app.rs`), so a builder isn't
    /// worth the indirection.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: &AppRepos,
        db: DatabaseConnection,
        bootstrap_secret: Option<String>,
        session_ttl: Duration,
        observers: Observers,
        storage_service: Arc<dyn StorageService>,
        ingest_token_ttl_seconds: Option<u64>,
        email_sender: Arc<dyn EmailSender>,
        public_base_url: Option<String>,
        escalation_scheduler: Arc<dyn EscalationScheduler>,
    ) -> Self {
        let action_service: Arc<dyn ActionService> = Arc::new(crate::action::DefaultActionService);
        // Shared so a password reset can forget the sources login exempts from its backstop.
        let known_login_sources = crate::login_sources::KnownLoginSources::login_default();
        let auth_service: Arc<dyn AuthService> = Arc::new(
            crate::auth::DefaultAuthService::new(
                repos.for_auth(),
                Some(db.clone()),
                bootstrap_secret,
                session_ttl,
                observers.clone(),
                Arc::clone(&email_sender),
                public_base_url.clone(),
            )
            .with_known_login_sources(known_login_sources.clone()),
        );
        let capability_service: Arc<dyn CapabilityService> = Arc::new(
            crate::capability::DefaultCapabilityService::new(repos.for_capability()),
        );
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
        let incident_observers = observers.clone();
        let ws_state_broadcaster = Arc::clone(observers.broadcaster());
        let pipeline_broadcaster_clone = Arc::clone(observers.broadcaster());
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
        let password_reset_service: Arc<dyn PasswordResetService> = Arc::new(
            crate::password_reset::DefaultPasswordResetService::new(
                repos.for_password_reset(),
                Some(db.clone()),
                Arc::clone(&email_sender),
                public_base_url.clone(),
            )
            .with_known_login_sources(known_login_sources),
        );
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
        let audit_log_repo: Arc<dyn rim_core::audit::repository::AuditLogRepository> = Arc::new(
            rim_infra_db::sea_orm_impl::audit::SeaOrmAuditLogRepository::new(
                rim_infra_db::DbHandle::from(db.clone()),
            ),
        );
        let audit_log_service: Arc<dyn rim_services::audit::service::AuditLogService> =
            Arc::new(crate::audit::DefaultAuditLogService::new(
                Arc::clone(&audit_log_repo),
                Arc::clone(&capability_service),
            ));
        let workspace_feature_service: Arc<dyn WorkspaceFeatureService> = Arc::new(
            crate::workspace_feature::DefaultWorkspaceFeatureService::new(
                repos.for_workspace_feature(),
            )
            .with_audit_log_service(Arc::clone(&audit_log_service)),
        );
        // Shared confirmation store — same instance used by command_service and
        // macro_service so that a proof issued by command_service can be verified
        // by macro_service (and vice versa, once macro-level challenge issuance lands).
        let confirmation_store: std::sync::Arc<
            dyn rim_services::command::confirmation::ConfirmationStore,
        > = rim_services::command::confirmation::shared_in_memory_store();

        // C1b (iron-man's note, §10.1): incident/oncall/escalation have no back-dependency on
        // command_service, so they're built here — before command_service — and injected
        // directly into `CommandServices`. This mirrors the plain-order pattern everywhere else
        // in this constructor; unlike the command↔macro pair, there is no init cycle here, so no
        // `OnceLock` late-binding is needed.
        let oncall_service: Arc<dyn OnCallService> =
            Arc::new(crate::oncall::DefaultOnCallService::new(repos.for_oncall()));

        let escalation_policy_service: Arc<dyn EscalationPolicyService> = Arc::new(
            crate::escalation::DefaultEscalationPolicyService::new(repos.for_escalation_policy()),
        );

        let incident_service: Arc<dyn IncidentService> =
            Arc::new(crate::incident::DefaultIncidentService::new(
                repos.for_incident(),
                Some(db.clone()),
                incident_observers,
                Arc::clone(&email_sender),
                public_base_url.clone(),
                Arc::clone(&message_service),
                Arc::clone(&escalation_scheduler),
            ));

        let command_services_args = || crate::command::CommandServices {
            auth_service: Arc::clone(&auth_service),
            capability_service: Arc::clone(&capability_service),
            conversation_service: Arc::clone(&conversation_service),
            message_service: Arc::clone(&message_service),
            message_reaction_service: Arc::clone(&message_reaction_service),
            pin_service: Arc::clone(&pin_service),
            read_state_service: Arc::clone(&read_state_service),
            server_member_service: Arc::clone(&server_member_service),
            user_service: Arc::clone(&user_service),
            workspace_state_service: Arc::clone(&workspace_state_service),
            audit_log_service: Some(Arc::clone(&audit_log_service)),
            incident_service: Arc::clone(&incident_service),
            oncall_service: Arc::clone(&oncall_service),
            workspace_feature_service: Arc::clone(&workspace_feature_service),
        };
        let command_service_concrete = Arc::new(
            crate::command::DefaultCommandService::with_confirmation_store(
                repos.for_command(),
                command_services_args(),
                Arc::clone(&confirmation_store),
            ),
        );
        let command_service: Arc<dyn CommandService> =
            Arc::clone(&command_service_concrete) as Arc<dyn CommandService>;
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
                    audit_log_service: Some(Arc::clone(&audit_log_service)),
                    confirmation_store: Some(Arc::clone(&confirmation_store)),
                },
            ));

        // Break the command↔macro init cycle: the command service is constructed
        // first (macro cell empty), then the macro service is built from it, then
        // the macro service is set back into the single command-service instance.
        // keybind/pipeline hold the same Arc, so they observe macros too — no
        // comment-only invariant.
        let _ = command_service_concrete.set_macro_service(Arc::clone(&macro_service));

        let channel_lens_service: Arc<dyn ChannelLensService> = Arc::new(
            crate::channel_lens::DefaultChannelLensService::new(repos.for_channel_lens()),
        );

        let skill_profile_service: Arc<dyn SkillProfileService> =
            Arc::new(crate::skill_profile::DefaultSkillProfileService::new(
                Arc::clone(&repos.skill_profile_repo),
            ));

        let ingest_token_service: Arc<dyn IngestTokenService> =
            Arc::new(crate::ingest_token::DefaultIngestTokenService::new(
                repos.for_ingest_token(),
                ingest_token_ttl_seconds,
            ));

        let pipeline_service: Arc<dyn PipelineService> =
            Arc::new(crate::pipeline::DefaultPipelineService::new(
                repos.for_pipeline(),
                Arc::clone(&command_service),
                pipeline_broadcaster_clone,
            ));

        Self {
            action_service,
            audit_log_service,
            auth_service,
            capability_service,
            channel_lens_service,
            command_service,
            conversation_service,
            custom_emoji_service,
            escalation_policy_service,
            event_service,
            incident_service,
            ingest_token_service,
            integration_service,
            keybind_service,
            link_preview_service,
            macro_service,
            mention_service,
            message_service,
            message_attachment_service,
            message_reaction_service,
            notification_preference_service,
            oncall_service,
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
            workspace_feature_service,
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
    fn audit_log_service(&self) -> Arc<dyn AuditLogService> {
        Arc::clone(&self.audit_log_service)
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
    fn escalation_policy_service(&self) -> Arc<dyn EscalationPolicyService> {
        Arc::clone(&self.escalation_policy_service)
    }
    fn event_service(&self) -> Arc<dyn EventService> {
        Arc::clone(&self.event_service)
    }
    fn incident_service(&self) -> Arc<dyn IncidentService> {
        Arc::clone(&self.incident_service)
    }
    fn ingest_token_service(&self) -> Arc<dyn IngestTokenService> {
        Arc::clone(&self.ingest_token_service)
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
    fn oncall_service(&self) -> Arc<dyn OnCallService> {
        Arc::clone(&self.oncall_service)
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
    fn workspace_feature_service(&self) -> Arc<dyn WorkspaceFeatureService> {
        Arc::clone(&self.workspace_feature_service)
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
