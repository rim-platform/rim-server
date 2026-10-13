use std::path::PathBuf;
use std::sync::Arc;

use rim_api::broadcast::InMemoryBroadcaster;
use rim_api::{AppSchema, build_schema};
use rim_core::server_feature::model::ServerFeature;
use rim_core::storage::StorageBackend;
use rim_infra_services::file_storage::LocalFileStorage;
use rim_services::events::{Broadcaster, Observers};
use rim_services::feature::service::FeatureGate;
use rim_services::presence::PresenceService;
use rim_services::storage::service::StorageService;
use rim_services::typing::TypingService;
use sea_orm::DatabaseConnection;
use tokio::sync::watch;

use crate::config::{RIMConfig, ServerRole};
use crate::s2s::{S2sSettings, S2sState};

use rim_api::federation::FederationContext;
use rim_core::s2s::repository::{PeerServerRepository, ServerIdentityRepository};
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::s2s::{
    SeaOrmPeerServerRepository, SeaOrmS2sNonceRepository, SeaOrmServerIdentityRepository,
};
use rim_infra_services::host::outbox::{HostEventWriter, OutboxSender, OutboxSink, RefusalCache};
use rim_infra_services::host::{DIRECT_SPACE_NAME, DefaultHostService};
use rim_infra_services::relay::{DefaultRelayService, RelaySettings};
use rim_infra_services::remote_workspace::{
    DefaultWorkspaceHost, DefaultWorkspaceRelay, RepoDelegationCheck,
};
use rim_infra_services::s2s::server_key::{self, DescriptorSettings, KeyFiles};
use rim_infra_services::s2s::{
    DefaultPeerService, DefaultS2sClient, DefaultS2sNonceService, HttpDiscoveryFetcher,
    S2sClientConfig,
};
use rim_infra_services::{AppRepos, DefaultAppServiceProvider, FederationHooks};
use rim_s2s::{Endpoints, Limits, Registration, RegistrationMode, SharedIdentity};
use rim_services::auth::service::RequestAuthenticator;
use rim_services::host::service::{HostService, RemoteKeyPackages};
use rim_services::provider::AppServiceProvider;
use rim_services::relay::dto::RelayInfoDto;
use rim_services::relay::service::RelayService;
use rim_services::remote_workspace::service::{
    RevocationFanout, WorkspaceHostService, WorkspaceRelayService,
};
use rim_services::s2s::service::{PeerService, S2sClient, S2sNonceService};
use rim_services::session_revocation::SessionRevocations;

pub struct AppRuntime {
    pub schema: AppSchema,
    pub request_authenticator: Arc<dyn RequestAuthenticator>,
    pub storage_backend: Arc<dyn StorageBackend>,
    pub storage_service: Arc<dyn StorageService>,
    pub message_attachment_service:
        Arc<dyn rim_services::message_attachment::service::MessageAttachmentService>,
    pub moderation_service: Arc<dyn rim_services::moderation::service::ModerationService>,
    pub plugin_bundle_service:
        Arc<dyn rim_services::workspace_plugin::service::PluginBundleService>,
    /// The instance's feature switches, for the upload routes and the discovery document.
    pub feature_gate: Arc<dyn FeatureGate>,
    pub s2s: S2sRuntime,
    /// The database connection pool the services share, closed once the server has stopped.
    pub db: DatabaseConnection,
    /// The background tasks' stop signal: they stop once it is dropped.
    pub shutdown_tx: watch::Sender<bool>,
    /// The IP deny list the edge refuses, kept current by a background task.
    pub(crate) ip_deny_list: Arc<crate::edge::IpDenyList>,
    /// Where ended sessions are reported; each WebSocket connection watches its session here.
    pub session_revocations: Arc<dyn SessionRevocations>,
}

/// Server identity and server-to-server traffic as wired at boot.
#[derive(Clone)]
pub struct S2sRuntime {
    pub state: S2sState,
    pub client: Arc<dyn S2sClient>,
    /// This server as its users' relay.
    pub relay: Arc<DefaultRelayService>,
    /// This server as the host of spaces with remote members.
    pub host: Arc<DefaultHostService>,
    /// This server as the host of workspaces with members whose relay is elsewhere.
    pub workspace_host: Arc<DefaultWorkspaceHost>,
    /// This server as the relay of members of workspaces hosted elsewhere.
    pub workspace_relay: Arc<DefaultWorkspaceRelay>,
    /// The relay's direct space.
    pub direct_space_id: rim_core::server::model::ServerId,
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

/// How often revocations due for another attempt are sent to hosts.
const REVOCATION_FANOUT_INTERVAL_SECS: u64 = 5;

/// How often the IP deny list is read again and expired blocks are deleted.
const IP_DENY_LIST_REFRESH_SECS: u64 = 30;

/// How often expired request nonces are deleted and a CLI key rotation (server key or franking
/// keys) is picked up.
const S2S_CLEANUP_INTERVAL_SECS: u64 = 60;

fn io_error(context: &str) -> impl FnOnce(String) -> std::io::Error + '_ {
    move |e| std::io::Error::other(format!("{context}: {e}"))
}

/// What the descriptor says besides the key, from the configuration.
fn descriptor_settings(
    config: &RIMConfig,
    role: ServerRole,
    s2s_origin: Option<&str>,
    registration: &rim_services::auth::dto::RegistrationMode,
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
        registration: match *registration {
            rim_services::auth::dto::RegistrationMode::Open => Registration {
                mode: RegistrationMode::Open,
                pow_bits: None,
            },
            rim_services::auth::dto::RegistrationMode::InviteOnly => Registration {
                mode: RegistrationMode::Invite,
                pow_bits: None,
            },
            rim_services::auth::dto::RegistrationMode::Pow { bits } => Registration {
                mode: RegistrationMode::Pow,
                pow_bits: Some(bits),
            },
        },
        mls_cipher_suites: rim_services::e2ee::ACCEPTED_CIPHER_SUITES.to_vec(),
    }
}

/// Load or create the server key, sign the descriptor, and wire peers, nonces and the client.
pub(crate) async fn build_s2s(
    config: &RIMConfig,
    db: &DbHandle,
    raw_db: &sea_orm::DatabaseConnection,
    registration: &rim_services::auth::dto::RegistrationMode,
) -> std::io::Result<S2sRuntime> {
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
    let settings = descriptor_settings(config, role, s2s_origin.as_deref(), registration);
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
    let repos = AppRepos::with_handle(db);
    let direct_space_id = repos
        .host_repo
        .ensure_direct_space(DIRECT_SPACE_NAME)
        .await
        .map_err(|e| io_error("direct space")(e.to_string()))?;
    let relay = Arc::new(DefaultRelayService::new(
        repos.for_relay(),
        Some(raw_db.clone()),
        identity.clone(),
        Arc::clone(&peers),
        Arc::clone(&client),
        RelaySettings {
            queue_bytes_per_user: config.relay_queue_bytes_per_user(),
            retention: chrono::Duration::days(i64::from(config.relay_retention_days())),
            quotas: config.quotas(),
        },
    ));
    let host = Arc::new(
        DefaultHostService::new(
            repos.for_host(),
            Some(raw_db.clone()),
            identity.clone(),
            Arc::clone(&peers),
            Arc::clone(&client),
            direct_space_id,
            config.s2s_device_rate_per_min(),
        )
        .with_quotas(config.quotas())
        .with_limits(config.instance_limits().map_err(std::io::Error::other)?)
        .with_delegations(Arc::new(RepoDelegationCheck::new(Arc::clone(
            &repos.delegation_repo,
        )))),
    );
    let workspace_host = Arc::new(DefaultWorkspaceHost::new(
        repos.for_workspace_host(),
        identity.clone(),
        Arc::clone(&peers),
        Arc::clone(&host),
        i64::from(config.s2s_subscriptions_per_peer()),
    ));
    let workspace_relay = Arc::new(DefaultWorkspaceRelay::new(
        repos.for_workspace_relay(),
        identity.clone(),
        Arc::clone(&peers),
        Arc::clone(&client),
        Arc::clone(&relay) as Arc<dyn RelayService>,
    ));
    Ok(S2sRuntime {
        relay: Arc::clone(&relay),
        host: Arc::clone(&host),
        workspace_host: Arc::clone(&workspace_host),
        workspace_relay: Arc::clone(&workspace_relay),
        direct_space_id,
        state: S2sState {
            client: Arc::clone(&client),
            relay,
            host,
            workspace_host,
            workspace_relay,
            identity,
            peers,
            nonces,
            rate_limiter: Arc::new(rim_infra_services::KeyedRateLimiter::new(
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
    let audit_retention_days = config
        .audit_retention_days()
        .map_err(std::io::Error::other)?;
    let instance_policy = config.instance_policy().map_err(std::io::Error::other)?;
    let instance_limits = config.instance_limits().map_err(std::io::Error::other)?;
    let account_deletion_grace_days = config
        .account_deletion_grace_days()
        .map_err(std::io::Error::other)?;
    let reserved_names = config.reserved_names().map_err(std::io::Error::other)?;
    let link_preview_concurrency = config
        .link_preview_concurrency()
        .map_err(std::io::Error::other)?;
    let password_hashing = rim_infra_services::password_hashing::PasswordHashing::new(
        config
            .password_hash_concurrency()
            .map_err(std::io::Error::other)?,
    );
    let instance_features = config.feature_gate().map_err(std::io::Error::other)?;
    log_features(&instance_features);
    let feature_gate: Arc<dyn FeatureGate> = Arc::new(instance_features);
    let email_disabled = feature_gate.state(ServerFeature::Email).is_disabled();
    let registration_mode = config.registration_mode().map_err(std::io::Error::other)?;
    let db = config
        .connect_database()
        .await
        .map_err(|e| std::io::Error::other(e.to_string()))?;
    let raw_db = db.clone(); // Keep raw DatabaseConnection for UoW
    let raw_db_for_tasks = raw_db.clone();
    let db: DbHandle = db.into();

    // The shared repo constructor keeps this composition root in sync with every other
    // production UoW path without hand-duplicating the repo literal.
    let repos = AppRepos::with_handle(&db);

    // Build the dummy hash that unknown-username logins verify against now, so the first such
    // login doesn't pay an extra Argon2 hash and stand out by its timing. Refuse to start
    // without it: those logins would otherwise fail differently from wrong passwords.
    rim_auth::password::verify_dummy_password("").map_err(|e| {
        std::io::Error::other(format!("failed to prepare dummy password hash: {e}"))
    })?;

    // Seed the builtin capabilities into the registry before the HTTP listener binds, so the
    // capability gate cannot deny a legitimate first request because the registry is still
    // empty. Idempotent: re-runs are no-ops past the first deploy.
    seed_builtin_capabilities(repos.capability_registry_repo.as_ref())
        .await
        .map_err(|e| std::io::Error::other(format!("failed to seed builtin capabilities: {e}")))?;

    // Audit-emit the AI-callable builtin command inventory at boot, so operators have a
    // grep-able record of which commands the AI surface can reach and a deploy that changes
    // the set shows up in the audit log.
    rim_infra_services::command::emit_ai_callable_inventory_audit();

    let s2s = build_s2s(config, &db, &raw_db, &registration_mode).await?;

    // The deny list is in force before the listener binds.
    let ip_block_service: Arc<dyn rim_services::ip_block::service::IpBlockService> = Arc::new(
        rim_infra_services::ip_block::DefaultIpBlockService::new(Arc::new(
            rim_infra_db::sea_orm_impl::ip_block::SeaOrmIpBlockRepository::new(db.clone()),
        )),
    );
    let ip_deny_list = Arc::new(crate::edge::IpDenyList::default());
    crate::edge::refresh_deny_list(ip_block_service.as_ref(), &ip_deny_list)
        .await
        .map_err(|e| io_error("IP deny list")(e.to_string()))?;

    let broadcaster = InMemoryBroadcaster::new(1024);
    let broadcaster_arc: Arc<dyn Broadcaster> = Arc::new(broadcaster.clone());
    // While server-to-server traffic is on, every event also reaches the outbox writer, which
    // never drops one.
    let mut observers = Observers::new(broadcaster_arc);
    let mut outbox_events = None;
    if s2s.state.settings.origin.is_some() {
        let (sink, events) = OutboxSink::channel();
        observers = observers.with_sink(Arc::new(sink));
        outbox_events = Some(events);
    }
    let observers_for_presence = Arc::new(observers.clone());
    let observers_for_schema = Arc::new(observers.clone());

    let (storage_backend, storage_service) = build_storage(config, &repos)?;

    // Real SMTP only when RIM_SMTP_HOST + RIM_SMTP_FROM are both configured; otherwise every
    // send falls back to LogEmailSender (log-only, no delivery). With email disabled every
    // message is dropped.
    let email_sender: Arc<dyn rim_infra_services::email::EmailSender> = match config.smtp_config() {
        _ if email_disabled => Arc::new(rim_infra_services::email::DisabledEmailSender),
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

    let franking_key_file = config.franking_key_file();
    let franking_keys = rim_infra_services::franking_keys::load_or_create(
        &franking_key_file,
        repos.e2ee_repo.as_ref(),
    )
    .await
    .map_err(|e| io_error("franking key")(format!("{e:#}")))?;
    tracing::info!(
        path = %franking_key_file.display(),
        current = franking_keys.current().id,
        "franking keys loaded"
    );
    let franking_keys = rim_infra_services::franking_keys::SharedFrankingKeys::from_file(
        franking_keys,
        franking_key_file,
    );
    key_login_audiences.push(s2s.state.identity.current().server_id().to_string());
    tracing::info!(
        audiences = ?key_login_audiences,
        "key login accepts proofs naming these audiences (RIM_KEY_LOGIN_AUDIENCES)"
    );
    let service_provider = DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        chrono::Duration::days(i64::from(config.session_ttl_days)),
        observers,
        Arc::clone(&storage_service),
        Arc::clone(&email_sender),
        config.public_base_url(),
        key_login_audiences,
        franking_keys.clone(),
        FederationHooks {
            direct_space_id: Some(s2s.direct_space_id),
            remote_key_packages: Some(Arc::clone(&s2s.host) as Arc<dyn RemoteKeyPackages>),
            relay_service: Some(Arc::clone(&s2s.relay) as Arc<dyn RelayService>),
            server_id: Some(s2s.state.identity.current().server_id().to_string()),
            quotas: config.quotas(),
            host_service: Some(Arc::clone(&s2s.host) as Arc<dyn HostService>),
            revocation_fanout: Some(Arc::clone(&s2s.workspace_relay) as Arc<dyn RevocationFanout>),
            reserved_names,
            password_hashing,
            terms_url: instance_policy.terms_url.clone(),
        },
        Arc::clone(&feature_gate),
        instance_limits,
    )
    .with_account_deletion(
        chrono::Duration::days(i64::from(account_deletion_grace_days)),
        Some(Arc::clone(&s2s.workspace_relay) as Arc<dyn WorkspaceRelayService>),
    )
    .with_link_preview_concurrency(link_preview_concurrency);
    let session_revocations = service_provider.session_revocations();
    s2s.workspace_host
        .set_session_revocations(Arc::clone(&session_revocations));
    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(service_provider);
    s2s.host.set_e2ee_service(service_provider.e2ee_service());

    let presence_manager = Arc::new(rim_infra_services::presence::PresenceManager::new(
        observers_for_presence,
    ));
    s2s.workspace_host.set_services(
        service_provider.auth_service(),
        Arc::clone(&presence_manager) as Arc<dyn PresenceService>,
        service_provider.e2ee_service(),
        service_provider.audit_log_service(),
    );

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

    // Background tasks: notification digest and retention, assuming a single
    // scheduler node. The jobs live in
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
                    _ = digest_tick.tick(), if !email_disabled => {
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
                                readd_nonces = purged.readd_nonces,
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

    // Background tasks: the outbox writer turns events into signed envelopes for relays, and
    // the sender delivers each relay's outbox in order.
    if let Some(events) = outbox_events {
        let refusals = Arc::new(RefusalCache::default());
        let wake = Arc::new(tokio::sync::Notify::new());
        let writer = HostEventWriter::new(
            repos.for_host(),
            Some(raw_db_for_tasks.clone()),
            s2s.state.identity.clone(),
            s2s.direct_space_id,
            Arc::clone(&refusals),
            Arc::clone(&wake),
        )
        .with_relay(Arc::clone(&s2s.relay) as Arc<dyn RelayService>);
        let sender = OutboxSender::new(repos.for_host(), Arc::clone(&s2s.client), refusals, wake);
        tokio::spawn(writer.run(events, shutdown_tx.subscribe()));
        tokio::spawn(sender.run(shutdown_tx.subscribe()));
    }

    // Background task: send device revocations due for another attempt to the hosts holding
    // their person.
    if s2s.state.settings.origin.is_some() {
        let workspace_relay = Arc::clone(&s2s.workspace_relay);
        let mut shutdown_rx6 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(
                REVOCATION_FANOUT_INTERVAL_SECS,
            ));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(e) = workspace_relay.send_due_revocations().await {
                            tracing::error!(error = %e, "revocation fan-out pass failed");
                        }
                    }
                    _ = shutdown_rx6.changed() => break,
                }
            }
        });
    }

    // Background task: delete expired server-to-server nonces, queued items, requests, outbox
    // rows and seen request seqs, pull what hosts queued, and pick up a server key or franking
    // key rotation made by the CLI, every minute.
    {
        let s2s = s2s.clone();
        let relay_repo = Arc::clone(&repos.relay_repo);
        let franking_keys = franking_keys.clone();
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
                        match franking_keys.reload() {
                            Ok(true) => tracing::info!(
                                current = franking_keys.keys().current().id,
                                "picked up the rotated franking keys"
                            ),
                            Ok(false) => {}
                            Err(e) => tracing::error!(error = %format!("{e:#}"), "could not read the franking key file"),
                        }
                        run_relay_cleanup(&s2s, relay_repo.as_ref()).await;
                    }
                    _ = shutdown_rx5.changed() => break,
                }
            }
        });
    }

    // Background task: read the IP deny list again and delete expired blocks, so a block or
    // unblock made by the CLI applies within the interval.
    {
        let service = Arc::clone(&ip_block_service);
        let list = Arc::clone(&ip_deny_list);
        let mut shutdown_rx7 = shutdown_tx.subscribe();
        tokio::spawn(async move {
            let mut tick =
                tokio::time::interval(std::time::Duration::from_secs(IP_DENY_LIST_REFRESH_SECS));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // The first tick fires at once; the list was loaded at boot.
            tick.tick().await;
            loop {
                tokio::select! {
                    _ = tick.tick() => {
                        if let Err(e) = crate::edge::refresh_deny_list(service.as_ref(), &list).await {
                            tracing::error!(error = %e, "could not refresh the IP deny list");
                        }
                    }
                    _ = shutdown_rx7.changed() => break,
                }
            }
        });
    }

    // Background task: export and delete command audit rows older than
    // `RIM_AUDIT_RETENTION_DAYS`, a minute after boot and then daily.
    if let Some(days) = audit_retention_days {
        let repo: Arc<dyn rim_core::audit::repository::AuditLogRepository> = Arc::new(
            rim_infra_db::sea_orm_impl::audit::SeaOrmAuditLogRepository::new(
                raw_db_for_tasks.clone().into(),
            ),
        );
        let export_dir =
            rim_infra_services::audit_retention::export_dir(std::path::Path::new(&config.data_dir));
        tokio::spawn(rim_infra_services::audit_retention::run_daily(
            repo,
            days,
            export_dir,
            shutdown_tx.subscribe(),
        ));
    }

    let message_attachment_service = service_provider.message_attachment_service();

    // Background tasks: mark stale attachment uploads failed daily and delete expired workspace
    // state hourly, each first a few minutes after boot.
    tokio::spawn(
        rim_infra_services::scheduled_purges::run_attachment_cleanup(
            Arc::clone(&message_attachment_service),
            shutdown_tx.subscribe(),
        ),
    );
    let workspace_state_repo: Arc<
        dyn rim_core::workspace_state::repository::WorkspaceStateRepository,
    > = Arc::new(
        rim_infra_db::sea_orm_impl::workspace_state::SeaOrmWorkspaceStateRepository::new(
            raw_db_for_tasks.clone().into(),
        ),
    );
    tokio::spawn(
        rim_infra_services::scheduled_purges::run_workspace_state_purge(
            workspace_state_repo,
            shutdown_tx.subscribe(),
        ),
    );
    // Background task: end the WebSocket connections of sessions that ended where this
    // process did not hear of it (another process, expiry, a purge), every interval.
    if let Some(interval) = config.ws_session_recheck() {
        tokio::spawn(rim_infra_services::session_revocation::run_recheck(
            service_provider.request_authenticator(),
            Arc::clone(&session_revocations),
            interval,
            shutdown_tx.subscribe(),
        ));
    }
    // Background task: purge the accounts whose deletion grace period ended, hourly.
    tokio::spawn(rim_infra_services::scheduled_purges::run_account_purge(
        service_provider.account_deletion_service(),
        shutdown_tx.subscribe(),
    ));
    let moderation_service = service_provider.moderation_service();
    let plugin_bundle_service = service_provider.plugin_bundle_service();

    if registration_mode == rim_services::auth::dto::RegistrationMode::Open {
        tracing::warn!(
            target: "audit",
            "RIM_OPEN_REGISTRATION=true: open registration mode is active; \
             instance accepts signups without invitation"
        );
    }
    if let rim_services::auth::dto::RegistrationMode::Pow { bits } = registration_mode {
        tracing::warn!(
            target: "audit",
            pow_bits = bits,
            open_registration_ignored = config.open_registration.unwrap_or(false),
            "RIM_REGISTRATION_POW_BITS is set: key-only signups need a proof-of-work or a token"
        );
    }

    let schema = build_schema(
        Arc::clone(&service_provider),
        broadcaster,
        Arc::clone(&presence_manager) as Arc<dyn PresenceService>,
        typing_tracker as Arc<dyn TypingService>,
        observers_for_schema,
        rim_api::RegistrationContext {
            registration_mode: Arc::new(registration_mode),
            audit_retention_days,
            instance_policy,
            instance_limits,
        },
        Some(FederationContext {
            relay: Arc::clone(&s2s.relay) as Arc<dyn RelayService>,
            host: Arc::clone(&s2s.host) as Arc<dyn HostService>,
            workspace_host: Arc::clone(&s2s.workspace_host) as Arc<dyn WorkspaceHostService>,
            workspace_relay: Arc::clone(&s2s.workspace_relay) as Arc<dyn WorkspaceRelayService>,
            info: RelayInfoDto {
                server_id: s2s.state.identity.current().server_id().to_string(),
                roles: s2s
                    .state
                    .settings
                    .role
                    .roles()
                    .into_iter()
                    .map(|r| r.as_str().to_string())
                    .collect(),
                direct_space_id: s2s.direct_space_id,
            },
            relay_role: s2s.state.settings.role.runs(rim_s2s::Role::Relay),
            host_role: s2s.state.settings.role.runs(rim_s2s::Role::Host),
        }),
    );
    let request_authenticator = service_provider.request_authenticator();

    Ok(AppRuntime {
        schema,
        request_authenticator,
        storage_backend,
        storage_service,
        message_attachment_service,
        moderation_service,
        plugin_bundle_service,
        feature_gate,
        s2s,
        db: raw_db_for_tasks,
        shutdown_tx,
        ip_deny_list,
        session_revocations,
    })
}

/// The stored files: `RIM_STORAGE_LOCAL_PATH`, else `files` under the data directory, created
/// when missing, and the storage service over them.
pub(crate) fn build_storage(
    config: &RIMConfig,
    repos: &AppRepos,
) -> std::io::Result<(Arc<dyn StorageBackend>, Arc<dyn StorageService>)> {
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
    Ok((storage_backend, storage_service))
}

/// Log the features that are not enabled, so an operator sees what the instance runs.
fn log_features(gate: &rim_infra_services::feature_gate::InstanceFeatureGate) {
    let disabled: Vec<&str> = gate.disabled().iter().map(ServerFeature::as_str).collect();
    let unavailable: Vec<String> = gate
        .unavailable()
        .iter()
        .map(|(feature, reason)| format!("{feature}: {reason}"))
        .collect();
    tracing::info!(
        disabled = ?disabled,
        unavailable = ?unavailable,
        "instance features (RIM_FEATURES_DISABLED)"
    );
}

/// The relay and host parts of the per-minute cleanup: acked and expired items, orphaned
/// envelopes, expired requests, outbox rows over 7 days, seen request seqs, ended delegations
/// and lapsed subscriptions, then a pull of what each host's outbox holds above what was
/// received from it.
async fn run_relay_cleanup(
    s2s: &S2sRuntime,
    relay_repo: &dyn rim_core::relay::repository::RelayRepository,
) {
    match s2s.relay.cleanup().await {
        Ok(done) => tracing::debug!(
            acked_items = done.acked_items,
            expired_items = done.expired_items,
            orphan_envelopes = done.orphan_envelopes,
            expired_requests = done.expired_requests,
            "relay cleanup"
        ),
        Err(e) => tracing::error!(error = %e, "relay cleanup failed"),
    }
    match s2s.host.cleanup().await {
        Ok(done) => tracing::debug!(
            outbox_rows = done.outbox_rows,
            seen_requests = done.seen_requests,
            "host cleanup"
        ),
        Err(e) => tracing::error!(error = %e, "host cleanup failed"),
    }
    match s2s.workspace_host.cleanup().await {
        Ok(done) => tracing::debug!(
            ended_delegations = done.ended_delegations,
            "delegation cleanup"
        ),
        Err(e) => tracing::error!(error = %e, "delegation cleanup failed"),
    }
    match s2s.workspace_relay.cleanup().await {
        Ok(done) => tracing::debug!(
            lapsed_subscriptions = done.lapsed_subscriptions,
            "subscription cleanup"
        ),
        Err(e) => tracing::error!(error = %e, "subscription cleanup failed"),
    }
    if s2s.state.settings.origin.is_none() {
        return;
    }
    match relay_repo.inbound_peers().await {
        Ok(peers) => {
            for peer in peers {
                if let Err(e) = s2s.relay.pull_outbox(peer.clone()).await {
                    tracing::debug!(peer_server_id = %peer, error = %e, "could not pull a host's outbox");
                }
            }
        }
        Err(e) => tracing::error!(error = %e, "could not list hosts to pull from"),
    }
}

/// How often expired notifications (older than 90 days) are purged: hourly.
const NOTIFICATION_RETENTION_INTERVAL_SECS: u64 = 60 * 60;

/// Idempotent startup seeding for the capability registry.
///
/// Runs before the HTTP listener binds, so the capability gate never sees an empty registry.
/// Emits one `capability_registry_seeded` audit line with the number of builtin capabilities.
async fn seed_builtin_capabilities(
    repo: &dyn rim_core::capability::repository::CapabilityRegistryRepository,
) -> Result<(), String> {
    let specs = rim_infra_services::capability::builtin_capabilities();
    for spec in specs {
        // Use the insert-only seed entrypoint so a builtin that has been
        // soft-retired by an operator (e.g. company-wide `audit.read`
        // disable) is NOT silently re-activated on the next deploy.
        repo.upsert_if_missing(spec.id, spec.is_self_scoped)
            .await
            .map_err(|e| format!("seed {}: {e:?}", spec.id))?;
    }
    tracing::info!(
        target: "audit",
        event = "capability_registry_seeded",
        count = specs.len(),
        "seeded builtin capabilities into registry"
    );
    Ok(())
}
