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
            password: "$argon2id$v=invalid$test_only".to_string(),
            email: format!("{}@test.example.com", username.to_lowercase()),
            display_name: None,
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
/// The storage backend is pointed at a temporary in-memory directory that is
/// never actually written during tests (no file-upload paths are exercised by
/// the registration resolver tests).
pub async fn build_test_schema(
    conn: &DatabaseConnection,
    registration_mode: rim_services::auth::dto::RegistrationMode,
) -> rim_api::AppSchema {
    use std::sync::Arc;

    use rim_api::RegistrationContext;
    use rim_infra_db::sea_orm_impl::{
        bootstrap_token::SeaOrmBootstrapTokenRepository,
        instance_invite::SeaOrmInstanceInviteRepository, invite::SeaOrmInviteRepository,
        keybind::SeaOrmKeybindRepository, server::SeaOrmServerRepository,
        server_member::SeaOrmServerMemberRepository, session::SeaOrmSessionRepository,
        user::SeaOrmUserRepository, user_command::SeaOrmUserCommandRepository,
    };
    use rim_infra_services::storage::DefaultStorageService;
    use rim_services::auth::dto::{PreviewInviteRateLimiterToken, SignupRateLimiterToken};
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
    };

    // No-op storage backend — file upload is not exercised here.
    let tmp_dir = PathBuf::from("/tmp/rim_api_e2e_test_storage");
    std::fs::create_dir_all(&tmp_dir).ok();
    let storage_backend = Arc::new(rim_infra_services::file_storage::LocalFileStorage::new(
        tmp_dir,
    ));
    let storage_service: Arc<dyn StorageService> = Arc::new(DefaultStorageService::new(
        Arc::clone(&storage_backend) as Arc<dyn rim_core::storage::StorageBackend>,
        Arc::clone(&repos.message_attachment_repo),
        1_073_741_824,
    ));

    let broadcaster_inner = Arc::new(NoopBroadcaster);
    let observers = Observers::new(broadcaster_inner as Arc<dyn rim_services::events::Broadcaster>);

    let service_provider: Arc<dyn AppServiceProvider> = Arc::new(DefaultAppServiceProvider::new(
        &repos,
        raw_db,
        None, // no bootstrap secret needed for registration tests
        chrono::Duration::days(30),
        observers.clone(),
        Arc::clone(&storage_service),
        None, // no ingest-token TTL configured for registration tests
        Arc::new(rim_infra_services::email::LogEmailSender),
        None, // no public_base_url configured for registration tests
        Arc::new(rim_infra_services::escalation::InMemoryEscalationScheduler::new()),
    ));

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
    )
}
