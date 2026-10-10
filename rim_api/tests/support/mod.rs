/// Shared test fixtures for `rim_api` e2e tests.
///
/// Provides a [`TestCtx`] that connects to the database, runs migrations once
/// per test process (idempotent), resets state between tests, and exposes
/// builder helpers for users, servers, and registration tokens.
///
/// Use [`TestCtx::new`] to get a context. Panics loudly when `DATABASE_URL`
/// is absent or does not look like a test database, rather than silently
/// skipping the test (matches the pattern used by
/// `rim_infra_db/tests/support/mod.rs`).
#[allow(dead_code)] // Not every test binary that includes `support` uses the GraphQL helpers.
pub mod graphql;
#[allow(dead_code)] // Only the encryption tests use the MLS client.
pub mod mls;
#[allow(dead_code)] // Only the server-to-server tests start servers.
pub mod nodes;
#[allow(dead_code)] // Not every test binary that includes `support` uses the registration seeders.
pub mod registration;

use std::env;
use std::path::PathBuf;
use std::sync::OnceLock;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

use rim_core::auth::model::{NewSession, SessionId};
use rim_core::auth::repository::SessionRepository;
use rim_core::server::model::Server;
use rim_core::server::repository::ServerRepository;
use rim_core::server_member::model::ServerMemberRole;
use rim_core::server_member::repository::ServerMemberRepository;
use rim_core::user::model::{NewUser, User};
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::server_member::SeaOrmServerMemberRepository;
use rim_infra_db::sea_orm_impl::session::SeaOrmSessionRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::{AppRepos, DefaultAppServiceProvider};
use rim_services::events::{NoopBroadcaster, Observers};
use rim_services::provider::AppServiceProvider;

static MIGRATED: OnceLock<()> = OnceLock::new();

fn is_test_db_url(url: &str) -> bool {
    let db_name = url.rsplit('/').next().unwrap_or("");
    let db_name = db_name.split('?').next().unwrap_or(db_name);
    let db_name = db_name.to_ascii_lowercase();
    ["testdb", "_test", "-test", "testing", "_testing"]
        .iter()
        .any(|needle| db_name.contains(needle))
}

/// Redact the password from a DB URL for safe logging.
/// Replaces `://user:password@host` with `://user:***@host`.
fn redact_url(url: &str) -> String {
    if let Some(at) = url.find('@')
        && let Some(scheme_end) = url.find("://")
    {
        let credentials = &url[scheme_end + 3..at];
        if let Some(colon) = credentials.find(':') {
            let user = &credentials[..colon];
            let host_and_rest = &url[at..];
            let scheme = &url[..scheme_end + 3];
            return format!("{scheme}{user}:***{host_and_rest}");
        }
    }
    "<redacted>".to_string()
}

/// The audience the test provider accepts in key login proofs.
#[allow(dead_code)]
pub const KEY_LOGIN_AUDIENCE: &str = "localhost:8080";

/// The test provider's franking key 1, so tests can compute and forge receipts.
#[allow(dead_code)]
pub const FRANKING_KEY: [u8; 32] = [
    0x80, 0x81, 0x82, 0x83, 0x84, 0x85, 0x86, 0x87, 0x88, 0x89, 0x8a, 0x8b, 0x8c, 0x8d, 0x8e, 0x8f,
    0x90, 0x91, 0x92, 0x93, 0x94, 0x95, 0x96, 0x97, 0x98, 0x99, 0x9a, 0x9b, 0x9c, 0x9d, 0x9e, 0x9f,
];

/// Connect to the test database, or **panic loudly**. Only reachable under the
/// crate's db-test feature (all test files are `#![cfg(feature = "…")]`-gated),
/// so a panic here means the feature was enabled without a usable test DB —
/// which must fail CI, not silently skip. See CLAUDE.md P6-W6.
pub async fn db_conn() -> DatabaseConnection {
    let url = match env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "DATABASE_URL must be set when running with --features e2e-tests \
             (e.g. postgres://rim:rim@localhost/rim_testdb). Every db-test requires it; \
             without it the suite would silently skip and report a false green."
        ),
    };
    assert!(
        is_test_db_url(&url),
        "DATABASE_URL must point to a test DB (dbname containing 'testdb'/'_test'/'testing'); \
         refusing to run destructive db-tests against: {}",
        redact_url(&url),
    );

    let mut opts = ConnectOptions::new(url.clone());
    opts.sqlx_logging(false);

    let conn = Database::connect(opts)
        .await
        .expect("failed to connect to DATABASE_URL");

    if MIGRATED.get().is_none() {
        migration::Migrator::up(&conn, None)
            .await
            .expect("failed to run migrations");
        let _ = MIGRATED.set(());
    }

    conn
}

/// Reset all relevant tables between tests.
///
/// Uses `TRUNCATE … RESTART IDENTITY CASCADE` so FK-dependent rows are removed
/// and serial sequences are reset.  Only tables that may accumulate state from
/// registration e2e tests are listed here.
pub async fn reset_db(conn: &DatabaseConnection) {
    let sql = r#"
TRUNCATE TABLE
  session,
  key_challenge,
  bootstrap_token,
  instance_invite,
  invite,
  server_member,
  server,
  "user"
RESTART IDENTITY CASCADE;
"#;
    conn.execute_unprepared(sql)
        .await
        .expect("failed to truncate tables");
}

/// Shared test context for registration e2e tests.
///
/// Wraps a live database connection and exposes helpers for seeding test data
/// (users, servers, tokens).  Construct via [`TestCtx::new`].
#[allow(dead_code)]
pub struct TestCtx {
    /// Raw database connection used by repo helpers.
    pub conn: DatabaseConnection,
    /// [`DbHandle`] wrapping the same connection, accepted by Sea-ORM repos.
    pub db: DbHandle,
}

#[allow(dead_code)]
impl TestCtx {
    /// Build a new `TestCtx`. Panics loudly if `DATABASE_URL` is not set or
    /// does not point at a test database — see [`db_conn`]. Runs all pending
    /// migrations exactly once per process and truncates the relevant tables
    /// so each test starts with a clean slate.
    pub async fn new() -> Self {
        let conn = db_conn().await;
        reset_db(&conn).await;
        let db: DbHandle = conn.clone().into();
        Self { conn, db }
    }

    /// Create a user with the given `username` and an unusable password hash.
    pub async fn create_user(&self, username: &str) -> User {
        let repo = SeaOrmUserRepository::new(self.db.clone());
        repo.create_user(NewUser {
            username: username.to_string(),
            password: Some("$argon2id$v=invalid$test_only".to_string()),
            email: Some(format!("{}@test.example.com", username.to_lowercase())),
            display_name: None,
            key_identity: None,
        })
        .await
        .expect("create_user failed")
    }

    /// Create a workspace (server) with the given `name`.
    pub async fn create_server(&self, name: &str) -> Server {
        let repo = SeaOrmServerRepository::new(self.db.clone());
        repo.create_server(name)
            .await
            .expect("create_server failed")
    }

    /// Add `user` as `Owner` of `server`.
    pub async fn add_owner(&self, server: &Server, user: &User) {
        self.add_member(server, user, ServerMemberRole::Owner).await;
    }

    /// Add `user` to `server` with the given `role`.
    pub async fn add_member(&self, server: &Server, user: &User, role: ServerMemberRole) {
        let repo = SeaOrmServerMemberRepository::new(self.db.clone());
        repo.add_member(server.id, user.id, role)
            .await
            .expect("add_member failed");
    }

    /// Create a live (unexpired, unrevoked) session row for `user` and return its id.
    ///
    /// Pair with [`graphql::Caller`] to execute GraphQL as that user: the HTTP layer would
    /// resolve the session cookie to exactly this `(user_id, session_id)` pair.
    pub async fn create_session(&self, user: &User) -> SessionId {
        let repo = SeaOrmSessionRepository::new(self.db.clone());
        repo.create_session(NewSession {
            user_id: user.id,
            token_hash: format!("e2e-session-{}", user.id),
            expires_at: chrono::Utc::now() + chrono::Duration::days(1),
            user_agent: None,
            ip: None,
        })
        .await
        .expect("create_session failed")
        .id
    }

    /// Build the real [`AppServiceProvider`] over the test database, for tests that need to
    /// reach a service the GraphQL schema does not expose (e.g. the alert-webhook path).
    pub fn build_provider(&self) -> std::sync::Arc<dyn AppServiceProvider> {
        build_test_provider(&self.conn).0
    }

    /// Build the schema and the provider it runs on, recording every event the services emit.
    pub fn build_recording(
        &self,
        registration_mode: rim_services::auth::dto::RegistrationMode,
    ) -> (
        rim_api::AppSchema,
        std::sync::Arc<dyn AppServiceProvider>,
        std::sync::Arc<RecordingBroadcaster>,
    ) {
        let events = std::sync::Arc::new(RecordingBroadcaster::default());
        let (provider, observers) = build_test_provider_with(
            &self.conn,
            std::sync::Arc::clone(&events) as std::sync::Arc<dyn rim_services::events::Broadcaster>,
        );
        let schema = schema_for(
            std::sync::Arc::clone(&provider),
            observers,
            registration_mode,
        );
        (schema, provider, events)
    }

    /// Build a full [`AppSchema`][rim_api::AppSchema] backed by real services
    /// and the test database.
    ///
    /// Uses default (invite-only) registration mode.  Pass the returned schema
    /// directly to `schema.execute(Request::new(...))` in tests.
    pub async fn build_schema(
        &self,
        registration_mode: rim_services::auth::dto::RegistrationMode,
    ) -> rim_api::AppSchema {
        build_test_schema(&self.conn, registration_mode).await
    }
}

/// Build a full [`AppSchema`][rim_api::AppSchema] backed by the given
/// database connection and real service implementations.
///
/// Files are stored under a fixed temporary directory; only the plugin bundle
/// tests write there.
pub async fn build_test_schema(
    conn: &DatabaseConnection,
    registration_mode: rim_services::auth::dto::RegistrationMode,
) -> rim_api::AppSchema {
    let (service_provider, observers) = build_test_provider(conn);
    schema_for(service_provider, observers, registration_mode)
}

/// The schema over `service_provider`, wired with the observers it was built with.
fn schema_for(
    service_provider: std::sync::Arc<dyn AppServiceProvider>,
    observers: Observers,
    registration_mode: rim_services::auth::dto::RegistrationMode,
) -> rim_api::AppSchema {
    use std::sync::Arc;

    use rim_api::RegistrationContext;
    use rim_services::auth::dto::{PreviewInviteRateLimiterToken, SignupRateLimiterToken};

    let presence_manager = Arc::new(rim_infra_services::presence::PresenceManager::new(
        Arc::new(observers.clone()),
    ));
    let typing_tracker = Arc::new(rim_infra_services::typing::TypingTracker::new());
    let observers_arc = Arc::new(observers);

    let broadcaster = rim_api::broadcast::InMemoryBroadcaster::new(16);

    rim_api::build_schema(
        service_provider,
        broadcaster,
        presence_manager as Arc<dyn rim_services::presence::PresenceService>,
        typing_tracker as Arc<dyn rim_services::typing::TypingService>,
        observers_arc,
        RegistrationContext {
            registration_mode: Arc::new(registration_mode),
            preview_rate_limiter: Arc::new(PreviewInviteRateLimiterToken),
            signup_rate_limiter: Arc::new(SignupRateLimiterToken),
        },
        None,
    )
}

/// Every event emitted through it, in order.
#[derive(Default)]
pub struct RecordingBroadcaster(std::sync::Mutex<Vec<rim_services::events::WorkspaceEvent>>);

#[allow(dead_code)]
impl RecordingBroadcaster {
    pub fn events(&self) -> Vec<rim_services::events::WorkspaceEvent> {
        self.0.lock().expect("lock").clone()
    }
}

impl rim_services::events::Broadcaster for RecordingBroadcaster {
    fn broadcast(&self, event: rim_services::events::WorkspaceEvent) {
        self.0.lock().expect("lock").push(event);
    }
}

/// Build the real service provider (and the observers it was wired with) over `conn`.
pub fn build_test_provider(
    conn: &DatabaseConnection,
) -> (std::sync::Arc<dyn AppServiceProvider>, Observers) {
    build_test_provider_with(conn, std::sync::Arc::new(NoopBroadcaster))
}

/// Build the real service provider over `conn`, emitting events through `broadcaster`.
pub fn build_test_provider_with(
    conn: &DatabaseConnection,
    broadcaster: std::sync::Arc<dyn rim_services::events::Broadcaster>,
) -> (std::sync::Arc<dyn AppServiceProvider>, Observers) {
    use std::sync::Arc;

    use rim_infra_db::sea_orm_impl::{
        bootstrap_token::SeaOrmBootstrapTokenRepository,
        instance_invite::SeaOrmInstanceInviteRepository, invite::SeaOrmInviteRepository,
        keybind::SeaOrmKeybindRepository, server::SeaOrmServerRepository,
        server_member::SeaOrmServerMemberRepository, session::SeaOrmSessionRepository,
        user::SeaOrmUserRepository, user_command::SeaOrmUserCommandRepository,
    };
    use rim_infra_services::storage::DefaultStorageService;
    use rim_services::storage::service::StorageService;

    let db: DbHandle = conn.clone().into();
    let raw_db = conn.clone();

    // Wire up all repos.
    let repos = AppRepos {
        bootstrap_token_repo: Arc::new(SeaOrmBootstrapTokenRepository::new(db.clone())),
        instance_invite_repo: Arc::new(SeaOrmInstanceInviteRepository::new(db.clone())),
        channel_lens_repo: Arc::new(
            rim_infra_db::sea_orm_impl::channel_lens::SeaOrmChannelLensRepository::new(db.clone()),
        ),
        conversation_repo: Arc::new(
            rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository::new(db.clone()),
        ),
        conversation_member_repo: Arc::new(
            rim_infra_db::sea_orm_impl::conversation_member::SeaOrmConversationMemberRepository::new(db.clone()),
        ),
        keybind_repo: Arc::new(SeaOrmKeybindRepository::new(db.clone())),
        macro_repo: Arc::new(rim_infra_db::sea_orm_impl::macro_def::SeaOrmMacroRepository::new(
            db.clone(),
        )),
        message_repo: Arc::new(rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository::new(
            db.clone(),
        )),
        message_attachment_repo: Arc::new(
            rim_infra_db::sea_orm_impl::message_attachment::SeaOrmMessageAttachmentRepository::new(db.clone()),
        ),
        message_thread_repo: Arc::new(
            rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository::new(db.clone()),
        ),
        message_reaction_repo: Arc::new(
            rim_infra_db::sea_orm_impl::message_reaction::SeaOrmMessageReactionRepository::new(
                db.clone(),
            ),
        ),
        message_mention_repo: Arc::new(
            rim_infra_db::sea_orm_impl::message_mention::SeaOrmMessageMentionRepository::new(
                db.clone(),
            ),
        ),
        message_pin_repo: Arc::new(
            rim_infra_db::sea_orm_impl::message_pin::SeaOrmMessagePinRepository::new(db.clone()),
        ),
        notification_preference_repo: Arc::new(
            rim_infra_db::sea_orm_impl::notification_preference::SeaOrmNotificationPreferenceRepository::new(
                db.clone(),
            ),
        ),
        password_reset_token_repo: Arc::new(
            rim_infra_db::sea_orm_impl::password_reset_token::SeaOrmPasswordResetTokenRepository::new(db.clone()),
        ),
        read_state_repo: Arc::new(
            rim_infra_db::sea_orm_impl::conversation_read_state::SeaOrmConversationReadStateRepository::new(
                db.clone(),
            ),
        ),
        user_command_repo: Arc::new(SeaOrmUserCommandRepository::new(db.clone())),
        server_repo: Arc::new(SeaOrmServerRepository::new(db.clone())),
        server_member_repo: Arc::new(SeaOrmServerMemberRepository::new(db.clone())),
        user_repo: Arc::new(SeaOrmUserRepository::new(db.clone())),
        session_repo: Arc::new(SeaOrmSessionRepository::new(db.clone())),
        key_identity_repo: Arc::new(
            rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyIdentityRepository::new(db.clone()),
        ),
        key_challenge_repo: Arc::new(
            rim_infra_db::sea_orm_impl::key_identity::SeaOrmKeyChallengeRepository::new(db.clone()),
        ),
        device_link_repo: Arc::new(
            rim_infra_db::sea_orm_impl::device_link::SeaOrmDeviceLinkRepository::new(db.clone()),
        ),
        workspace_plugin_repo: Arc::new(
            rim_infra_db::sea_orm_impl::workspace_plugin::SeaOrmWorkspacePluginRepository::new(
                db.clone(),
            ),
        ),
        e2ee_repo: Arc::new(rim_infra_db::sea_orm_impl::e2ee::SeaOrmE2eeRepository::new(
            db.clone(),
        )),
        relay_repo: Arc::new(rim_infra_db::sea_orm_impl::relay::SeaOrmRelayRepository::new(
            db.clone(),
        )),
        host_repo: Arc::new(rim_infra_db::sea_orm_impl::host::SeaOrmHostRepository::new(
            db.clone(),
        )),
        relay_abuse_repo: Arc::new(
            rim_infra_db::sea_orm_impl::relay_abuse::SeaOrmRelayAbuseRepository::new(db.clone()),
        ),
        invite_repo: Arc::new(SeaOrmInviteRepository::new(db.clone())),
        link_preview_repo: Arc::new(
            rim_infra_db::sea_orm_impl::link_preview::SeaOrmLinkPreviewRepository::new(db.clone()),
        ),
        custom_emoji_repo: Arc::new(
            rim_infra_db::sea_orm_impl::custom_emoji::SeaOrmCustomEmojiRepository::new(db.clone()),
        ),
        incident_repo: Arc::new(
            rim_infra_db::sea_orm_impl::incident::SeaOrmIncidentRepository::new(db.clone()),
        ),
        incident_phase_repo: Arc::new(
            rim_infra_db::sea_orm_impl::incident_phase::SeaOrmIncidentPhaseRepository::new(
                db.clone(),
            ),
        ),
        incident_message_tag_repo: Arc::new(
            rim_infra_db::sea_orm_impl::incident_message_tag::SeaOrmIncidentMessageTagRepository::new(db.clone()),
        ),
        ingest_token_repo: Arc::new(
            rim_infra_db::sea_orm_impl::ingest_token::SeaOrmIngestTokenRepository::new(db.clone()),
        ),
        skill_profile_repo: Arc::new(
            rim_infra_db::sea_orm_impl::skill_profile::SeaOrmSkillProfileRepository::new(db.clone()),
        ),
        workspace_state_repo: Arc::new(
            rim_infra_db::sea_orm_impl::workspace_state::SeaOrmWorkspaceStateRepository::new(
                db.clone(),
            ),
        ),
        pipeline_repo: Arc::new(
            rim_infra_db::sea_orm_impl::pipeline::SeaOrmPipelineRepository::new(db.clone()),
        ),
        capability_registry_repo: Arc::new(
            rim_infra_db::sea_orm_impl::capability_registry::SeaOrmCapabilityRegistryRepository::new(db.clone()),
        ),
        on_call_schedule_repo: Arc::new(
            rim_infra_db::sea_orm_impl::on_call_schedule::SeaOrmOnCallScheduleRepository::new(
                db.clone(),
            ),
        ),
        rotation_layer_repo: Arc::new(
            rim_infra_db::sea_orm_impl::on_call_rotation_layer::SeaOrmRotationLayerRepository::new(
                db.clone(),
            ),
        ),
        rotation_participant_repo: Arc::new(
            rim_infra_db::sea_orm_impl::on_call_rotation_participant::SeaOrmRotationParticipantRepository::new(
                db.clone(),
            ),
        ),
        on_call_override_repo: Arc::new(
            rim_infra_db::sea_orm_impl::on_call_override::SeaOrmOnCallOverrideRepository::new(
                db.clone(),
            ),
        ),
        escalation_policy_repo: Arc::new(
            rim_infra_db::sea_orm_impl::escalation_policy::SeaOrmEscalationPolicyRepository::new(
                db.clone(),
            ),
        ),
        escalation_step_repo: Arc::new(
            rim_infra_db::sea_orm_impl::escalation_step::SeaOrmEscalationStepRepository::new(
                db.clone(),
            ),
        ),
        conversation_escalation_policy_repo: Arc::new(
            rim_infra_db::sea_orm_impl::conversation_escalation_policy::SeaOrmConversationEscalationPolicyRepository::new(
                db.clone(),
            ),
        ),
        escalation_run_repo: Arc::new(
            rim_infra_db::sea_orm_impl::escalation_run::SeaOrmEscalationRunRepository::new(
                db.clone(),
            ),
        ),
        workspace_feature_repo: Arc::new(
            rim_infra_db::sea_orm_impl::workspace_feature::SeaOrmWorkspaceFeatureRepository::new(
                db.clone(),
            ),
        ),
        moderation_repo: Arc::new(
            rim_infra_db::sea_orm_impl::moderation::SeaOrmModerationRepository::new(db.clone()),
        ),
        notification_repo: Arc::new(
            rim_infra_db::sea_orm_impl::notification::SeaOrmNotificationRepository::new(
                db.clone(),
            ),
        ),
    };

    // Local storage in a fixed temporary directory; plugin bundle tests write to it.
    let tmp_dir = PathBuf::from("/tmp/rim_api_e2e_test_storage");
    std::fs::create_dir_all(&tmp_dir).ok();
    let storage_backend = Arc::new(rim_infra_services::file_storage::LocalFileStorage::new(
        tmp_dir,
    ));
    let storage_service: Arc<dyn StorageService> = Arc::new(DefaultStorageService::new(
        Arc::clone(&storage_backend) as Arc<dyn rim_core::storage::StorageBackend>,
        Arc::clone(&repos.message_attachment_repo),
        Arc::clone(&repos.workspace_plugin_repo),
        1_073_741_824,
    ));

    let observers = Observers::new(broadcaster);

    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        chrono::Duration::days(30),
        observers.clone(),
        Arc::clone(&storage_service),
        None, // no ingest-token TTL configured for registration tests
        Arc::new(rim_infra_services::email::LogEmailSender),
        None, // no public_base_url configured for registration tests
        Arc::new(rim_infra_services::escalation::InMemoryEscalationScheduler::new()),
        vec![KEY_LOGIN_AUDIENCE.to_string()],
        rim_infra_services::franking_keys::SharedFrankingKeys::fixed(
            rim_infra_services::franking_keys::FrankingKeys::new(vec![
                rim_infra_services::franking_keys::FrankingKey::new(
                    1,
                    FRANKING_KEY,
                    chrono::Utc::now(),
                ),
            ])
            .expect("franking keys"),
        ),
        rim_infra_services::FederationHooks::default(),
    ));

    (service_provider, observers)
}
