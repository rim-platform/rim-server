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

use crate::config::{RIMConfig, ServerRole};
use crate::s2s::{S2sSettings, S2sState};

use rim_core::s2s::repository::{PeerServerRepository, ServerIdentityRepository};
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::s2s::{
    SeaOrmPeerServerRepository, SeaOrmS2sNonceRepository, SeaOrmServerIdentityRepository,
};
use rim_infra_services::s2s::server_key::{self, DescriptorSettings, KeyFiles};
use rim_infra_services::s2s::{
    DefaultPeerService, DefaultS2sClient, DefaultS2sNonceService, HttpDiscoveryFetcher,
    S2sClientConfig,
};
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_s2s::{Endpoints, Limits, Registration, RegistrationMode, SharedIdentity};
use rim_services::auth::service::RequestAuthenticator;
use rim_services::provider::AppServiceProvider;
use rim_services::s2s::service::{PeerService, S2sClient, S2sNonceService};

#[allow(dead_code)] // shutdown_tx is held to keep the watch channel open
pub struct AppRuntime {
    pub schema: AppSchema,
    pub request_authenticator: Arc<dyn RequestAuthenticator>,
    pub storage_backend: Arc<dyn StorageBackend>,
    pub storage_service: Arc<dyn StorageService>,
    pub message_attachment_service:
        Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
    pub moderation_service: Arc<dyn rim_services::moderation::service::ModerationService>,
    pub ingest_token_service: Arc<dyn rim_services::ingest_token::service::IngestTokenService>,
    pub incident_service: Arc<dyn rim_services::incident::service::IncidentService>,
    pub workspace_feature_service:
        Arc<dyn rim_services::workspace_feature::service::WorkspaceFeatureService>,
    pub plugin_bundle_service:
        Arc<dyn rim_services::workspace_plugin::service::PluginBundleService>,
    pub alert_rate_limiter: Arc<rim_infra_services::AlertRateLimiter>,
    pub alert_max_body_bytes: usize,
    pub s2s: S2sRuntime,
    pub shutdown_tx: watch::Sender<bool>,
}

/// Server identity and server-to-server traffic as wired at boot.
#[derive(Clone)]
pub struct S2sRuntime {
    pub state: S2sState,
    pub client: Arc<dyn S2sClient>,
    pub key_files: KeyFiles,
    pub identity_repo: Arc<dyn ServerIdentityRepository>,
    pub descriptor_settings: DescriptorSettings,
}

impl S2sRuntime {
    /// Pick up a key rotation or recovery made by the CLI. Returns whether the identity
    /// changed.
    pub async fn reload_identity(&self) -> anyhow::Result<bool> {
        server_key::reload_if_rotated(
            self.identity_repo.as_ref(),
            &self.key_files,
            &self.state.identity,
            &self.descriptor_settings,
        )
        .await
    }
}

/// How long an outgoing server-to-server request may take.
const S2S_REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// Largest server-to-server response read: the forward path's cap.
const S2S_MAX_RESPONSE_BYTES: usize = 32 * 1024 * 1024;

/// How often expired request nonces are deleted and a CLI key rotation is picked up.
const S2S_CLEANUP_INTERVAL_SECS: u64 = 60;

fn io_error(context: &str) -> impl FnOnce(String) -> std::io::Error + '_ {
    move |e| std::io::Error::other(format!("{context}: {e}"))
}

/// What the descriptor says besides the key, from the configuration.
fn descriptor_settings(
    config: &RIMConfig,
    role: ServerRole,
    s2s_origin: Option<&str>,
) -> DescriptorSettings {
    let origin = config.public_origin();
    let websocket = match origin.split_once("://") {
        Some(("https", rest)) => format!("wss://{rest}"),
        Some((_, rest)) => format!("ws://{rest}"),
        None => origin.clone(),
    };
    DescriptorSettings {
        roles: role.roles(),
        endpoints: Endpoints {
            graphql: format!("{origin}/api"),
            websocket: format!("{websocket}/api/ws"),
            s2s: s2s_origin.map(|o| format!("{o}/s2s/v1")),
        },
        limits: Limits {
            max_body_bytes: config.s2s_max_body_bytes() as u64,
            clock_skew_seconds: 300,
            max_deliver_items: 256,
        },
        registration: Registration {
            mode: match config.registration_mode() {
                crate::config::RegistrationMode::Open => RegistrationMode::Open,
                crate::config::RegistrationMode::InviteOnly => RegistrationMode::Invite,
            },
            pow_bits: None,
        },
        mls_cipher_suites: rim_services::e2ee::ACCEPTED_CIPHER_SUITES.to_vec(),
    }
}

/// Load or create the server key, sign the descriptor, and wire peers, nonces and the client.
async fn build_s2s(config: &RIMConfig, db: &DbHandle) -> std::io::Result<S2sRuntime> {
    let role = config.server_role().map_err(io_error("RIM_ROLE"))?;
    let denied_peers = config
        .s2s_denied_peers()
        .map_err(io_error("RIM_S2S_DENIED_PEERS"))?;
    let allowed_relays = config
        .host_allowed_relays()
        .map_err(io_error("RIM_HOST_ALLOWED_RELAYS"))?;
    let s2s_origin = config.s2s_origin();
    let key_files = config.key_files();
    let identity_repo: Arc<dyn ServerIdentityRepository> =
        Arc::new(SeaOrmServerIdentityRepository::new(db.clone()));
    let loaded = server_key::load_or_create(identity_repo.as_ref(), &key_files)
        .await
        .map_err(|e| io_error("server key")(format!("{e:#}")))?;
    let settings = descriptor_settings(config, role, s2s_origin.as_deref());
    let identity = server_key::with_descriptor(identity_repo.as_ref(), loaded, &settings)
        .await
        .map_err(|e| io_error("server descriptor")(format!("{e:#}")))?;
    tracing::info!(
        server_id = %identity.server_id(),
        key_seq = identity.state.seq,
        role = ?role,
        s2s = s2s_origin.is_some(),
        "server identity loaded"
    );
    if s2s_origin.is_none() {
        tracing::info!(
            "server-to-server traffic is off: set RIM_PUBLIC_BASE_URL to an https URL to turn              it on"
        );
    }
    let identity = SharedIdentity::new(identity);

    let allow_http = config.s2s_allow_http();
    let allow_private = config.s2s_allow_private_addresses();
    let peer_repo: Arc<dyn PeerServerRepository> =
        Arc::new(SeaOrmPeerServerRepository::new(db.clone()));
    let fetcher = HttpDiscoveryFetcher::new(allow_private)
        .map_err(|e| io_error("discovery client")(e.to_string()))?;
    let peers: Arc<dyn PeerService> = Arc::new(DefaultPeerService::new(
        Arc::clone(&peer_repo),
        Arc::new(fetcher),
        allow_http,
    ));
    let nonces: Arc<dyn S2sNonceService> = Arc::new(DefaultS2sNonceService::new(
        Arc::new(SeaOrmS2sNonceRepository::new(db.clone())),
        peer_repo,
    ));
    let client: Arc<dyn S2sClient> = Arc::new(
        DefaultS2sClient::new(
            identity.clone(),
            Arc::clone(&peers),
            S2sClientConfig {
                own_origin: s2s_origin.clone(),
                allow_http,
                allow_private,
                timeout: S2S_REQUEST_TIMEOUT,
                max_response_bytes: S2S_MAX_RESPONSE_BYTES,
            },
        )
        .map_err(|e| io_error("server-to-server client")(e.to_string()))?,
    );
    Ok(S2sRuntime {
        state: S2sState {
            identity,
            peers,
            nonces,
            rate_limiter: Arc::new(rim_infra_services::AlertRateLimiter::new(
                60,
                config.s2s_peer_rate_per_min(),
            )),
            settings: Arc::new(S2sSettings {
                role,
                origin: s2s_origin,
                denied_peers: denied_peers.into_iter().collect(),
                allowed_relays: allowed_relays.into_iter().collect(),
                max_body_bytes: config.s2s_max_body_bytes(),
            }),
        },
        client,
        key_files,
        identity_repo,
        descriptor_settings: settings,
    })
}

pub async fn build_app_runtime(config: &RIMConfig) -> std::io::Result<AppRuntime> {
    // Checked first, so a misconfigured server stops before it touches the database or disk.
    let mut key_login_audiences = config
        .key_login_audiences()
        .map_err(std::io::Error::other)?;
    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let raw_db = db.clone(); // Keep raw DatabaseConnection for UoW
    let db: DbHandle = db.into();

    // Canonical repo constructor (mirrors the FL1 composition-root note in
    // CLAUDE.md) — keeps this composition root in sync with every other
    // production UoW path without hand-duplicating the 31-repo literal.
    let repos = AppRepos::with_handle(&db);

    // Build the dummy hash that unknown-username logins verify against now, so the first such
    // login doesn't pay an extra Argon2 hash and stand out by its timing. Refuse to start
    // without it: those logins would otherwise fail differently from wrong passwords.
    rim_auth::password::verify_dummy_password("").map_err(|e| {
        std::io::Error::other(format!("failed to prepare dummy password hash: {e}"))
    })?;

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
            Arc::clone(&repos.message_attachment_repo),
            Arc::clone(&repos.workspace_plugin_repo),
            max_workspace_bytes,
        ));

    // Track B3 — real SMTP only when RIM_SMTP_HOST + RIM_SMTP_FROM are both configured;
    // otherwise every send falls back to LogEmailSender (log-only, no delivery). See
    // docs/email.md.
    let email_sender: Arc<dyn rim_infra_services::email::EmailSender> = match config.smtp_config() {
        Some(smtp) => Arc::new(
            rim_infra_services::email::SmtpEmailSender::new(smtp)
                .map_err(|e| std::io::Error::other(format!("failed to build SMTP sender: {e}")))?,
        ),
        None => {
            tracing::warn!(
                "RIM_SMTP_HOST/RIM_SMTP_FROM not configured; using LogEmailSender \
                 (emails will not be delivered)"
            );
            Arc::new(rim_infra_services::email::LogEmailSender)
        }
    };

    // C1b — single-node in-memory escalation timer wheel (§6). Constructed here (not inside
    // the provider) so this function can also drive the boot-recovery arm + the polling loop
    // below, mirroring how `presence_manager`/`typing_tracker` are built here and handed to the
    // schema as a concrete type alongside their `Arc<dyn ...>` use inside the provider.
    let escalation_scheduler =
        Arc::new(rim_infra_services::escalation::InMemoryEscalationScheduler::new());

    let s2s = build_s2s(config, &db).await?;
    key_login_audiences.push(s2s.state.identity.current().server_id().to_string());
    tracing::info!(
        audiences = ?key_login_audiences,
        "key login accepts proofs naming these audiences (RIM_KEY_LOGIN_AUDIENCES)"
    );
    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        chrono::Duration::days(i64::from(config.session_ttl_days)),
        observers,
        Arc::clone(&storage_service),
        config.ingest_token_ttl_seconds(),
        Arc::clone(&email_sender),
        config.public_base_url(),
        Arc::clone(&escalation_scheduler)
            as Arc<dyn rim_services::escalation::scheduler::EscalationScheduler>,
        key_login_audiences,
    ));

    // Boot recovery (§6) — extracted into `rim_infra_services::escalation::recover_active_escalation_runs`
    // so it's independently db-testable without spinning up the whole app runtime.
    match rim_infra_services::escalation::recover_active_escalation_runs(
        repos.escalation_run_repo.as_ref(),
        escalation_scheduler.as_ref(),
    )
    .await
    {
        Ok(armed) => {
            tracing::info!(
                target = "audit",
                event = "escalation_boot_recovery",
                armed_count = armed,
                "re-armed active escalation runs on boot (§6)"
            );
        }
        Err(e) => {
            tracing::error!(error = %e, "boot recovery: failed to list active escalation runs");
        }
    }

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

    // Background task: escalation timer driver (§6). Sleeps until the earliest armed deadline
    // (or a floor of `escalation_timer_resolution_secs` when nothing is armed), fires every due
    // run through `IncidentService::fire_escalation_step`, and races that sleep against
    // `notified()` so a newly-armed earlier deadline (or a cancellation) is picked up
    // immediately rather than after the stale sleep elapses — mirrors the pipeline evaluator's
    // `tokio::select!` shutdown pattern above.
    {
        let scheduler = Arc::clone(&escalation_scheduler);
        let incident_service_for_driver = service_provider.incident_service();
        let floor = std::time::Duration::from_secs(config.escalation_timer_resolution_secs());
        let mut shutdown_rx3 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            loop {
                let sleep_duration =
                    escalation_sleep_duration(scheduler.next_deadline(), chrono::Utc::now(), floor);
                tokio::select! {
                    () = tokio::time::sleep(sleep_duration) => {
                        let due = scheduler.take_due(chrono::Utc::now());
                        for run_id in due {
                            if let Err(e) = incident_service_for_driver.fire_escalation_step(run_id).await {
                                tracing::error!(error = %e, run_id = %run_id, "escalation step fire failed");
                            }
                        }
                    }
                    () = scheduler.notified() => {
                        // Loop again to recompute the deadline against the updated armed set.
                    }
                    _ = shutdown_rx3.changed() => {
                        tracing::info!("escalation driver: shutdown signal received");
                        break;
                    }
                }
            }
        });
    }

    // Background tasks: notification digest and retention (Track 1.3), next to the escalation
    // driver and under the same single-scheduler-node assumption. The jobs live in
    // `rim_infra_services::notification` behind `NotificationJobs`; this loop only times them.
    // The hourly retention tick also purges MLS rows the encryption retention rules let go.
    {
        let e2ee_service = service_provider.e2ee_service();
        let jobs: Arc<dyn rim_infra_services::notification::NotificationJobs> = Arc::new(
            rim_infra_services::notification::DefaultNotificationJobs::new(
                repos.for_notification_jobs(),
                Arc::clone(&email_sender),
                Arc::clone(&presence_manager) as Arc<dyn PresenceService>,
                config.public_base_url(),
            ),
        );
        let digest_every =
            std::time::Duration::from_secs(config.notification_digest_interval_secs());
        let retention_every = std::time::Duration::from_secs(NOTIFICATION_RETENTION_INTERVAL_SECS);
        let mut shutdown_rx4 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut digest_tick = tokio::time::interval(digest_every);
            let mut retention_tick = tokio::time::interval(retention_every);
            // A late tick runs once, not in a burst.
            digest_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            retention_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = digest_tick.tick() => {
                        match jobs.run_digest(chrono::Utc::now()).await {
                            Ok(report) if report.candidates > 0 => tracing::info!(
                                candidates = report.candidates,
                                sent = report.sent,
                                skipped_active = report.skipped_active,
                                failed = report.failed,
                                "notification digest run"
                            ),
                            Ok(_) => {}
                            Err(e) => tracing::error!(error = %e, "notification digest run failed"),
                        }
                    }
                    _ = retention_tick.tick() => {
                        match jobs.purge_expired(chrono::Utc::now()).await {
                            Ok(deleted) if deleted > 0 => tracing::info!(deleted, "purged expired notifications"),
                            Ok(_) => {}
                            Err(e) => tracing::error!(error = %e, "notification retention purge failed"),
                        }
                        match e2ee_service.purge().await {
                            Ok(purged) => tracing::info!(
                                commits = purged.commits,
                                welcomes = purged.welcomes,
                                key_packages = purged.key_packages,
                                readd_requests = purged.readd_requests,
                                franked_contents = purged.franked_contents,
                                "purged MLS rows past retention"
                            ),
                            Err(e) => tracing::error!(error = %e, "MLS retention purge failed"),
                        }
                    }
                    _ = shutdown_rx4.changed() => {
                        tracing::info!("notification jobs: shutdown signal received");
                        break;
                    }
                }
            }
        });
    }

    // Background task: delete expired server-to-server nonces and pick up a key rotation made
    // by the CLI, every minute.
    {
        let s2s = s2s.clone();
        let mut shutdown_rx5 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_secs(S2S_CLEANUP_INTERVAL_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        match s2s.state.nonces.purge_expired().await {
                            Ok(deleted) if deleted > 0 => tracing::debug!(deleted, "purged expired server-to-server nonces"),
                            Ok(_) => {}
                            Err(e) => tracing::error!(error = %e, "server-to-server nonce purge failed"),
                        }
                        if let Err(e) = s2s.reload_identity().await {
                            tracing::error!(error = %format!("{e:#}"), "could not pick up the rotated server key");
                        }
                    }
                    _ = shutdown_rx5.changed() => break,
                }
            }
        });
    }

    let message_attachment_service = service_provider.message_attachment_service();
    let moderation_service = service_provider.moderation_service();
    let ingest_token_service = service_provider.ingest_token_service();
    let incident_service = service_provider.incident_service();
    let workspace_feature_service = service_provider.workspace_feature_service();
    let plugin_bundle_service = service_provider.plugin_bundle_service();

    // Track B2 — alert-ingest rate limiting is keyed per ingest-token id, not IP (a token is
    // the trusted principal here; see `AlertRateLimiter`'s doc comment). Window is fixed at
    // 60s; the configured `alert_rate_limit_per_min` is the per-window request cap.
    let alert_rate_limiter = Arc::new(rim_infra_services::AlertRateLimiter::new(
        60,
        config.alert_rate_limit_per_min(),
    ));
    let alert_max_body_bytes = config.alert_max_body_bytes();

    // ── Registration mode boot-time validation (§6.3) ──────────────────────
    let registration_mode = config.registration_mode();

    // marker: see spec §4.7 R2 — limiter lives inside DefaultAuthService;
    // this token is a typed marker carried through RegistrationContext for
    // future-extensibility (e.g. swapping in a Redis-backed limiter).
    let preview_rate_limiter = Arc::new(rim_services::auth::dto::PreviewInviteRateLimiterToken);

    // marker: see spec §6.3 R2 — same rationale as preview_rate_limiter above.
    let signup_rate_limiter = Arc::new(rim_services::auth::dto::SignupRateLimiterToken);

    if registration_mode == crate::config::RegistrationMode::Open {
        tracing::warn!(
            target = "audit",
            "RIM_OPEN_REGISTRATION=true: open registration mode is active; \
             instance accepts signups without invitation"
        );
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
        moderation_service,
        ingest_token_service,
        incident_service,
        workspace_feature_service,
        plugin_bundle_service,
        alert_rate_limiter,
        alert_max_body_bytes,
        s2s,
        shutdown_tx,
    })
}

/// How often expired notifications (older than 90 days) are purged: hourly.
const NOTIFICATION_RETENTION_INTERVAL_SECS: u64 = 60 * 60;

/// Computes how long the escalation timer driver should sleep before its
/// next wakeup.
///
/// Three cases:
/// - No run armed (`next_deadline` is `None`): sleep for the `floor` (the
///   configured `escalation_timer_resolution_secs`), then loop and re-check.
/// - A run is already past due (`deadline <= now`): sleep zero, fire
///   immediately.
/// - A run is armed in the future: sleep until that deadline, but capped at
///   `floor`. This cap is a correctness requirement, not just tidiness — the
///   driver races this sleep against `scheduler.notified()` (see the
///   `tokio::select!` call site) so a *newly*-armed, earlier deadline is
///   normally picked up instantly. But if the narrow window between reading
///   `next_deadline()` and polling `notified()` misses a fresh `arm()` call
///   (e.g. a brand-new incident declared while an older, far-future run is
///   already armed), the driver would otherwise sleep until that older
///   deadline — which could be minutes away — before re-checking. Capping at
///   `floor` bounds that worst case to `floor` (~5s by default) instead.
fn escalation_sleep_duration(
    next_deadline: Option<chrono::DateTime<chrono::Utc>>,
    now: chrono::DateTime<chrono::Utc>,
    floor: std::time::Duration,
) -> std::time::Duration {
    match next_deadline {
        Some(deadline) => {
            if deadline <= now {
                std::time::Duration::ZERO
            } else {
                (deadline - now).to_std().unwrap_or(floor).min(floor)
            }
        }
        None => floor,
    }
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

#[cfg(test)]
mod escalation_sleep_duration_tests {
    use super::escalation_sleep_duration;
    use chrono::Duration as ChronoDuration;
    use std::time::Duration;

    const FLOOR: Duration = Duration::from_secs(5);

    #[test]
    fn no_run_armed_sleeps_the_floor() {
        let now = chrono::Utc::now();
        assert_eq!(escalation_sleep_duration(None, now, FLOOR), FLOOR);
    }

    #[test]
    fn past_due_deadline_sleeps_zero() {
        let now = chrono::Utc::now();
        let deadline = now - ChronoDuration::seconds(1);
        assert_eq!(
            escalation_sleep_duration(Some(deadline), now, FLOOR),
            Duration::ZERO
        );
    }

    #[test]
    fn equal_to_now_sleeps_zero() {
        let now = chrono::Utc::now();
        assert_eq!(
            escalation_sleep_duration(Some(now), now, FLOOR),
            Duration::ZERO
        );
    }

    #[test]
    fn near_deadline_sleeps_until_deadline() {
        let now = chrono::Utc::now();
        let deadline = now + ChronoDuration::seconds(2);
        let sleep = escalation_sleep_duration(Some(deadline), now, FLOOR);
        // Should be ~2s, well under the 5s floor, and NOT capped.
        assert!(sleep >= Duration::from_millis(1900) && sleep <= Duration::from_secs(2));
    }

    /// This is the regression test for iron-man's C1b-review hardening: a
    /// deadline far in the future (minutes away) must NOT produce a sleep
    /// longer than `floor`. Without the `.min(floor)` cap, a missed
    /// `notified()` window for a freshly-armed *earlier* run would leave the
    /// driver asleep until this far-future deadline instead of re-checking
    /// within `floor`.
    #[test]
    fn far_future_deadline_is_capped_at_floor() {
        let now = chrono::Utc::now();
        let deadline = now + ChronoDuration::minutes(10);
        assert_eq!(escalation_sleep_duration(Some(deadline), now, FLOOR), FLOOR);
    }
}
