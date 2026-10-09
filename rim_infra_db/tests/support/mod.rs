use std::env;
use std::sync::OnceLock;

use migration::MigratorTrait;
use sea_orm::{ConnectOptions, ConnectionTrait, Database, DatabaseConnection};

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

    eprintln!("db-tests: connected to {}", redact_url(&url));

    if MIGRATED.get().is_none() {
        migration::Migrator::up(&conn, None)
            .await
            .expect("failed to run migrations");
        let _ = MIGRATED.set(());
    }

    conn
}

pub async fn reset_db(conn: &DatabaseConnection) {
    // Full application-table truncate list, alphabetical. Kept in sync with
    // entity/src/*.rs (table_name = "..."); excludes seaql_migrations.
    // CASCADE ensures FK-dependent rows are cleared regardless of declaration order.
    let sql = r#"
TRUNCATE TABLE
  bootstrap_token,
  capability_registry,
  channel_lens,
  command_audit_log,
  conversation,
  conversation_escalation_policy,
  conversation_member,
  conversation_read_state,
  custom_emoji,
  device_key,
  device_link,
  escalation_policy,
  escalation_run,
  escalation_step,
  incident,
  incident_message_tag,
  incident_phase,
  instance_invite,
  invite,
  key_challenge,
  key_log_entry,
  keybind,
  link_preview,
  "macro",
  message,
  message_attachment,
  message_change_seq,
  message_mention,
  message_pin,
  message_reaction,
  message_report,
  message_thread,
  notification_preference,
  on_call_override,
  on_call_rotation_layer,
  on_call_rotation_participant,
  on_call_schedule,
  password_reset_token,
  pipeline,
  plugin_bundle,
  profile_card,
  server,
  server_ban,
  server_member,
  server_member_timeout,
  session,
  user_command,
  user_notification,
  user_notification_settings,
  user_skill_profile,
  "user",
  workspace_feature,
  workspace_ingest_token,
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
