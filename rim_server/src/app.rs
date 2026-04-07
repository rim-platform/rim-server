use std::path::PathBuf;
use std::sync::Arc;

use rim_api::broadcast::InMemoryBroadcaster;
use rim_api::{AppSchema, build_schema};
use rim_core::conversation_member::repository::ConversationMemberRepository;
use rim_core::message::repository::MessageRepository;
use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::storage::StorageBackend;
use rim_infra_services::file_storage::LocalFileStorage;
use rim_services::events::{Broadcaster, Observers};
use rim_services::storage::service::StorageService;
use sea_orm::Database;
use tokio::sync::watch;

use crate::config::RIMConfig;

use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl;
use rim_infra_db::sea_orm_impl::{
    invite::SeaOrmInviteRepository, keybind::SeaOrmKeybindRepository,
    server::SeaOrmServerRepository, server_member::SeaOrmServerMemberRepository,
    session::SeaOrmSessionRepository, user::SeaOrmUserRepository,
    user_command::SeaOrmUserCommandRepository,
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
    pub message_attachment_repo: Arc<dyn MessageAttachmentRepository>,
    pub message_repo: Arc<dyn MessageRepository>,
    pub conversation_repo: Arc<dyn rim_core::conversation::repository::ConversationRepository>,
    pub conversation_member_repo: Arc<dyn ConversationMemberRepository>,
    pub server_member_repo: Arc<dyn rim_core::server_member::repository::ServerMemberRepository>,
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

    let repos = AppRepos {
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
        incident_repo: Arc::new(
            sea_orm_impl::incident::SeaOrmIncidentRepository::new(db.clone()),
        ),
        incident_phase_repo: Arc::new(
            sea_orm_impl::incident_phase::SeaOrmIncidentPhaseRepository::new(db.clone()),
        ),
        incident_message_tag_repo: Arc::new(
            sea_orm_impl::incident_message_tag::SeaOrmIncidentMessageTagRepository::new(
                db.clone(),
            ),
        ),
        skill_profile_repo: Arc::new(
            sea_orm_impl::skill_profile::SeaOrmSkillProfileRepository::new(db.clone()),
        ),
    };

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

    let message_attachment_repo = Arc::clone(&repos.message_attachment_repo);
    let message_repo = Arc::clone(&repos.message_repo);
    let conversation_repo = Arc::clone(&repos.conversation_repo);
    let conversation_member_repo = Arc::clone(&repos.conversation_member_repo);
    let server_member_repo = Arc::clone(&server_member_repo);

    let schema = build_schema(
        Arc::clone(&service_provider),
        broadcaster,
        Arc::clone(&presence_manager),
        typing_tracker,
        observers_for_schema,
    );
    let request_authenticator = service_provider.request_authenticator();

    Ok(AppRuntime {
        schema,
        request_authenticator,
        storage_backend,
        storage_service,
        message_attachment_repo,
        message_repo,
        conversation_repo,
        conversation_member_repo,
        server_member_repo,
        shutdown_tx,
    })
}
