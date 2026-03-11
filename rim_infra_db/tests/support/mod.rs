use std::env;
use std::sync::OnceLock;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

static MIGRATED: OnceLock<()> = OnceLock::new();

fn is_test_db_url(url: &str) -> bool {
    // Very small safety heuristic: only allow URLs that clearly point to a test database.
    // This is intentionally conservative.
    let db_name = url.rsplit('/').next().unwrap_or("");
    let db_name = db_name.split('?').next().unwrap_or(db_name);
    let db_name = db_name.to_ascii_lowercase();

    ["testdb", "_test", "-test", "testing", "_testing"]
        .iter()
        .any(|needle| db_name.contains(needle))
}

pub async fn db_conn_or_skip() -> Option<DatabaseConnection> {
    let url = match env::var("DATABASE_URL") {
        Ok(v) if !v.trim().is_empty() => v,
        _ => {
            eprintln!(
                "SKIP: rim_infra_db integration tests require DATABASE_URL (e.g. postgres://...)."
            );
            return None;
        }
    };

    if !is_test_db_url(&url) {
        eprintln!(
            "SKIP: DATABASE_URL must point to a test DB (dbname containing e.g. 'testdb', '_test', 'testing').\n  got: {url}"
        );
        return None;
    }

    let mut opts = ConnectOptions::new(url);
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

    Some(conn)
}

pub async fn reset_db(conn: &DatabaseConnection) {
    // Keep this list explicit and stable; exclude seaql_migrations.
    // CASCADE ensures FK-dependent rows are cleared.
    let sql = r#"
TRUNCATE TABLE
  conversation_member,
  message_attachment,
  message,
  message_thread,
  user_command,
  invite,
  session,
  server_member,
  conversation,
  server,
  "user"
RESTART IDENTITY CASCADE;
"#;

    // SeaORM's `ConnectionTrait::execute` takes a `StatementBuilder`, not a raw `Statement`.
    // For a raw SQL string, use `execute_unprepared`.
    conn.execute_unprepared(sql)
        .await
        .expect("failed to truncate tables");
}
