use std::env;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};
use tokio::sync::OnceCell;

use rim_core::conversation::model::{ConversationType, NewConversation};
use rim_core::conversation::repository::ConversationRepository;
use rim_core::message::model::NewMessage;
use rim_core::message::repository::MessageRepository;
use rim_core::message_thread::model::MessageThreadId;
use rim_core::server::model::ServerId;
use rim_core::server::repository::ServerRepository;
use rim_core::user::model::UserId;
use rim_core::user::repository::UserRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::conversation::SeaOrmConversationRepository;
use rim_infra_db::sea_orm_impl::message::SeaOrmMessageRepository;
use rim_infra_db::sea_orm_impl::server::SeaOrmServerRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;

use rim_core::conversation::model::ConversationId;

/// Set once this process has recreated and migrated the test database.
static FRESH: OnceCell<()> = OnceCell::const_new();

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
/// which must fail CI, not silently skip. The first call in a process starts from a
/// freshly created, migrated database.
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

    FRESH.get_or_init(|| recreate_database(&url)).await;

    let conn = connect(&url).await;
    eprintln!("db-tests: connected to {}", redact_url(&url));
    conn
}

async fn connect(url: &str) -> DatabaseConnection {
    let mut opts = ConnectOptions::new(url.to_string());
    opts.sqlx_logging(false);
    Database::connect(opts)
        .await
        .expect("failed to connect to the test database")
}

/// Drops and recreates the test database through the `postgres` maintenance database, then
/// migrates it up. The migration round-trip tests leave dropped columns behind, which Postgres
/// never reclaims, so a reused database eventually hits the 1600-column limit.
async fn recreate_database(url: &str) {
    let (base, query) = url.split_once('?').unwrap_or((url, ""));
    let (prefix, name) = base
        .rsplit_once('/')
        .expect("database name in DATABASE_URL");
    let query = if query.is_empty() {
        String::new()
    } else {
        format!("?{query}")
    };

    let maintenance = connect(&format!("{prefix}/postgres{query}")).await;
    maintenance
        .execute_unprepared(&format!("DROP DATABASE IF EXISTS \"{name}\" WITH (FORCE)"))
        .await
        .expect("drop the test database");
    maintenance
        .execute_unprepared(&format!("CREATE DATABASE \"{name}\""))
        .await
        .expect("create the test database");
    maintenance.close().await.expect("close");

    let conn = connect(url).await;
    migration::Migrator::up(&conn, None)
        .await
        .expect("failed to run migrations");
    conn.close().await.expect("close");
}

pub async fn reset_db(conn: &DatabaseConnection) {
    // Full application-table truncate list, alphabetical. Kept in sync with
    // entity/src/*.rs (table_name = "..."); excludes seaql_migrations.
    // CASCADE ensures FK-dependent rows are cleared regardless of declaration order.
    let sql = r#"
TRUNCATE TABLE
  blocked_email,
  capability_registry,
  channel_lens,
  command_audit_log,
  conversation,
  conversation_member,
  conversation_read_state,
  custom_emoji,
  device_key,
  device_link,
  device_request_seen,
  host_delegation,
  host_event_stream,
  instance_invite,
  invite,
  ip_block,
  key_challenge,
  key_log_entry,
  keybind,
  link_preview,
  "macro",
  message,
  message_attachment,
  message_franking_revision,
  message_change_seq,
  message_mention,
  message_pin,
  message_reaction,
  message_report,
  message_thread,
  mls_commit,
  mls_group,
  mls_group_device,
  mls_key_package,
  mls_readd_nonce,
  mls_readd_request,
  mls_welcome,
  mls_welcome_recipient,
  notification_preference,
  password_reset_token,
  peer_key_log_entry,
  peer_server,
  pipeline,
  plugin_bundle,
  profile_card,
  relay_abuse_report,
  relay_block,
  relay_contact,
  relay_device_cursor,
  relay_dm_request,
  relay_envelope,
  relay_item,
  relay_principal_block,
  relay_revocation_fanout,
  relay_subscription,
  remote_identity_cache,
  s2s_nonce,
  s2s_outbox,
  s2s_peer_delivery,
  s2s_peer_inbound,
  server,
  server_ban,
  server_descriptor,
  server_key_log_entry,
  server_member,
  server_member_timeout,
  session,
  user_command,
  user_notification,
  user_notification_settings,
  "user",
  workspace_plugin,
  workspace_state
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
    /// Create a new test context. Panics loudly if `DATABASE_URL` is not set
    /// or does not point at a test database — see [`db_conn`].
    pub async fn new() -> Self {
        let conn = db_conn().await;
        reset_db(&conn).await;
        Self { db: conn.into() }
    }

    pub async fn create_user(&self, username: &str) -> rim_core::user::model::User {
        let repo = SeaOrmUserRepository::new(self.db.clone());
        repo.create_user(rim_core::user::model::NewUser {
            username: username.to_string(),
            password: Some("test_hash".to_string()),
            email: Some(format!("{}@test.example.com", username.to_lowercase())),
            display_name: None,
            key_identity: None,
        })
        .await
        .unwrap()
    }

    /// Create a key-only account: no password, no email, `principal_id` and a stand-in
    /// identity key set (key-only signup).
    pub async fn create_key_only_user(
        &self,
        username: &str,
        principal_id: &str,
    ) -> rim_core::user::model::User {
        let repo = SeaOrmUserRepository::new(self.db.clone());
        repo.create_user(rim_core::user::model::NewUser {
            username: username.to_string(),
            password: None,
            email: None,
            display_name: None,
            key_identity: Some(rim_core::user::model::NewUserKeyIdentity {
                principal_id: principal_id.to_string(),
                identity_key: format!("ed25519:{principal_id}"),
            }),
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
            is_private: false,
            created_by: None,
            encrypted: false,
        })
        .await
        .unwrap()
    }

    /// Inserts a thread with no root message. Production threads always start from a root
    /// (`create_thread_for_root`); tests that only need a thread to hang replies on use this.
    pub async fn create_thread(
        &self,
        conv_id: ConversationId,
    ) -> rim_core::message_thread::model::MessageThread {
        use sea_orm::{ActiveModelTrait, Set};

        let now: sea_orm::prelude::DateTimeWithTimeZone = chrono::Utc::now().into();
        entity::message_thread::ActiveModel {
            id: Set(MessageThreadId::new().into()),
            conversation_id: Set(conv_id.into()),
            first_message_id: Set(None),
            created_at: Set(now),
            last_message_at: Set(now),
        }
        .insert(&self.db)
        .await
        .unwrap()
        .into()
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
        })
        .await
        .unwrap()
    }
}
