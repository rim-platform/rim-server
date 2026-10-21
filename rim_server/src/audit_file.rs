//! Retention of the daily audit files: the files whose day lies more than the retention back
//! are deleted a minute after boot and then once a day.

use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{Days, NaiveDate, Utc};
use tokio::sync::watch;

/// Days the audit files are kept when `RIM_AUDIT_LOG_RETENTION_DAYS` is unset.
pub(crate) const DEFAULT_RETENTION_DAYS: u32 = 90;
/// The shortest retention accepted.
pub(crate) const MIN_RETENTION_DAYS: u32 = 1;
/// The longest retention accepted.
pub(crate) const MAX_RETENTION_DAYS: u32 = 3650;
/// Delay before the first run, so it stays out of the way of a starting server.
const FIRST_RUN_DELAY: Duration = Duration::from_secs(60);
/// Interval between runs.
const RUN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);

/// Delete the daily audit files in `dir` whose day is more than `retention_days` before
/// `today`. Other files are never touched. A file that cannot be deleted is logged and skipped.
/// Returns the deleted paths.
///
/// # Errors
/// When `dir` cannot be read.
pub(crate) fn prune(
    dir: &Path,
    retention_days: u32,
    today: NaiveDate,
) -> std::io::Result<Vec<PathBuf>> {
    let Some(oldest_kept) = today.checked_sub_days(Days::new(u64::from(retention_days))) else {
        return Ok(Vec::new());
    };
    let mut deleted = Vec::new();
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(day) = name
            .to_str()
            .and_then(rim_logging::audit_file_day)
            .and_then(|day| NaiveDate::parse_from_str(day, "%Y-%m-%d").ok())
        else {
            continue;
        };
        if day >= oldest_kept || !entry.file_type().is_ok_and(|t| t.is_file()) {
            continue;
        }
        let path = entry.path();
        match std::fs::remove_file(&path) {
            Ok(()) => deleted.push(path),
            Err(error) => {
                tracing::warn!(path = %path.display(), %error, "could not delete an old audit file");
            }
        }
    }
    deleted.sort();
    Ok(deleted)
}

/// Run [`prune`] on `dir` a minute after boot and then once a day, until `shutdown` changes or
/// its sender is dropped. Logs how many files each run deleted, and a failed run.
pub(crate) async fn run_daily(
    dir: PathBuf,
    retention_days: u32,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + FIRST_RUN_DELAY, RUN_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => match prune(&dir, retention_days, Utc::now().date_naive()) {
                Ok(deleted) if deleted.is_empty() => {}
                Ok(deleted) => tracing::info!(
                    count = deleted.len(),
                    retention_days,
                    "deleted audit files past their retention"
                ),
                Err(error) => tracing::warn!(
                    dir = %dir.display(),
                    %error,
                    "could not read the audit file directory"
                ),
            },
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty directory under the system's temporary directory.
    fn temp_dir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("rim-audit-file-{}", uuid::Uuid::now_v7()));
        std::fs::create_dir(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn prune_deletes_the_daily_files_past_the_retention_and_nothing_else() {
        let dir = temp_dir();
        for name in [
            "audit.2026-07-22.log",
            "audit.2026-07-23.log",
            "audit.2026-10-21.log",
            "audit.2025-01-01.log",
            "audit.log",
            "audit.2026-01-01.log.gz",
            "other.2026-01-01.log",
        ] {
            std::fs::write(dir.join(name), "{}\n").expect("write");
        }
        std::fs::create_dir(dir.join("audit.2026-01-02.log")).expect("mkdir");

        let today = NaiveDate::from_ymd_opt(2026, 10, 21).expect("date");
        let deleted = prune(&dir, 90, today).expect("prune");

        assert_eq!(
            deleted,
            vec![
                dir.join("audit.2025-01-01.log"),
                dir.join("audit.2026-07-22.log")
            ]
        );
        let mut left: Vec<String> = std::fs::read_dir(&dir)
            .expect("read dir")
            .map(|e| e.expect("entry").file_name().into_string().expect("utf8"))
            .collect();
        left.sort();
        assert_eq!(
            left,
            [
                "audit.2026-01-01.log.gz",
                "audit.2026-01-02.log",
                "audit.2026-07-23.log",
                "audit.2026-10-21.log",
                "audit.log",
                "other.2026-01-01.log",
            ]
        );
        std::fs::remove_dir_all(&dir).expect("clean up");
    }

    #[test]
    fn prune_fails_on_a_missing_directory() {
        let dir = std::env::temp_dir().join(format!("rim-audit-missing-{}", uuid::Uuid::now_v7()));
        let today = NaiveDate::from_ymd_opt(2026, 10, 21).expect("date");
        assert!(prune(&dir, 90, today).is_err());
    }

    #[tokio::test]
    async fn the_job_stops_on_shutdown_before_its_first_run() {
        let (tx, rx) = watch::channel(false);
        let task = tokio::spawn(run_daily(PathBuf::from("/nonexistent"), 90, rx));
        tx.send(true).expect("send");
        tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("stopped")
            .expect("joined");
    }
}
