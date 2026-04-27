use std::env;
use std::sync::OnceLock;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::message_thread::repository::MessageThreadRepository;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::message_thread::SeaOrmMessageThreadRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

use rim_core::conversation::model::ConversationId;

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

/// Redact the password from a DB URL for safe logging.
/// Replaces `://user:password@host` with `://user:***@host`.
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

    let mut opts = ConnectOptions::new(url.clone());
    opts.sqlx_logging(false);

    let conn = Database::connect(opts)
        .await
        .expect("failed to connect to DATABASE_URL");

    eprintln!("db-tests: connected to {}", redact_url(&url));

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

    // SeaORM's `ConnectionTrait::execute` takes a `StatementBuilder`, not a raw `Statement`.
    // For a raw SQL string, use `execute_unprepared`.
    conn.execute_unprepared(sql)
        .await
        .expect("failed to truncate tables");
}

#[allow(dead_code)]
pub struct TestCtx {
    pub db: DbHandle,
}

#[allow(dead_code)]
impl TestCtx {
    /// Create a new test context. Returns `None` if `DATABASE_URL` is not set (test skipped).
    pub async fn new() -> Option<Self> {
        let conn = db_conn_or_skip().await?;
        reset_db(&conn).await;
        Some(Self { db: conn.into() })
    }

    pub async fn create_user(&self, username: &str) -> rim_core::user::model::User {
        let repo = SeaOrmUserRepository::new(self.db.clone());
        repo.create_user(rim_core::user::model::NewUser {
            username: username.to_string(),
            password: "test_hash".to_string(),
        })
        .await
        .unwrap()
    }

    pub async fn create_server(&self, name: &str) -> rim_core::server::model::Server {
        let repo = SeaOrmServerRepository::new(self.db.clone());
        repo.create_server(name).await.unwrap()
    }

    pub async fn create_conversation(
        &self,
        server_id: ServerId,
        conv_type: ConversationType,
    ) -> rim_core::conversation::model::Conversation {
        let repo = SeaOrmConversationRepository::new(self.db.clone());
        repo.create_conversation(NewConversation {
            server_id,
            conversation_type: conv_type,
            title: None,
        })
        .await
        .unwrap()
    }

    pub async fn create_thread(
        &self,
        conv_id: ConversationId,
    ) -> rim_core::message_thread::model::MessageThread {
        let repo = SeaOrmMessageThreadRepository::new(self.db.clone());
        repo.create_thread(conv_id).await.unwrap()
    }

    pub async fn create_message(
        &self,
        conv_id: ConversationId,
        user_id: UserId,
        text: &str,
    ) -> rim_core::message::model::Message {
        let repo = SeaOrmMessageRepository::new(self.db.clone());
        repo.create_message(&NewMessage {
            conversation_id: conv_id,
            thread_id: None,
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_by: user_id,
            text: Some(text.to_string()),
            attachments: vec![],
        })
        .await
        .unwrap()
    }

    pub async fn create_thread_message(
        &self,
        conv_id: ConversationId,
        thread_id: MessageThreadId,
        user_id: UserId,
        text: &str,
    ) -> rim_core::message::model::Message {
        let repo = SeaOrmMessageRepository::new(self.db.clone());
        repo.create_message(&NewMessage {
            conversation_id: conv_id,
            thread_id: Some(thread_id),
            parent_message_id: None,
            forwarded_from_message_id: None,
            sent_by: user_id,
            text: Some(text.to_string()),
            attachments: vec![],
        })
        .await
        .unwrap()
    }
}
