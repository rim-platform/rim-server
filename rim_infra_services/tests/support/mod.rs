use std::env;
use std::sync::OnceLock;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

static MIGRATED: OnceLock<()> = OnceLock::new();

fn is_test_db_url(url: &str) -> bool {
    let db_name = url.rsplit('/').next().unwrap_or("");
    let db_name = db_name.split('?').next().unwrap_or(db_name);
    let db_name = db_name.to_ascii_lowercase();
    ["testdb", "_test", "-test", "testing", "_testing"]
        .iter()
        .any(|needle| db_name.contains(needle))
}

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
/// which must fail CI, not silently skip.
pub async fn db_conn() -> DatabaseConnection {
    let url = match env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => panic!(
            "DATABASE_URL must be set when running with --features db-tests \
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

    eprintln!("infra-services db-tests: connected to {}", redact_url(&url));

    if MIGRATED.get().is_none() {
        migration::Migrator::up(&conn, None)
            .await
            .expect("failed to run migrations");
        let _ = MIGRATED.set(());
    }

    conn
}

/// Truncate all application tables so each test starts from a clean slate.
///
/// This includes `instance_invite`, which is not in the `rim_infra_db` truncate list (those
/// tests don't need it).
pub async fn reset_db(conn: &DatabaseConnection) {
    let sql = r#"
TRUNCATE TABLE
  instance_invite,
  conversation_member,
  message_attachment,
  message,
  message_thread,
  user_command,
  keybind,
  invite,
  session,
  server_member,
  conversation,
  server,
  "user"
RESTART IDENTITY CASCADE;
"#;
    conn.execute_unprepared(sql)
        .await
        .expect("failed to truncate tables");
}

/// The audience the key login proofs of these tests name.
#[allow(dead_code)]
pub const AUDIENCE: &str = "localhost:8080";

/// The real services over `repos` and `db`, with the instance caps `limits`: a fresh password
/// key, every feature on, and files in a directory under the system's temporary directory.
#[allow(dead_code)]
pub fn provider(
    db: DatabaseConnection,
    repos: &rim_infra_services::AppRepos,
    limits: rim_services::limits::InstanceLimits,
) -> rim_infra_services::DefaultAppServiceProvider {
    use std::sync::Arc;

    use rim_infra_services::franking_keys::{FrankingKey, FrankingKeys, SharedFrankingKeys};

    let files = env::temp_dir().join("rim_infra_services_db_test_storage");
    std::fs::create_dir_all(&files).expect("storage directory");
    let storage = rim_infra_services::storage::DefaultStorageService::new(
        Arc::new(rim_infra_services::file_storage::LocalFileStorage::new(
            files,
        )),
        Arc::clone(&repos.message_attachment_repo),
        Arc::clone(&repos.workspace_plugin_repo),
        1_073_741_824,
    );
    let hooks = rim_infra_services::FederationHooks {
        password_key: Some(
            rim_infra_services::password_key::PasswordKey::generate().expect("password key"),
        ),
        ..Default::default()
    };
    rim_infra_services::DefaultAppServiceProvider::new(
        repos,
        db,
        chrono::Duration::days(30),
        rim_services::events::Observers::new(Arc::new(rim_services::events::NoopBroadcaster)),
        Arc::new(storage),
        Arc::new(rim_infra_services::email::LogEmailSender),
        None,
        vec![AUDIENCE.to_string()],
        SharedFrankingKeys::fixed(
            FrankingKeys::new(vec![FrankingKey::new(1, [0x80; 32], chrono::Utc::now())])
                .expect("franking keys"),
        ),
        hooks,
        Arc::new(rim_infra_services::feature_gate::InstanceFeatureGate::all_enabled()),
        limits,
    )
}

/// Sign `username` up through `provider`'s key signup from `remote_ip`, with the version 2
/// identity from `seed`, a first device and a password: in open registration without `token`,
/// in invite-only registration with it.
///
/// # Errors
/// The signup's refusal.
#[allow(dead_code)]
pub async fn key_signup(
    provider: &rim_infra_services::DefaultAppServiceProvider,
    username: &str,
    seed: u8,
    token: Option<rim_services::auth::command::SignupToken>,
    remote_ip: &str,
) -> rim_services::ServiceResult<rim_services::auth::dto::SignupResultDto> {
    use rim_identity::testing::{TestIdentity, device_login, signing_key};
    use rim_identity::{KeyState, SignedStatement, StatementVersion};
    use rim_services::account_password::dto::PasswordRegistrationStartCmd;
    use rim_services::auth::dto::RegistrationMode;
    use rim_services::key_auth::command::{
        IssueKeyChallengeCmd, SessionKeepChoice, SignedPayload, SignupWithKeyCmd,
    };
    use rim_services::provider::AppServiceProvider;

    let signed = |s: SignedStatement| SignedPayload {
        payload: s.payload,
        signature: s.signature,
    };
    let identity = TestIdentity::with_version(seed, StatementVersion::V2);
    let inception = identity.inception();
    let state = KeyState::from_inception(&inception).expect("inception");
    let device_id = uuid::Uuid::now_v7();
    let device = signing_key(seed.wrapping_add(50));
    let cert = identity.device_cert(&state, device_id, &device, "Laptop");
    let key_auth = provider.key_auth_service();
    let challenge = key_auth
        .issue_challenge(IssueKeyChallengeCmd {
            remote_ip: remote_ip.to_string(),
        })
        .await?;
    let proof = device_login(
        &device,
        &state,
        device_id,
        challenge.challenge_id.into(),
        &challenge.nonce,
        AUDIENCE,
    );
    let (registration, request) =
        rim_auth::opaque::client::Registration::start(b"correct horse battery staple");
    let response = provider
        .account_password_service()
        .registration_start(PasswordRegistrationStartCmd {
            principal_id: state.principal_id.clone(),
            request,
            remote_ip: remote_ip.to_string(),
        })
        .await?;
    let (upload, _) = registration.finish(&response).expect("registration upload");
    let registration_mode = if token.is_some() {
        RegistrationMode::InviteOnly
    } else {
        RegistrationMode::Open
    };
    key_auth
        .signup_with_key(SignupWithKeyCmd {
            username: username.to_string(),
            display_name: None,
            key_log: vec![signed(inception)],
            device: signed(cert),
            proof: signed(proof),
            password_registration: upload,
            registration_mode,
            token,
            proof_of_work: None,
            remote_ip: remote_ip.to_string(),
            terms_accepted_url: None,
            confirmed_minimum_age: None,
            keep: SessionKeepChoice {
                keep_signed_in: true,
                keep_key_held: true,
            },
        })
        .await
}
