//! `GET /healthz` and `GET /readyz`: liveness and readiness for orchestrators and load
//! balancers. Unauthenticated, outside CORS and CSRF; the answers carry no paths or errors,
//! the reasons a check failed go to the log. A readiness result is reused for one second, so
//! callers cannot drive the database checks at request rate.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use actix_web::{HttpResponse, web};
use async_trait::async_trait;
use migration::MigratorTrait;
use sea_orm::{ConnectionTrait, DatabaseConnection, DbBackend, Statement};
use serde_json::{Map, Value, json};
use tokio::sync::Mutex;
use tokio::time::Instant;

use crate::config::VERSION;

/// How long one readiness check may take before it counts as failed.
const CHECK_TIMEOUT: Duration = Duration::from_secs(3);

/// How long a readiness result is reused before the checks run again.
const READINESS_TTL: Duration = Duration::from_secs(1);

/// The pieces readiness depends on.
#[async_trait]
pub(crate) trait ReadinessProbe: Send + Sync {
    /// Whether the database answers `SELECT 1`.
    async fn database(&self) -> anyhow::Result<()>;
    /// The names of the migrations recorded as applied.
    async fn applied_migrations(&self) -> anyhow::Result<HashSet<String>>;
    /// The names of the migrations this build registers.
    fn registered_migrations(&self) -> Vec<String>;
    /// Whether the server key file can be opened for reading.
    async fn server_key_file(&self) -> anyhow::Result<()>;
    /// Whether the franking key file can be opened for reading.
    async fn franking_key_file(&self) -> anyhow::Result<()>;
}

/// The probe of a running server: its database connection and configured key files.
pub(crate) struct SystemProbe {
    pub(crate) db: DatabaseConnection,
    pub(crate) server_key_file: PathBuf,
    pub(crate) franking_key_file: PathBuf,
}

async fn readable_file(path: &PathBuf) -> anyhow::Result<()> {
    let metadata = tokio::fs::metadata(path).await?;
    anyhow::ensure!(metadata.is_file(), "not a regular file");
    tokio::fs::File::open(path).await?;
    Ok(())
}

#[async_trait]
impl ReadinessProbe for SystemProbe {
    async fn database(&self) -> anyhow::Result<()> {
        self.db.execute_unprepared("SELECT 1").await?;
        Ok(())
    }

    async fn applied_migrations(&self) -> anyhow::Result<HashSet<String>> {
        let rows = self
            .db
            .query_all_raw(Statement::from_string(
                DbBackend::Postgres,
                "SELECT version FROM seaql_migrations",
            ))
            .await?;
        rows.iter()
            .map(|row| Ok(row.try_get::<String>("", "version")?))
            .collect()
    }

    fn registered_migrations(&self) -> Vec<String> {
        migration::Migrator::migrations()
            .iter()
            .map(|m| m.name().to_string())
            .collect()
    }

    async fn server_key_file(&self) -> anyhow::Result<()> {
        readable_file(&self.server_key_file).await
    }

    async fn franking_key_file(&self) -> anyhow::Result<()> {
        readable_file(&self.franking_key_file).await
    }
}

/// The outcome of one readiness check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum CheckOutcome {
    Ok,
    /// Failed, with a reason safe to show to anyone.
    Fail(String),
}

/// Every readiness check by name, in a fixed order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Readiness {
    pub(crate) checks: Vec<(&'static str, CheckOutcome)>,
}

impl Readiness {
    pub(crate) fn is_ready(&self) -> bool {
        self.checks
            .iter()
            .all(|(_, outcome)| *outcome == CheckOutcome::Ok)
    }

    /// The `/readyz` body: `status`, `version` and each check.
    pub(crate) fn to_json(&self) -> Value {
        let checks: Map<String, Value> = self
            .checks
            .iter()
            .map(|(name, outcome)| {
                let value = match outcome {
                    CheckOutcome::Ok => json!({ "status": "ok" }),
                    CheckOutcome::Fail(reason) => json!({ "status": "fail", "reason": reason }),
                };
                ((*name).to_string(), value)
            })
            .collect();
        json!({
            "status": if self.is_ready() { "ok" } else { "fail" },
            "version": VERSION,
            "checks": checks,
        })
    }
}

/// Run `check` within [`CHECK_TIMEOUT`]; a failure is logged and reported as `reason`.
async fn bounded<F>(name: &'static str, reason: &str, check: F) -> CheckOutcome
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    match tokio::time::timeout(CHECK_TIMEOUT, check).await {
        Ok(Ok(())) => CheckOutcome::Ok,
        Ok(Err(e)) => {
            tracing::warn!(check = name, error = %format!("{e:#}"), "readiness check failed");
            CheckOutcome::Fail(reason.to_string())
        }
        Err(_) => {
            tracing::warn!(check = name, "readiness check timed out");
            CheckOutcome::Fail("timed out".to_string())
        }
    }
}

/// The registered migrations missing from `applied`. Applied migrations this build does not
/// know (a newer schema under an older binary) are not pending.
pub(crate) fn pending_migrations(registered: &[String], applied: &HashSet<String>) -> usize {
    registered
        .iter()
        .filter(|name| !applied.contains(*name))
        .count()
}

async fn migrations_check(probe: &dyn ReadinessProbe) -> CheckOutcome {
    let applied = match tokio::time::timeout(CHECK_TIMEOUT, probe.applied_migrations()).await {
        Ok(Ok(applied)) => applied,
        Ok(Err(e)) => {
            tracing::warn!(check = "migrations", error = %format!("{e:#}"), "readiness check failed");
            return CheckOutcome::Fail("migration table unreadable".to_string());
        }
        Err(_) => {
            tracing::warn!(check = "migrations", "readiness check timed out");
            return CheckOutcome::Fail("timed out".to_string());
        }
    };
    match pending_migrations(&probe.registered_migrations(), &applied) {
        0 => CheckOutcome::Ok,
        pending => {
            tracing::warn!(check = "migrations", pending, "migrations pending");
            CheckOutcome::Fail(format!("{pending} pending"))
        }
    }
}

/// Run every readiness check.
pub(crate) async fn check(probe: &dyn ReadinessProbe) -> Readiness {
    let (database, migrations, server_key, franking_key) = tokio::join!(
        bounded("database", "unreachable", probe.database()),
        migrations_check(probe),
        bounded("serverKey", "not readable", probe.server_key_file()),
        bounded("frankingKey", "not readable", probe.franking_key_file()),
    );
    Readiness {
        checks: vec![
            ("database", database),
            ("migrations", migrations),
            ("serverKey", server_key),
            ("frankingKey", franking_key),
        ],
    }
}

/// `GET /healthz`: 200 whenever the process serves requests.
pub(crate) async fn healthz() -> HttpResponse {
    HttpResponse::Ok()
        .insert_header(("cache-control", "no-store"))
        .json(json!({ "status": "ok", "version": VERSION }))
}

/// The readiness probe with its last result, shared by every worker.
pub(crate) struct ReadinessCache {
    probe: Arc<dyn ReadinessProbe>,
    ttl: Duration,
    last: Mutex<Option<(Instant, Readiness)>>,
}

impl ReadinessCache {
    pub(crate) fn new(probe: Arc<dyn ReadinessProbe>) -> Self {
        Self::with_ttl(probe, READINESS_TTL)
    }

    fn with_ttl(probe: Arc<dyn ReadinessProbe>, ttl: Duration) -> Self {
        Self {
            probe,
            ttl,
            last: Mutex::new(None),
        }
    }

    /// The last result while it is younger than the time to live, else a fresh one. Concurrent
    /// callers wait for the one check in flight instead of starting their own.
    pub(crate) async fn readiness(&self) -> Readiness {
        let mut last = self.last.lock().await;
        if let Some((at, readiness)) = last.as_ref()
            && at.elapsed() < self.ttl
        {
            return readiness.clone();
        }
        let readiness = check(self.probe.as_ref()).await;
        *last = Some((Instant::now(), readiness.clone()));
        readiness
    }
}

/// `GET /readyz`: 200 when every readiness check passes, else 503 naming the failing ones.
pub(crate) async fn readyz(cache: web::Data<ReadinessCache>) -> HttpResponse {
    let readiness = cache.readiness().await;
    let mut response = if readiness.is_ready() {
        HttpResponse::Ok()
    } else {
        HttpResponse::ServiceUnavailable()
    };
    response
        .insert_header(("cache-control", "no-store"))
        .json(readiness.to_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A probe whose every piece answers as configured.
    struct FakeProbe {
        database_calls: std::sync::atomic::AtomicUsize,
        database: bool,
        applied: Option<Vec<&'static str>>,
        registered: Vec<&'static str>,
        server_key: bool,
        franking_key: bool,
    }

    impl Default for FakeProbe {
        fn default() -> Self {
            Self {
                database_calls: std::sync::atomic::AtomicUsize::new(0),
                database: true,
                applied: Some(vec!["m1", "m2"]),
                registered: vec!["m1", "m2"],
                server_key: true,
                franking_key: true,
            }
        }
    }

    fn answer(ok: bool) -> anyhow::Result<()> {
        if ok {
            Ok(())
        } else {
            Err(anyhow::anyhow!("/secret/path: permission denied"))
        }
    }

    #[async_trait]
    impl ReadinessProbe for FakeProbe {
        async fn database(&self) -> anyhow::Result<()> {
            self.database_calls
                .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            answer(self.database)
        }

        async fn applied_migrations(&self) -> anyhow::Result<HashSet<String>> {
            self.applied
                .as_ref()
                .map(|names| names.iter().map(|n| n.to_string()).collect())
                .ok_or_else(|| anyhow::anyhow!("relation \"seaql_migrations\" does not exist"))
        }

        fn registered_migrations(&self) -> Vec<String> {
            self.registered.iter().map(|n| n.to_string()).collect()
        }

        async fn server_key_file(&self) -> anyhow::Result<()> {
            answer(self.server_key)
        }

        async fn franking_key_file(&self) -> anyhow::Result<()> {
            answer(self.franking_key)
        }
    }

    fn failing(readiness: &Readiness) -> Vec<&'static str> {
        readiness
            .checks
            .iter()
            .filter(|(_, outcome)| *outcome != CheckOutcome::Ok)
            .map(|(name, _)| *name)
            .collect()
    }

    #[tokio::test]
    async fn every_check_passing_is_ready() {
        let readiness = check(&FakeProbe::default()).await;
        assert!(readiness.is_ready());
        assert_eq!(
            readiness.to_json(),
            json!({
                "status": "ok",
                "version": VERSION,
                "checks": {
                    "database": { "status": "ok" },
                    "migrations": { "status": "ok" },
                    "serverKey": { "status": "ok" },
                    "frankingKey": { "status": "ok" },
                },
            })
        );
    }

    #[tokio::test]
    async fn an_unreachable_database_fails_database_and_migrations() {
        let probe = FakeProbe {
            database: false,
            applied: None,
            ..Default::default()
        };
        let readiness = check(&probe).await;
        assert!(!readiness.is_ready());
        assert_eq!(failing(&readiness), vec!["database", "migrations"]);
        let body = readiness.to_json();
        assert_eq!(body["status"], "fail");
        assert_eq!(body["checks"]["database"]["reason"], "unreachable");
        assert_eq!(
            body["checks"]["migrations"]["reason"],
            "migration table unreadable"
        );
    }

    #[tokio::test]
    async fn a_pending_migration_fails_migrations_with_the_count() {
        let probe = FakeProbe {
            registered: vec!["m1", "m2", "m3", "m4"],
            ..Default::default()
        };
        let readiness = check(&probe).await;
        assert_eq!(failing(&readiness), vec!["migrations"]);
        assert_eq!(
            readiness.to_json()["checks"]["migrations"]["reason"],
            "2 pending"
        );
    }

    #[tokio::test]
    async fn unreadable_key_files_fail_their_checks_without_the_error() {
        let probe = FakeProbe {
            server_key: false,
            franking_key: false,
            ..Default::default()
        };
        let readiness = check(&probe).await;
        assert_eq!(failing(&readiness), vec!["serverKey", "frankingKey"]);
        let body = readiness.to_json().to_string();
        assert!(body.contains("not readable"));
        assert!(!body.contains("/secret/path"));
    }

    #[test]
    fn migrations_unknown_to_the_build_are_not_pending() {
        let registered = vec!["m1".to_string(), "m2".to_string()];
        let applied: HashSet<String> = ["m1", "m2", "m3"].iter().map(|n| n.to_string()).collect();
        assert_eq!(pending_migrations(&registered, &applied), 0);
        let applied: HashSet<String> = ["m1"].iter().map(|n| n.to_string()).collect();
        assert_eq!(pending_migrations(&registered, &applied), 1);
    }

    #[tokio::test]
    async fn readable_file_accepts_a_file_and_refuses_a_directory_or_missing_path() {
        let dir = std::env::temp_dir().join(format!("rim-readyz-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir_all(&dir).expect("temp dir");
        let file = dir.join("key.json");
        std::fs::write(&file, "{}").expect("write");
        assert!(readable_file(&file).await.is_ok());
        assert!(readable_file(&dir).await.is_err());
        assert!(readable_file(&dir.join("missing.json")).await.is_err());
        std::fs::remove_dir_all(&dir).expect("cleanup");
    }

    #[actix_web::test]
    async fn readyz_answers_503_when_a_check_fails() {
        let ready = ReadinessCache::new(Arc::new(FakeProbe::default()));
        let response = readyz(web::Data::new(ready)).await;
        assert_eq!(response.status(), actix_web::http::StatusCode::OK);
        let unready = ReadinessCache::new(Arc::new(FakeProbe {
            franking_key: false,
            ..Default::default()
        }));
        let response = readyz(web::Data::new(unready)).await;
        assert_eq!(
            response.status(),
            actix_web::http::StatusCode::SERVICE_UNAVAILABLE
        );
    }

    #[actix_web::test]
    async fn readiness_within_the_time_to_live_runs_the_checks_once() {
        let probe = Arc::new(FakeProbe::default());
        let cache = web::Data::new(ReadinessCache::new(probe.clone()));
        let first = readyz(cache.clone()).await;
        let second = readyz(cache).await;
        assert_eq!(first.status(), actix_web::http::StatusCode::OK);
        assert_eq!(second.status(), actix_web::http::StatusCode::OK);
        assert_eq!(
            probe
                .database_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            1
        );
    }

    #[tokio::test]
    async fn an_expired_readiness_result_runs_the_checks_again() {
        let probe = Arc::new(FakeProbe::default());
        let cache = ReadinessCache::with_ttl(probe.clone(), Duration::ZERO);
        cache.readiness().await;
        cache.readiness().await;
        assert_eq!(
            probe
                .database_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            2
        );
    }
}
