use std::sync::Arc;

use crate::account_deletion::DefaultAccountDeletionService;
use crate::email::EmailSender;
use chrono::Duration;
use rim_services::account_deletion::service::AccountDeletionService;
use rim_services::audit::service::AuditLogService;
use rim_services::auth::service::{AuthService, RequestAuthenticator};
use rim_services::capability::service::CapabilityService;
use rim_services::channel_lens::service::ChannelLensService;
use rim_services::command::service::CommandService;
use rim_services::conversation::service::ConversationService;
use rim_services::custom_emoji::service::CustomEmojiService;
use rim_services::default_workspace::DefaultWorkspaceService;
use rim_services::device_link::service::DeviceLinkService;
use rim_services::events::Observers;
use rim_services::feature::service::FeatureGate;
use rim_services::ip_block::service::IpBlockService;
use rim_services::key_auth::service::KeyAuthService;
use rim_services::keybind::service::KeybindService;
use rim_services::limits::InstanceLimits;
use rim_services::link_preview::service::LinkPreviewService;
use rim_services::macro_def::service::MacroService;
use rim_services::mention::service::MentionService;
use rim_services::message::service::MessageService;
use rim_services::message_attachment::service::MessageAttachmentService;
use rim_services::message_reaction::service::MessageReactionService;
use rim_services::message_thread::service::ThreadService;
use rim_services::moderation::service::ModerationService;
use rim_services::notification::service::NotificationService;
use rim_services::notification_preference::service::NotificationPreferenceService;
use rim_services::operator::InstanceOperatorService;
use rim_services::password_reset::service::PasswordResetService;
use rim_services::pin::service::PinService;
use rim_services::pipeline::service::PipelineService;
use rim_services::provider::AppServiceProvider;
use rim_services::read_state::service::ReadStateService;
use rim_services::relay::service::RelayAbuseService;
use rim_services::server::service::ServerService;
use rim_services::server_member::service::ServerMemberService;
use rim_services::session_revocation::SessionRevocations;
use rim_services::storage::service::StorageService;
use rim_services::user::service::UserService;
use rim_services::workspace_plugin::service::{PluginBundleService, WorkspacePluginService};
use rim_services::workspace_state::service::WorkspaceStateService;
use sea_orm::DatabaseConnection;

use crate::repos::AppRepos;

/// What the services need from this server's relay and host roles. Empty in tests that do not
/// cross servers.
#[derive(Clone, Default)]
pub struct FederationHooks {
    /// The relay's direct space.
    pub direct_space_id: Option<rim_core::server::model::ServerId>,
    /// Claims of remote users' key packages.
    pub remote_key_packages: Option<Arc<dyn rim_services::host::service::RemoteKeyPackages>>,
    /// Told when a local user's card or devices change.
    pub relay_service: Option<Arc<dyn rim_services::relay::service::RelayService>>,
    /// This server's id, which proof-of-work signups are bound to.
    pub server_id: Option<String>,
    /// What each local user may hold and send.
    pub quotas: crate::quota::Quotas,
    /// Refreshes a remote user's identity before they sign in directly.
    pub host_service: Option<Arc<dyn rim_services::host::service::HostService>>,
    /// Sends a local device's revocation to every host holding its person.
    pub revocation_fanout:
        Option<Arc<dyn rim_services::remote_workspace::service::RevocationFanout>>,
    /// Usernames and display names signups and profile edits refuse.
    pub reserved_names: crate::reserved_names::ReservedNames,
    /// Runs Argon2 hashing and verification for signups, logins and password resets.
    pub password_hashing: crate::password_hashing::PasswordHashing,
    /// The terms of service URL every signup must accept; `None` when there are none.
    pub terms_url: Option<String>,
    /// Refuse every password signup, so new accounts sign up with a key.
    pub signup_requires_keys: bool,
    /// The operator's DM settings.
    pub dm_policy: rim_services::dm_policy::dto::DmOperatorPolicy,
    /// The workspace every new account joins as a Member, and the definition an owner claim
    /// creates it from.
    pub default_workspace: crate::default_workspace::DefaultWorkspace,
    /// This server's handle domain (`RIM_HANDLE_DOMAIN`); `None` gives local accounts no handle.
    pub handle_domain: Option<String>,
    /// The minimum age every signup must confirm; `None` when there is none.
    pub minimum_age: Option<u8>,
    /// The key public join link tokens are sealed under; `None` leaves public links unavailable.
    pub link_key: Option<crate::link_key::LinkKey>,
    /// What public join links start with, `<link base>/join/<token>`; `None` gives
    /// `http://localhost`.
    pub link_base: Option<String>,
}

#[derive(Clone)]
pub struct DefaultAppServiceProvider {
    audit_log_service: Arc<dyn AuditLogService>,
    auth_service: Arc<dyn AuthService>,
    channel_lens_service: Arc<dyn ChannelLensService>,
    command_service: Arc<dyn CommandService>,
    conversation_service: Arc<dyn ConversationService>,
    custom_emoji_service: Arc<dyn CustomEmojiService>,
    key_auth_service: Arc<dyn KeyAuthService>,
    keybind_service: Arc<dyn KeybindService>,
    link_preview: crate::link_preview::DefaultLinkPreviewService,
    macro_service: Arc<dyn MacroService>,
    mention_service: Arc<dyn MentionService>,
    message_service: Arc<dyn MessageService>,
    message_attachment_service: Arc<dyn MessageAttachmentService>,
    message_reaction_service: Arc<dyn MessageReactionService>,
    moderation_service: Arc<dyn ModerationService>,
    notification_service: Arc<dyn NotificationService>,
    notification_preference_service: Arc<dyn NotificationPreferenceService>,
    password_reset_service: Arc<dyn PasswordResetService>,
    pin_service: Arc<dyn PinService>,
    read_state_service: Arc<dyn ReadStateService>,
    server_service: Arc<dyn ServerService>,
    server_member_service: Arc<dyn ServerMemberService>,
    thread_service: Arc<dyn ThreadService>,
    user_service: Arc<dyn UserService>,
    workspace_state_service: Arc<dyn WorkspaceStateService>,
    pipeline_service: Arc<dyn PipelineService>,
    request_authenticator: Arc<dyn RequestAuthenticator>,
    device_link_service: Arc<dyn DeviceLinkService>,
    e2ee_service: Arc<dyn rim_services::e2ee::service::E2eeService>,
    workspace_plugin_service: Arc<dyn WorkspacePluginService>,
    plugin_bundle_service: Arc<dyn PluginBundleService>,
    feature_gate: Arc<dyn FeatureGate>,
    account_deletion: DefaultAccountDeletionService,
    session_revocations: Arc<dyn SessionRevocations>,
    relay_abuse_service: Arc<dyn RelayAbuseService>,
    ip_block_service: Arc<dyn IpBlockService>,
    instance_operator_service: Arc<dyn InstanceOperatorService>,
    dm_policy_service: Arc<dyn rim_services::dm_policy::service::DmPolicyService>,
    default_workspace_service: Arc<dyn DefaultWorkspaceService>,
    public_join_link_service:
        Arc<dyn rim_services::public_join_link::service::PublicJoinLinkService>,
}

impl DefaultAppServiceProvider {
    /// Composition-root constructor — wires every service impl from shared repos/config. Its
    /// argument count grows with each cross-cutting concern (observers, storage, email); it has exactly one call site (`rim_server/src/app.rs`), so a builder isn't
    /// worth the indirection.
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        repos: &AppRepos,
        db: DatabaseConnection,
        session_ttl: Duration,
        observers: Observers,
        storage_service: Arc<dyn StorageService>,
        email_sender: Arc<dyn EmailSender>,
        public_base_url: Option<String>,
        key_login_audiences: Vec<String>,
        franking_keys: crate::franking_keys::SharedFrankingKeys,
        federation: FederationHooks,
        feature_gate: Arc<dyn FeatureGate>,
        limits: InstanceLimits,
    ) -> Self {
        // Shared so every path that ends sessions closes the connections watching them.
        let session_revocations: Arc<dyn SessionRevocations> =
            Arc::new(crate::session_revocation::InProcessSessionRevocations::default());
        // Shared so a password reset can forget the sources login exempts from its backstop.
        let known_login_sources = crate::login_sources::KnownLoginSources::login_default();
        // Shared so password and key-only signups count against one open-mode budget per IP.
        let signup_rate_limiter =
            crate::registration_rate_limiter::RegistrationRateLimiter::signup_default();
        // Shared so a signup on either path refreshes the "full" answer the other gives.
        let account_cap = crate::account_cap::AccountCap::new(limits.accounts);
        let auth_service: Arc<dyn AuthService> = Arc::new(
            crate::auth::DefaultAuthService::new(
                repos.for_auth(),
                Some(db.clone()),
                session_ttl,
                observers.clone(),
                Arc::clone(&email_sender),
                public_base_url.clone(),
            )
            .with_known_login_sources(known_login_sources.clone())
            .with_signup_rate_limiter(signup_rate_limiter.clone())
            .with_limits(limits)
            .with_reserved_names(federation.reserved_names.clone())
            .with_password_hashing(federation.password_hashing.clone())
            .with_session_revocations(Arc::clone(&session_revocations))
            .with_terms_url(federation.terms_url.clone())
            .with_minimum_age(federation.minimum_age)
            .with_signup_requires_keys(federation.signup_requires_keys)
            .with_default_workspace(federation.default_workspace.clone())
            .with_account_cap(account_cap.clone()),
        );
        let account_deletion = DefaultAccountDeletionService::new(
            Arc::new(
                rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository::new(
                    rim_infra_db::DbHandle::from(db.clone()),
                ),
            ),
            Arc::clone(&repos.session_repo),
            Some(db.clone()),
            Arc::clone(&storage_service),
            observers.clone(),
        )
        .with_session_revocations(Arc::clone(&session_revocations));
        let mut key_auth = crate::key_auth::DefaultKeyAuthService::new(
            repos.for_key_auth(),
            Some(db.clone()),
            session_ttl,
            key_login_audiences,
            observers.clone(),
            signup_rate_limiter,
        );
        if let Some(relay) = &federation.relay_service {
            key_auth = key_auth.with_relay_service(Arc::clone(relay));
        }
        if let Some(server_id) = &federation.server_id {
            key_auth = key_auth.with_server_id(server_id.clone());
        }
        key_auth = key_auth
            .with_quotas(federation.quotas)
            .with_limits(limits)
            .with_reserved_names(federation.reserved_names.clone())
            .with_session_revocations(Arc::clone(&session_revocations))
            .with_terms_url(federation.terms_url.clone())
            .with_minimum_age(federation.minimum_age)
            .with_default_workspace(federation.default_workspace.clone())
            .with_account_cap(account_cap);
        if let Some(host) = &federation.host_service {
            key_auth = key_auth.with_host_service(Arc::clone(host));
        }
        if let Some(fanout) = &federation.revocation_fanout {
            key_auth = key_auth.with_revocation_fanout(Arc::clone(fanout));
        }
        let capability_service: Arc<dyn CapabilityService> = Arc::new(
            crate::capability::DefaultCapabilityService::new(repos.for_capability()),
        );
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
        let default_workspace_service: Arc<dyn DefaultWorkspaceService> = Arc::new(
            crate::default_workspace::DefaultDefaultWorkspaceService::new(
                federation.default_workspace.clone(),
                Arc::clone(&repos.default_workspace_repo),
            )
            .with_join(repos.for_auth().signup_repos(), observers.clone())
            .with_database(db.clone()),
        );
        let default_workspace_rules = crate::default_workspace_rules::DefaultWorkspaceRules::new(
            Arc::clone(&default_workspace_service),
            Arc::clone(&repos.user_repo),
            limits,
        );
        let mut conversation_concrete = crate::conversation::DefaultConversationService::new(
            repos.for_conversation(),
            observers.clone(),
        )
        .with_e2ee_repo(Arc::clone(&repos.e2ee_repo))
        .with_direct_space(federation.direct_space_id)
        .with_limits(limits)
        .with_default_workspace(Arc::clone(&default_workspace_service));
        if let Some(host) = &federation.host_service {
            conversation_concrete = conversation_concrete.with_request_gate(Arc::clone(host));
        }
        let conversation_service: Arc<dyn ConversationService> = Arc::new(conversation_concrete);
        let notification_service: Arc<dyn NotificationService> = Arc::new(
            crate::notification::DefaultNotificationService::new(
                repos.for_notification(),
                observers.clone(),
            )
            .with_default_workspace(Arc::clone(&default_workspace_service)),
        );
        let mls_wire: Arc<dyn rim_services::e2ee::wire::MlsWire> =
            Arc::new(crate::e2ee::openmls_wire::OpenMlsWire::new());
        let mut message_concrete = crate::message::DefaultMessageService::new(
            repos.for_message(),
            Some(db.clone()),
            observers.clone(),
        )
        .with_audit_log_service(Arc::clone(&audit_log_service))
        .with_notification_service(Arc::clone(&notification_service))
        .with_e2ee(
            Arc::clone(&repos.e2ee_repo),
            Arc::clone(&mls_wire),
            franking_keys.clone(),
        )
        .with_moderation_repo(Arc::clone(&repos.moderation_repo))
        .with_direct_space(federation.direct_space_id)
        .with_limits(limits)
        .with_handle_domain(federation.handle_domain.clone());
        if let Some(rate) = federation.quotas.messages {
            message_concrete = message_concrete.with_message_budget(rate);
        }
        if let Some(rate) = federation.quotas.probation_messages {
            message_concrete = message_concrete.with_probation_budget(rate);
        }
        let message_service: Arc<dyn MessageService> = Arc::new(message_concrete);
        let mut e2ee_concrete = crate::e2ee::DefaultE2eeService::new(
            repos.for_e2ee(),
            Some(db.clone()),
            Arc::clone(&mls_wire),
            observers.clone(),
        )
        .with_audit_log_service(Arc::clone(&audit_log_service))
        .with_franking_keys(franking_keys)
        .with_key_package_cap(federation.quotas.key_packages_per_device);
        if let Some(remote) = &federation.remote_key_packages {
            e2ee_concrete = e2ee_concrete.with_remote_key_packages(Arc::clone(remote));
        }
        let e2ee_service: Arc<dyn rim_services::e2ee::service::E2eeService> =
            Arc::new(e2ee_concrete);
        let key_auth_service: Arc<dyn KeyAuthService> =
            Arc::new(key_auth.with_e2ee_service(Arc::clone(&e2ee_service)));
        let moderation_service: Arc<dyn ModerationService> = Arc::new(
            crate::moderation::DefaultModerationService::new(
                repos.for_moderation(),
                observers.clone(),
            )
            .with_audit_log_service(Arc::clone(&audit_log_service))
            .with_direct_space(federation.direct_space_id)
            .with_default_workspace(Arc::clone(&default_workspace_service))
            .with_database(db.clone()),
        );
        let message_attachment_service: Arc<dyn MessageAttachmentService> = Arc::new(
            crate::message_attachment::DefaultMessageAttachmentService::new(
                repos.for_attachment(),
                Arc::clone(&storage_service),
            ),
        );
        let mention_service: Arc<dyn MentionService> = Arc::new(
            crate::mention::DefaultMentionService::new(repos.for_mention()),
        );
        let pin_observers = observers.clone();
        let server_observers = observers.clone();
        let read_state_observers = observers.clone();
        let workspace_plugin_observers = observers.clone();
        let ws_state_broadcaster: Arc<dyn rim_services::events::Broadcaster> =
            Arc::new(observers.clone());
        let pipeline_broadcaster_clone: Arc<dyn rim_services::events::Broadcaster> =
            Arc::new(observers.clone());
        let message_reaction_service: Arc<dyn MessageReactionService> = Arc::new(
            crate::message_reaction::DefaultMessageReactionService::new(
                repos.for_reaction(),
                Some(db.clone()),
                observers.clone(),
            )
            .with_notification_service(Arc::clone(&notification_service))
            .with_feature_gate(Arc::clone(&feature_gate)),
        );
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
            .with_known_login_sources(known_login_sources)
            .with_password_hashing(federation.password_hashing.clone())
            .with_session_revocations(Arc::clone(&session_revocations)),
        );
        let pin_service: Arc<dyn PinService> = Arc::new(crate::pin::DefaultPinService::new(
            repos.for_pin(),
            pin_observers,
            Arc::clone(&message_service),
        ));
        let read_state_service: Arc<dyn ReadStateService> = Arc::new(
            crate::read_state::DefaultReadStateService::new(
                repos.for_read_state(),
                read_state_observers,
            )
            .with_db(db.clone()),
        );
        let server_service: Arc<dyn ServerService> = Arc::new(
            crate::server::DefaultServerService::new(
                repos.for_server(),
                Some(db.clone()),
                server_observers,
            )
            .with_limits(limits)
            .with_storage(Arc::clone(&storage_service))
            .with_direct_space(federation.direct_space_id),
        );
        let server_member_service: Arc<dyn ServerMemberService> = Arc::new(
            crate::server_member::DefaultServerMemberService::new(repos.for_server_member()),
        );
        let thread_service: Arc<dyn ThreadService> =
            Arc::new(crate::thread::DefaultThreadService::new(repos.for_thread()));
        let user_service: Arc<dyn UserService> = Arc::new(
            crate::user::DefaultUserService::new(repos.for_user())
                .with_limits(limits)
                .with_reserved_names(federation.reserved_names.clone())
                .with_handle_domain(federation.handle_domain.clone())
                .with_storage(Arc::clone(&storage_service))
                .with_profile_events(
                    Arc::clone(&repos.server_repo),
                    observers.clone(),
                    federation.direct_space_id,
                ),
        );
        let workspace_state_service: Arc<dyn WorkspaceStateService> = Arc::new(
            crate::workspace_state::DefaultWorkspaceStateService::new(
                repos.for_workspace_state(),
                ws_state_broadcaster,
            )
            .with_direct_space(federation.direct_space_id)
            .with_limits(limits),
        );
        // Shared confirmation store — same instance used by command_service and
        // macro_service so that a proof issued by command_service can be verified
        // by macro_service (and vice versa, once macro-level challenge issuance lands).
        let confirmation_store: std::sync::Arc<
            dyn rim_services::command::confirmation::ConfirmationStore,
        > = rim_services::command::confirmation::shared_in_memory_store();

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
            moderation_service: Arc::clone(&moderation_service),
            notification_service: Arc::clone(&notification_service),
        };
        let mut command_service_concrete =
            crate::command::DefaultCommandService::with_confirmation_store(
                repos.for_command(),
                command_services_args(),
                Arc::clone(&confirmation_store),
            );
        if let Some(direct_space_id) = federation.direct_space_id {
            command_service_concrete = command_service_concrete.with_direct_space(direct_space_id);
        }
        let command_service_concrete = Arc::new(command_service_concrete);
        let command_service: Arc<dyn CommandService> =
            Arc::clone(&command_service_concrete) as Arc<dyn CommandService>;
        let keybind_service: Arc<dyn KeybindService> = Arc::new(
            crate::keybind::DefaultKeybindService::new(
                repos.for_keybind(),
                crate::keybind::KeybindServices {
                    command_service: Arc::clone(&command_service),
                },
            )
            .with_direct_space(federation.direct_space_id),
        );
        let request_authenticator: Arc<dyn RequestAuthenticator> = Arc::new(
            crate::request_auth::DefaultRequestAuthenticator::new(repos.for_request_auth()),
        );
        let link_preview =
            crate::link_preview::DefaultLinkPreviewService::new(repos.for_link_preview())
                .with_feature_gate(Arc::clone(&feature_gate));
        let custom_emoji_service: Arc<dyn CustomEmojiService> = Arc::new(
            crate::custom_emoji::DefaultCustomEmojiService::new(
                repos.for_custom_emoji(),
                Arc::clone(&storage_service),
            )
            .with_direct_space(federation.direct_space_id),
        );
        let macro_service: Arc<dyn MacroService> = Arc::new(
            crate::macro_def::DefaultMacroService::new(
                repos.for_macro(),
                crate::macro_def::MacroServices {
                    command_service: Arc::clone(&command_service),
                    audit_log_service: Some(Arc::clone(&audit_log_service)),
                    confirmation_store: Some(Arc::clone(&confirmation_store)),
                },
            )
            .with_limits(limits)
            .with_default_workspace(default_workspace_rules.clone()),
        );

        // Break the command↔macro init cycle: the command service is constructed
        // first (macro cell empty), then the macro service is built from it, then
        // the macro service is set back into the single command-service instance.
        // keybind/pipeline hold the same Arc, so they observe macros too — no
        // comment-only invariant.
        let _ = command_service_concrete.set_macro_service(Arc::clone(&macro_service));

        let channel_lens_service: Arc<dyn ChannelLensService> = Arc::new(
            crate::channel_lens::DefaultChannelLensService::new(repos.for_channel_lens()),
        );

        let pipeline_service: Arc<dyn PipelineService> = Arc::new(
            crate::pipeline::DefaultPipelineService::new(
                repos.for_pipeline(),
                Arc::clone(&command_service),
                pipeline_broadcaster_clone,
            )
            .with_direct_space(federation.direct_space_id)
            .with_limits(limits)
            .with_default_workspace(default_workspace_rules),
        );

        let device_link_service: Arc<dyn DeviceLinkService> =
            Arc::new(crate::device_link::DefaultDeviceLinkService::new(
                repos.for_device_link(),
                Some(db.clone()),
            ));

        let workspace_plugin_service: Arc<dyn WorkspacePluginService> = Arc::new(
            crate::workspace_plugin::DefaultWorkspacePluginService::new(
                repos.for_workspace_plugin(),
                Some(db.clone()),
                workspace_plugin_observers,
            )
            .with_direct_space(federation.direct_space_id)
            .with_limits(limits),
        );
        let plugin_bundle_service: Arc<dyn PluginBundleService> = Arc::new(
            crate::plugin_bundle::DefaultPluginBundleService::new(
                repos.for_plugin_bundle(),
                Arc::clone(&storage_service),
            )
            .with_direct_space(federation.direct_space_id),
        );

        let relay_abuse_service: Arc<dyn RelayAbuseService> = Arc::new(
            crate::relay::abuse::DefaultRelayAbuseService::new(repos.for_relay_abuse())
                .with_db(db.clone())
                .with_session_revocations(Arc::clone(&session_revocations)),
        );
        let ip_block_service: Arc<dyn IpBlockService> =
            Arc::new(crate::ip_block::DefaultIpBlockService::new(
                Arc::new(
                    rim_infra_db::sea_orm_impl::ip_block::SeaOrmIpBlockRepository::new(
                        rim_infra_db::DbHandle::from(db.clone()),
                    ),
                ),
                Arc::clone(&repos.user_repo),
            ));
        let instance_operator_service: Arc<dyn InstanceOperatorService> = Arc::new(
            crate::operator::DefaultInstanceOperatorService::new(repos.for_relay_abuse()),
        );
        let dm_policy_service: Arc<dyn rim_services::dm_policy::service::DmPolicyService> =
            Arc::new(crate::dm_policy::DefaultDmPolicyService::new(
                Arc::clone(&repos.dm_policy_repo),
                Arc::clone(&repos.server_member_repo),
                federation.dm_policy.clone(),
            ));

        let public_join_link_service: Arc<
            dyn rim_services::public_join_link::service::PublicJoinLinkService,
        > = Arc::new(
            crate::public_join_link::DefaultPublicJoinLinkService::new(
                repos.for_public_join_link(),
                observers.clone(),
                federation.link_key.clone(),
                federation
                    .link_base
                    .clone()
                    .unwrap_or_else(|| "http://localhost".to_string()),
            )
            .with_database(db.clone())
            .with_audit_log_service(Arc::clone(&audit_log_service))
            .with_direct_space(federation.direct_space_id)
            .with_default_workspace(Arc::clone(&default_workspace_service))
            .with_limits(limits),
        );

        Self {
            audit_log_service,
            auth_service,
            channel_lens_service,
            command_service,
            conversation_service,
            custom_emoji_service,
            key_auth_service,
            keybind_service,
            link_preview,
            macro_service,
            mention_service,
            message_service,
            message_attachment_service,
            message_reaction_service,
            moderation_service,
            notification_service,
            notification_preference_service,
            password_reset_service,
            pin_service,
            read_state_service,
            server_service,
            server_member_service,
            thread_service,
            user_service,
            workspace_state_service,
            pipeline_service,
            request_authenticator,
            device_link_service,
            e2ee_service,
            workspace_plugin_service,
            plugin_bundle_service,
            feature_gate,
            account_deletion,
            session_revocations,
            relay_abuse_service,
            ip_block_service,
            instance_operator_service,
            dm_policy_service,
            default_workspace_service,
            public_join_link_service,
        }
    }

    /// The registry every service here reports ended sessions to; live connections watch their
    /// session in it.
    pub fn session_revocations(&self) -> Arc<dyn SessionRevocations> {
        Arc::clone(&self.session_revocations)
    }

    /// Set the time between an account's deletion request and its purge, and the relay whose
    /// hosts a purged account's subscriptions are cancelled at.
    pub fn with_account_deletion(
        mut self,
        grace: Duration,
        relay_departure: Option<
            Arc<dyn rim_services::remote_workspace::service::WorkspaceRelayService>,
        >,
    ) -> Self {
        let mut account_deletion = self.account_deletion.with_grace(grace);
        if let Some(relay) = relay_departure {
            account_deletion = account_deletion.with_relay_departure(relay);
        }
        self.account_deletion = account_deletion;
        self
    }

    /// Set how many link preview fetches the process runs at once.
    pub fn with_link_preview_concurrency(mut self, fetches: usize) -> Self {
        self.link_preview = self.link_preview.with_concurrency(fetches);
        self
    }
}

impl AppServiceProvider for DefaultAppServiceProvider {
    fn account_deletion_service(&self) -> Arc<dyn AccountDeletionService> {
        Arc::new(self.account_deletion.clone())
    }
    fn audit_log_service(&self) -> Arc<dyn AuditLogService> {
        Arc::clone(&self.audit_log_service)
    }
    fn auth_service(&self) -> Arc<dyn AuthService> {
        Arc::clone(&self.auth_service)
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
        Arc::new(self.link_preview.clone())
    }
    fn key_auth_service(&self) -> Arc<dyn KeyAuthService> {
        Arc::clone(&self.key_auth_service)
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
    fn thread_service(&self) -> Arc<dyn ThreadService> {
        Arc::clone(&self.thread_service)
    }
    fn user_service(&self) -> Arc<dyn UserService> {
        Arc::clone(&self.user_service)
    }
    fn moderation_service(&self) -> Arc<dyn ModerationService> {
        Arc::clone(&self.moderation_service)
    }
    fn notification_service(&self) -> Arc<dyn NotificationService> {
        Arc::clone(&self.notification_service)
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
    fn device_link_service(&self) -> Arc<dyn DeviceLinkService> {
        Arc::clone(&self.device_link_service)
    }
    fn e2ee_service(&self) -> Arc<dyn rim_services::e2ee::service::E2eeService> {
        Arc::clone(&self.e2ee_service)
    }
    fn workspace_plugin_service(&self) -> Arc<dyn WorkspacePluginService> {
        Arc::clone(&self.workspace_plugin_service)
    }
    fn plugin_bundle_service(&self) -> Arc<dyn PluginBundleService> {
        Arc::clone(&self.plugin_bundle_service)
    }
    fn feature_gate(&self) -> Arc<dyn FeatureGate> {
        Arc::clone(&self.feature_gate)
    }
    fn relay_abuse_service(&self) -> Arc<dyn RelayAbuseService> {
        Arc::clone(&self.relay_abuse_service)
    }
    fn ip_block_service(&self) -> Arc<dyn IpBlockService> {
        Arc::clone(&self.ip_block_service)
    }
    fn instance_operator_service(&self) -> Arc<dyn InstanceOperatorService> {
        Arc::clone(&self.instance_operator_service)
    }
    fn dm_policy_service(&self) -> Arc<dyn rim_services::dm_policy::service::DmPolicyService> {
        Arc::clone(&self.dm_policy_service)
    }

    fn default_workspace_service(&self) -> Arc<dyn DefaultWorkspaceService> {
        Arc::clone(&self.default_workspace_service)
    }

    fn public_join_link_service(
        &self,
    ) -> Arc<dyn rim_services::public_join_link::service::PublicJoinLinkService> {
        Arc::clone(&self.public_join_link_service)
    }
}
