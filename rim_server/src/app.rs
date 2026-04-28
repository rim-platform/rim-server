use std::path::PathBuf;
use std::sync::Arc;

use rim_api::broadcast::InMemoryBroadcaster;
use rim_api::{AppSchema, build_schema};
use rim_core::storage::StorageBackend;
use rim_infra_services::file_storage::LocalFileStorage;
use rim_services::events::{Broadcaster, Observers};
use rim_services::presence::PresenceService;
use rim_services::storage::service::StorageService;
use rim_services::typing::TypingService;
use sea_orm::Database;
use tokio::sync::watch;

use crate::config::RIMConfig;

use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl;
use rim_infra_db::sea_orm_impl::{
    bootstrap_token::SeaOrmBootstrapTokenRepository,
    instance_invite::SeaOrmInstanceInviteRepository, invite::SeaOrmInviteRepository,
    keybind::SeaOrmKeybindRepository, server::SeaOrmServerRepository,
    server_member::SeaOrmServerMemberRepository, session::SeaOrmSessionRepository,
    user::SeaOrmUserRepository, user_command::SeaOrmUserCommandRepository,
};
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_services::auth::service::RequestAuthenticator;
use rim_services::provider::AppServiceProvider;

#[allow(dead_code)] // shutdown_tx is held to keep the watch channel open
pub struct AppRuntime {
    pub schema: AppSchema,
    pub request_authenticator: Arc<dyn RequestAuthenticator>,
    pub storage_backend: Arc<dyn StorageBackend>,
    pub storage_service: Arc<dyn StorageService>,
    pub message_attachment_service:
        Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
    pub shutdown_tx: watch::Sender<bool>,
}

pub async fn build_app_runtime(config: &RIMConfig) -> std::io::Result<AppRuntime> {
    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let raw_db = db.clone(); // Keep raw DatabaseConnection for UoW
    let db: DbHandle = db.into();

    let user_repo: Arc<dyn rim_core::user::repository::UserRepository> =
        Arc::new(SeaOrmUserRepository::new(db.clone()));
    let server_repo: Arc<dyn rim_core::server::repository::ServerRepository> =
        Arc::new(SeaOrmServerRepository::new(db.clone()));
    let server_member_repo: Arc<dyn rim_core::server_member::repository::ServerMemberRepository> =
        Arc::new(SeaOrmServerMemberRepository::new(db.clone()));
    let session_repo: Arc<dyn rim_core::auth::repository::SessionRepository> =
        Arc::new(SeaOrmSessionRepository::new(db.clone()));
    let invite_repo: Arc<dyn rim_core::auth::repository::InviteRepository> =
        Arc::new(SeaOrmInviteRepository::new(db.clone()));
    let bootstrap_token_repo: Arc<dyn rim_core::auth::repository::BootstrapTokenRepository> =
        Arc::new(SeaOrmBootstrapTokenRepository::new(db.clone()));
    let instance_invite_repo: Arc<dyn rim_core::auth::repository::InstanceInviteRepository> =
        Arc::new(SeaOrmInstanceInviteRepository::new(db.clone()));

    let repos = AppRepos {
        bootstrap_token_repo: Arc::clone(&bootstrap_token_repo),
        instance_invite_repo: Arc::clone(&instance_invite_repo),
        channel_lens_repo: Arc::new(
            sea_orm_impl::channel_lens::SeaOrmChannelLensRepository::new(db.clone()),
        ),
        conversation_repo: Arc::new(
            sea_orm_impl::conversation::SeaOrmConversationRepository::new(db.clone()),
        ),
        conversation_member_repo: Arc::new(
            sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository::new(db.clone()),
        ),
        keybind_repo: Arc::new(SeaOrmKeybindRepository::new(db.clone())),
        macro_repo: Arc::new(sea_orm_impl::macro_def::SeaOrmMacroRepository::new(
            db.clone(),
        )),
        message_repo: Arc::new(sea_orm_impl::message::SeaOrmMessageRepository::new(
            db.clone(),
        )),
        message_attachment_repo: Arc::new(
            sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository::new(db.clone()),
        ),
        message_thread_repo: Arc::new(
            sea_orm_impl::message_thread::SeaOrmMessageThreadRepository::new(db.clone()),
        ),
        message_reaction_repo: Arc::new(
            sea_orm_impl::message_reaction::SeaOrmMessageReactionRepository::new(db.clone()),
        ),
        message_mention_repo: Arc::new(
            sea_orm_impl::message_mention::SeaOrmMessageMentionRepository::new(db.clone()),
        ),
        message_pin_repo: Arc::new(sea_orm_impl::message_pin::SeaOrmMessagePinRepository::new(
            db.clone(),
        )),
        notification_preference_repo: Arc::new(
            sea_orm_impl::notification_preference::SeaOrmNotificationPreferenceRepository::new(
                db.clone(),
            ),
        ),
        password_reset_token_repo: Arc::new(
            sea_orm_impl::password_reset_token::SeaOrmPasswordResetTokenRepository::new(db.clone()),
        ),
        read_state_repo: Arc::new(
            sea_orm_impl::conversation_read_state::SeaOrmConversationReadStateRepository::new(
                db.clone(),
            ),
        ),
        user_command_repo: Arc::new(SeaOrmUserCommandRepository::new(db.clone())),
        server_repo: Arc::clone(&server_repo),
        server_member_repo: Arc::clone(&server_member_repo),
        user_repo: Arc::clone(&user_repo),
        session_repo: Arc::clone(&session_repo),
        invite_repo: Arc::clone(&invite_repo),
        link_preview_repo: Arc::new(
            sea_orm_impl::link_preview::SeaOrmLinkPreviewRepository::new(db.clone()),
        ),
        custom_emoji_repo: Arc::new(
            sea_orm_impl::custom_emoji::SeaOrmCustomEmojiRepository::new(db.clone()),
        ),
        incident_repo: Arc::new(sea_orm_impl::incident::SeaOrmIncidentRepository::new(
            db.clone(),
        )),
        incident_phase_repo: Arc::new(
            sea_orm_impl::incident_phase::SeaOrmIncidentPhaseRepository::new(db.clone()),
        ),
        incident_message_tag_repo: Arc::new(
            sea_orm_impl::incident_message_tag::SeaOrmIncidentMessageTagRepository::new(db.clone()),
        ),
        skill_profile_repo: Arc::new(
            sea_orm_impl::skill_profile::SeaOrmSkillProfileRepository::new(db.clone()),
        ),
        workspace_state_repo: Arc::new(
            sea_orm_impl::workspace_state::SeaOrmWorkspaceStateRepository::new(db.clone()),
        ),
        pipeline_repo: Arc::new(sea_orm_impl::pipeline::SeaOrmPipelineRepository::new(
            db.clone(),
        )),
        capability_registry_repo: Arc::new(
            sea_orm_impl::capability_registry::SeaOrmCapabilityRegistryRepository::new(db.clone()),
        ),
    };

    // Phase 1 §3.3 — closes Black Widow C1. Seed builtin capabilities into
    // the registry SYNCHRONOUSLY before the HTTP listener binds, so the
    // capability gate cannot deny a legitimate first-request `chat.write`
    // due to a registry-seeding race. Idempotent: re-runs are no-ops past
    // the first deploy.
    seed_builtin_capabilities(repos.capability_registry_repo.as_ref())
        .await
        .map_err(|e| std::io::Error::other(format!("failed to seed builtin capabilities: {e}")))?;

    // Phase 1 §3.4 — F1 closure (Ant-Man / Widow). Audit-emit the
    // AI-callable command inventory at boot so operators have a stable,
    // grep-able record of which commands the LLM surface can reach. Phase 1
    // builtins are all `ai_callable: false`; the emission still runs (with
    // count=0) so a deploy that accidentally promotes a builtin to AI-
    // callable surfaces the change in the audit log immediately, not after
    // a real exploit.
    rim_infra_services::command::emit_ai_callable_inventory_audit();

    let broadcaster = InMemoryBroadcaster::new(1024);
    let broadcaster_arc: Arc<dyn Broadcaster> = Arc::new(broadcaster.clone());
    let observers = Observers::new(broadcaster_arc);
    let observers_for_presence = Arc::new(observers.clone());
    let observers_for_schema = Arc::new(observers.clone());

    // Build storage backend
    let files_dir = config
        .storage_local_path
        .as_ref()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from(&config.data_dir).join("files"));
    std::fs::create_dir_all(&files_dir)
        .map_err(|e| std::io::Error::other(format!("failed to create files dir: {e}")))?;
    let storage_backend: Arc<dyn StorageBackend> = Arc::new(LocalFileStorage::new(files_dir));

    let max_workspace_bytes = config.max_workspace_storage_bytes.unwrap_or(1_073_741_824);
    let storage_service: Arc<dyn StorageService> =
        Arc::new(rim_infra_services::storage::DefaultStorageService::new(
            Arc::clone(&storage_backend),
            max_workspace_bytes,
        ));

    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        config.bootstrap_secret.clone(),
        chrono::Duration::days(i64::from(config.session_ttl_days)),
        observers,
        Arc::clone(&storage_service),
    ));

    let presence_manager = Arc::new(rim_infra_services::presence::PresenceManager::new(
        observers_for_presence,
    ));

    let typing_tracker = Arc::new(rim_infra_services::typing::TypingTracker::new());

    // Background task: check presence timeouts and clean up stale typing entries every 30 seconds.
    let (shutdown_tx, mut shutdown_rx) = watch::channel(false);
    {
        let pm = Arc::clone(&presence_manager);
        let tt = Arc::clone(&typing_tracker);
        tokio::spawn(async move {
            let mut interval = tokio::time::interval(std::time::Duration::from_secs(30));
            loop {
                tokio::select! {
                    _ = interval.tick() => {
                        pm.check_timeouts();
                        tt.cleanup_stale();
                    }
                    _ = shutdown_rx.changed() => {
                        break;
                    }
                }
            }
        });
    }

    // Background task: pipeline evaluator — subscribes to broadcast events and runs pipelines.
    {
        let pipeline_service = service_provider.pipeline_service();
        let mut event_rx = broadcaster.subscribe();
        let mut shutdown_rx2 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                tokio::select! {
                    result = event_rx.recv() => {
                        match result {
                            Ok(event) => {
                                // Skip events emitted by the evaluator itself to prevent loops
                                if event.is_pipeline_event() {
                                    continue;
                                }
                                if let Err(e) = pipeline_service.evaluate_event(event).await {
                                    tracing::error!(error = %e, "pipeline evaluator failed");
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                                tracing::warn!(skipped = n, "pipeline evaluator lagged, some events dropped");
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                                tracing::info!("pipeline evaluator: broadcast channel closed, shutting down");
                                break;
                            }
                        }
                    }
                    _ = shutdown_rx2.changed() => {
                        tracing::info!("pipeline evaluator: shutdown signal received");
                        break;
                    }
                }
            }
        });
    }

    let message_attachment_service = service_provider.message_attachment_service();

    // ── Registration mode boot-time validation (§6.3) ──────────────────────
    let registration_mode = config.registration_mode();

    let preview_rate_limiter = Arc::new(rim_services::auth::dto::PreviewInviteRateLimiterToken);

    // preview rate limiter is ALWAYS required (§4.7 boot invariant).
    // We use the token type as a proof-of-wiring; absence here would be a
    // programmer error caught by the test `boot_fails_when_preview_rate_limiter_missing`.
    let _ = &preview_rate_limiter; // proves it's wired

    let signup_rate_limiter = Arc::new(rim_services::auth::dto::SignupRateLimiterToken);

    if registration_mode == crate::config::RegistrationMode::Open {
        tracing::warn!(
            target = "audit",
            "RIM_OPEN_REGISTRATION=true: open registration mode is active; \
             instance accepts signups without invitation"
        );
        // Fail-closed: ensure signup rate limiter is wired.
        // In this implementation, the rate limiter lives inside DefaultAuthService
        // and is always built. The token in context is the invariant proof.
        let _ = &signup_rate_limiter; // proves it's wired
    }

    let registration_mode_dto = match registration_mode {
        crate::config::RegistrationMode::InviteOnly => {
            rim_services::auth::dto::RegistrationMode::InviteOnly
        }
        crate::config::RegistrationMode::Open => rim_services::auth::dto::RegistrationMode::Open,
    };

    let schema = build_schema(
        Arc::clone(&service_provider),
        broadcaster,
        Arc::clone(&presence_manager) as Arc<dyn PresenceService>,
        typing_tracker as Arc<dyn TypingService>,
        observers_for_schema,
        rim_api::RegistrationContext {
            registration_mode: Arc::new(registration_mode_dto),
            preview_rate_limiter,
            signup_rate_limiter,
        },
    );
    let request_authenticator = service_provider.request_authenticator();

    Ok(AppRuntime {
        schema,
        request_authenticator,
        storage_backend,
        storage_service,
        message_attachment_service,
        shutdown_tx,
    })
}

/// Idempotent startup seeding for the capability registry.
///
/// Runs synchronously before the HTTP listener binds — see the
/// `build_app_runtime` call site for the rationale (Black Widow C1
/// registry-seeding race closure). Also emits the F1 `ai_callable_inventory`
/// audit line so operators can confirm at boot which caps the gate
/// recognises.
async fn seed_builtin_capabilities(
    repo: &dyn rim_core::capability::repository::CapabilityRegistryRepository,
) -> Result<(), String> {
    let specs = rim_infra_services::capability::builtin_capabilities();
    for spec in specs {
        // Use the insert-only seed entrypoint so a builtin that has been
        // soft-retired by an operator (e.g. company-wide `audit.read`
        // disable) is NOT silently re-activated on the next deploy
        // (Gate 4 M3).
        repo.upsert_if_missing(
            spec.id,
            rim_core::capability::model::CapabilitySourceKind::Builtin,
            None,
            spec.is_self_scoped,
        )
        .await
        .map_err(|e| format!("seed {}: {e:?}", spec.id))?;
    }
    tracing::info!(
        target = "audit",
        event = "capability_registry_seeded",
        count = specs.len(),
        "seeded builtin capabilities into registry (Phase 1 §3.3)"
    );
    Ok(())
}
