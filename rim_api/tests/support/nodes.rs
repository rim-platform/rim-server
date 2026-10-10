//! Real RIM servers in one process for server-to-server tests: each listens on loopback with
//! its own database (`DATABASE_URL`, and the same name with `b` appended, created when
//! missing), `RIM_S2S_ALLOW_HTTP` and `RIM_S2S_ALLOW_PRIVATE_ADDRESSES`. Each server's pool is
//! capped and closed when its test ends, so every two-server test of a binary fits in
//! Postgres's connections.

use std::net::TcpListener;
use std::sync::Arc;
use std::time::Duration;

use migration::MigratorTrait;
use rim_api::AppSchema;
use rim_s2s::ServerIdentity;
use rim_server::app::S2sRuntime;
use rim_server::config::RIMConfig;
use sea_orm::{
    ConnectOptions, ConnectionTrait, Database, DatabaseConnection, DbBackend, Statement,
};

/// S2S tables, cleared before each test so every server boots a fresh identity.
pub const S2S_TABLES: &str = "TRUNCATE TABLE server_key_log_entry, server_descriptor, peer_key_log_entry, peer_server, \
     s2s_nonce";

/// Connections each test server's pool opens at most.
pub const SERVER_POOL: u32 = 5;
/// Connections a test's own handle on a server's database opens at most.
const TEST_POOL: u32 = 2;
/// How long closing a pool may wait for its connections to come back.
const CLOSE_TIMEOUT: Duration = Duration::from_secs(10);

/// A running server: its origin, data directory, database and S2S handles. [`Node::stop`]
/// ends it; dropping it unstopped, as a panicking test does, ends it too on a multi-threaded
/// runtime.
pub struct Node {
    pub origin: String,
    pub data: tempfile::TempDir,
    pub db: DatabaseConnection,
    pub s2s: S2sRuntime,
    pub schema: AppSchema,
    server_db: DatabaseConnection,
    handle: actix_web::dev::ServerHandle,
    shutdown: tokio::sync::watch::Sender<bool>,
    stopped: bool,
}

impl Node {
    pub fn identity(&self) -> Arc<ServerIdentity> {
        self.s2s.state.identity.current()
    }

    pub fn id(&self) -> String {
        self.identity().server_id().to_string()
    }

    /// Stop the server, end its background tasks and close the server's pool and the test's
    /// own, so their connections are free for the next test.
    pub async fn stop(mut self) {
        teardown(
            self.handle.clone(),
            &self.shutdown,
            [self.server_db.clone(), self.db.clone()],
        )
        .await;
        self.stopped = true;
    }
}

impl Drop for Node {
    fn drop(&mut self) {
        if self.stopped {
            return;
        }
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            let _ = self.shutdown.send(true);
            return;
        };
        if runtime.runtime_flavor() != tokio::runtime::RuntimeFlavor::MultiThread {
            let _ = self.shutdown.send(true);
            return;
        }
        let handle = self.handle.clone();
        let pools = [self.server_db.clone(), self.db.clone()];
        let shutdown = &self.shutdown;
        tokio::task::block_in_place(|| runtime.block_on(teardown(handle, shutdown, pools)));
    }
}

/// Stop the HTTP server, signal the background tasks and close `pools`.
async fn teardown(
    handle: actix_web::dev::ServerHandle,
    shutdown: &tokio::sync::watch::Sender<bool>,
    pools: [DatabaseConnection; 2],
) {
    handle.stop(false).await;
    let _ = shutdown.send(true);
    for pool in &pools {
        if tokio::time::timeout(CLOSE_TIMEOUT, pool.close_by_ref())
            .await
            .is_err()
        {
            eprintln!("a test server's pool did not close within {CLOSE_TIMEOUT:?}");
        }
    }
}

/// A pool of at most `max` connections to `url`.
async fn pool(url: &str, max: u32) -> DatabaseConnection {
    let mut options = ConnectOptions::new(url.to_string());
    options.max_connections(max).sqlx_logging(false);
    Database::connect(options).await.expect("db")
}

/// The second test database: the first one's name with `b` appended, created when missing.
pub async fn second_database(first: &DatabaseConnection) -> String {
    let url = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let (base, query) = match url.split_once('?') {
        Some((base, query)) => (base.to_string(), format!("?{query}")),
        None => (url.clone(), String::new()),
    };
    let (prefix, name) = base.rsplit_once('/').expect("database name");
    let second = format!("{name}b");
    let exists = first
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT 1 AS one FROM pg_database WHERE datname = $1",
            [second.clone().into()],
        ))
        .await
        .expect("pg_database")
        .is_some();
    if !exists {
        first
            .execute_unprepared(&format!("CREATE DATABASE \"{second}\""))
            .await
            .expect("create the second test database");
    }
    format!("{prefix}/{second}{query}")
}

/// Both test databases, migrated, with `truncate` run on each.
pub async fn databases_with(truncate: &str) -> (String, String) {
    let ctx = super::TestCtx::new().await;
    ctx.conn
        .execute_unprepared(truncate)
        .await
        .expect("truncate");
    let first = std::env::var("DATABASE_URL").expect("DATABASE_URL");
    let second = second_database(&ctx.conn).await;
    let conn = pool(&second, 1).await;
    migration::Migrator::up(&conn, None).await.expect("migrate");
    conn.execute_unprepared(truncate).await.expect("truncate");
    conn.close().await.expect("close");
    ctx.conn.close().await.expect("close");
    (first, second)
}

pub async fn start(database_url: &str, tweak: impl FnOnce(&mut RIMConfig)) -> Node {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().expect("addr").port();
    let origin = format!("http://127.0.0.1:{port}");
    let data = tempfile::tempdir().expect("data dir");
    let mut config = RIMConfig {
        server_host: "127.0.0.1".into(),
        server_port: port,
        database_url: database_url.into(),
        data_dir: data.path().to_string_lossy().into_owned(),
        public_base_url: Some(origin.clone()),
        s2s_allow_http: Some(true),
        s2s_allow_private_addresses: Some(true),
        database_max_connections: Some(SERVER_POOL),
        ..Default::default()
    };
    tweak(&mut config);
    let running = rim_server::server::serve(&config, listener)
        .await
        .expect("serve");
    let handle = running.server.handle();
    tokio::spawn(running.server);
    Node {
        origin,
        data,
        db: pool(database_url, TEST_POOL).await,
        s2s: running.s2s,
        schema: running.schema,
        server_db: running.db,
        handle,
        shutdown: running.shutdown_tx,
        stopped: false,
    }
}

/// Both test databases, migrated, with their S2S tables empty.
pub async fn databases() -> (String, String) {
    databases_with(S2S_TABLES).await
}
