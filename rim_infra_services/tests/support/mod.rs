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
    if let Some(at) = url.find('@') {
        if let Some(scheme_end) = url.find("://") {
            let credentials = &url[scheme_end + 3..at];
            if let Some(colon) = credentials.find(':') {
                let user = &credentials[..colon];
                let host_and_rest = &url[at..];
                let scheme = &url[..scheme_end + 3];
                return format!("{scheme}{user}:***{host_and_rest}");
            }
        }
    }
    "<redacted>".to_string()
}

/// Obtain a database connection or skip the test.
///
/// Returns `None` when `DATABASE_URL` is unset (test is skipped), `Some` when
/// the connection succeeds and all migrations have been applied.
///
/// The function is idempotent: migrations are applied at most once per
/// process thanks to the `OnceLock` guard.
pub async fn db_conn_or_skip() -> Option<DatabaseConnection> {
    let url = match env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!(
                "SKIP: rim_infra_services integration tests require DATABASE_URL \
                 (e.g. postgres://rim:rim@localhost/rim_testdb)."
            );
            return None;
        }
    };

    if !is_test_db_url(&url) {
        eprintln!(
            "SKIP: DATABASE_URL must point to a test DB \
             (dbname must contain 'testdb', '_test', 'testing', etc.).\n  got: {url}"
        );
        return None;
    }

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

    Some(conn)
}

/// Truncate all application tables so each test starts from a clean slate.
///
/// This includes `bootstrap_token` and `instance_invite` which are not in the
/// `rim_infra_db` truncate list (those tests don't need them).
pub async fn reset_db(conn: &DatabaseConnection) {
    let sql = r#"
TRUNCATE TABLE
  bootstrap_token,
  instance_invite,
  incident_message_tag,
  incident_phase,
  incident,
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
