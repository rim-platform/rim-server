//! The command audit log retention purge. Rows older than the retention are first written as
//! gzipped JSON Lines to `<data dir>/audit-exports/`, the file is synced, and only then are the
//! exported rows deleted, a bounded batch per statement.

use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use flate2::Compression;
use flate2::write::GzEncoder;
use serde::Serialize;
use tokio::sync::watch;
use uuid::Uuid;

use rim_core::audit::repository::{AuditLogEntryRecord, AuditLogRepository};
use rim_core::repository::error::RepoError;

/// Fewest days `RIM_AUDIT_RETENTION_DAYS` accepts.
pub const MIN_RETENTION_DAYS: u32 = 1;
/// Most days `RIM_AUDIT_RETENTION_DAYS` accepts.
pub const MAX_RETENTION_DAYS: u32 = 3650;
/// Rows read per export page and deleted per statement.
pub const BATCH_SIZE: u32 = 5000;
/// Directory under the data directory that holds the exports.
pub const EXPORT_DIR_NAME: &str = "audit-exports";

/// Wait after boot before the first scheduled purge.
const FIRST_RUN_DELAY: Duration = Duration::from_secs(60);
/// Time between scheduled purges.
const RUN_INTERVAL: Duration = Duration::from_secs(24 * 60 * 60);
/// Timestamp format of the export file names.
const NAME_TIME_FORMAT: &str = "%Y%m%dT%H%M%SZ";

/// The export directory under `data_dir`.
pub fn export_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(EXPORT_DIR_NAME)
}

/// The cutoff for a retention of `days`: rows created before it are purged.
pub fn cutoff(now: DateTime<Utc>, days: u32) -> DateTime<Utc> {
    now - chrono::Duration::days(i64::from(days))
}

/// Why a purge stopped.
#[derive(Debug, thiserror::Error)]
pub enum PurgeError {
    /// The rows could not be read for the export; nothing was deleted.
    #[error("could not read the audit rows to export, nothing was deleted: {0}")]
    Read(#[source] RepoError),
    /// The export file could not be written or synced; nothing was deleted.
    #[error("could not write the audit export, nothing was deleted: {0}")]
    Export(#[source] io::Error),
    /// A delete statement failed after the export was written; the report's rows up to that
    /// batch are gone, the rest stay for the next run.
    #[error("could not delete the exported audit rows: {0}")]
    Delete(#[source] RepoError),
}

/// What one purge did.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PurgeReport {
    pub cutoff: DateTime<Utc>,
    pub exported: u64,
    pub deleted: u64,
    /// `created_at` of the oldest and newest exported row.
    pub oldest: Option<DateTime<Utc>>,
    pub newest: Option<DateTime<Utc>>,
    /// The export file, `None` when no row was older than the cutoff.
    pub export_path: Option<PathBuf>,
}

/// One exported audit row: the fields the admin audit view shows, with the parent macro row.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportLine<'a> {
    id: String,
    actor_user_id: String,
    server_id: String,
    command_id: &'a str,
    initiator: &'a str,
    args_redacted: &'a serde_json::Value,
    sensitivity: &'a str,
    dangerous: bool,
    confirmation_kind: &'a str,
    typed_verb: Option<&'a str>,
    outcome: &'a str,
    error_kind: Option<&'a str>,
    capability_check_outcome: Option<&'a str>,
    confirmation_outcome: Option<&'a str>,
    parent_audit_id: Option<String>,
    created_at: String,
}

impl<'a> From<&'a AuditLogEntryRecord> for ExportLine<'a> {
    fn from(r: &'a AuditLogEntryRecord) -> Self {
        Self {
            id: r.id.to_string(),
            actor_user_id: r.actor_user_id.to_string(),
            server_id: r.server_id.to_string(),
            command_id: &r.command_id,
            initiator: &r.initiator,
            args_redacted: &r.args_redacted,
            sensitivity: &r.sensitivity,
            dangerous: r.dangerous,
            confirmation_kind: &r.confirmation_kind,
            typed_verb: r.typed_verb.as_deref(),
            outcome: &r.outcome,
            error_kind: r.error_kind.as_deref(),
            capability_check_outcome: r.capability_check_outcome.as_deref(),
            confirmation_outcome: r.confirmation_outcome.as_deref(),
            parent_audit_id: r.parent_audit_id.map(|id| id.to_string()),
            created_at: r.created_at.to_rfc3339(),
        }
    }
}

/// A gzipped JSON Lines export being written to a hidden partial file in the export
/// directory. [`ExportWriter::finish`] syncs it and renames it to its final name; dropped
/// unfinished, the partial file is removed.
pub(crate) struct ExportWriter {
    dir: PathBuf,
    partial: PathBuf,
    encoder: Option<GzEncoder<BufWriter<File>>>,
    lines: u64,
}

impl ExportWriter {
    /// Create the export directory when missing and open a new partial file in it.
    pub(crate) fn create(dir: &Path) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let partial = dir.join(format!(".audit-export-{}.partial", Uuid::now_v7()));
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        Ok(Self {
            dir: dir.to_path_buf(),
            partial,
            encoder: Some(GzEncoder::new(BufWriter::new(file), Compression::default())),
            lines: 0,
        })
    }

    /// Append one line per row.
    pub(crate) fn write_rows(&mut self, rows: &[AuditLogEntryRecord]) -> io::Result<()> {
        let encoder = self
            .encoder
            .as_mut()
            .ok_or_else(|| io::Error::other("the export is already finished"))?;
        for row in rows {
            serde_json::to_writer(&mut *encoder, &ExportLine::from(row))?;
            encoder.write_all(b"\n")?;
            self.lines += 1;
        }
        Ok(())
    }

    /// Finish the gzip stream, sync the file, and rename it to
    /// `audit-<oldest>-<newest>.jsonl.gz` (with a `-N` suffix when that name is taken), then
    /// sync the directory. Returns the final path.
    pub(crate) fn finish(
        mut self,
        oldest: DateTime<Utc>,
        newest: DateTime<Utc>,
    ) -> io::Result<PathBuf> {
        let encoder = self
            .encoder
            .take()
            .ok_or_else(|| io::Error::other("the export is already finished"))?;
        let file = encoder.finish()?.into_inner().map_err(|e| e.into_error())?;
        file.sync_all()?;
        drop(file);
        let target = free_name(&self.dir, oldest, newest)?;
        fs::rename(&self.partial, &target)?;
        sync_dir(&self.dir)?;
        self.partial = PathBuf::new();
        Ok(target)
    }
}

impl Drop for ExportWriter {
    fn drop(&mut self) {
        if !self.partial.as_os_str().is_empty() {
            let _ = fs::remove_file(&self.partial);
        }
    }
}

/// The first unused export name for the range.
fn free_name(dir: &Path, oldest: DateTime<Utc>, newest: DateTime<Utc>) -> io::Result<PathBuf> {
    let stem = format!(
        "audit-{}-{}",
        oldest.format(NAME_TIME_FORMAT),
        newest.format(NAME_TIME_FORMAT)
    );
    let first = dir.join(format!("{stem}.jsonl.gz"));
    if !first.exists() {
        return Ok(first);
    }
    (2..1000)
        .map(|n| dir.join(format!("{stem}-{n}.jsonl.gz")))
        .find(|p| !p.exists())
        .ok_or_else(|| io::Error::other(format!("no free export name for {stem}")))
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

/// Run blocking file work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> io::Result<T> + Send + 'static,
) -> io::Result<T> {
    tokio::task::spawn_blocking(work)
        .await
        .map_err(io::Error::other)?
}

/// The exported rows: where they went and the key of the last one.
struct Exported {
    path: PathBuf,
    lines: u64,
    oldest: DateTime<Utc>,
    newest: DateTime<Utc>,
    last_key: (DateTime<Utc>, Uuid),
}

/// Write every row older than `cutoff` to a new export file, `batch` rows per read. `None`
/// when there is no such row.
async fn export(
    repo: &dyn AuditLogRepository,
    cutoff: DateTime<Utc>,
    dir: &Path,
    batch: u32,
) -> Result<Option<Exported>, PurgeError> {
    let batch = batch.max(1);
    let mut rows = repo
        .list_older_than(cutoff, None, batch)
        .await
        .map_err(PurgeError::Read)?;
    let Some(first) = rows.first() else {
        return Ok(None);
    };
    let oldest = first.created_at;
    let dir = dir.to_path_buf();
    let mut writer = blocking(move || ExportWriter::create(&dir))
        .await
        .map_err(PurgeError::Export)?;
    let mut last_key = (oldest, first.id);
    while let Some(last) = rows.last() {
        last_key = (last.created_at, last.id);
        let full = rows.len() >= batch as usize;
        writer = blocking(move || writer.write_rows(&rows).map(|()| writer))
            .await
            .map_err(PurgeError::Export)?;
        if !full {
            break;
        }
        rows = repo
            .list_older_than(cutoff, Some(last_key), batch)
            .await
            .map_err(PurgeError::Read)?;
    }
    let lines = writer.lines;
    let newest = last_key.0;
    let path = blocking(move || writer.finish(oldest, newest))
        .await
        .map_err(PurgeError::Export)?;
    Ok(Some(Exported {
        path,
        lines,
        oldest,
        newest,
        last_key,
    }))
}

/// Export the rows older than `cutoff` to `export_dir`, then delete exactly the exported
/// rows, `batch` per statement. A failed export deletes nothing.
///
/// # Errors
/// A [`PurgeError`] naming the step that failed.
pub async fn purge(
    repo: &dyn AuditLogRepository,
    cutoff: DateTime<Utc>,
    export_dir: &Path,
    batch: u32,
) -> Result<PurgeReport, PurgeError> {
    let batch = batch.max(1);
    let Some(exported) = export(repo, cutoff, export_dir, batch).await? else {
        return Ok(PurgeReport {
            cutoff,
            exported: 0,
            deleted: 0,
            oldest: None,
            newest: None,
            export_path: None,
        });
    };
    let mut deleted = 0;
    loop {
        let n = repo
            .delete_older_than_batch(cutoff, exported.last_key, batch)
            .await
            .map_err(PurgeError::Delete)?;
        crate::metrics::audit_rows_purged(n);
        deleted += n;
        if n < u64::from(batch) {
            break;
        }
    }
    Ok(PurgeReport {
        cutoff,
        exported: exported.lines,
        deleted,
        oldest: Some(exported.oldest),
        newest: Some(exported.newest),
        export_path: Some(exported.path),
    })
}

/// Run one purge and put it on record: an `audit.retention.purged` audit event with the counts
/// and the export path, or a warning when the purge stopped. `trigger` names who ran it
/// (`job` or `cli`).
///
/// # Errors
/// What [`purge`] failed with.
pub async fn run(
    repo: &dyn AuditLogRepository,
    cutoff: DateTime<Utc>,
    export_dir: &Path,
    trigger: &'static str,
) -> Result<PurgeReport, PurgeError> {
    match purge(repo, cutoff, export_dir, BATCH_SIZE).await {
        Ok(report) => {
            let export_path = report
                .export_path
                .as_deref()
                .map(|p| p.display().to_string())
                .unwrap_or_default();
            tracing::info!(
                target: "audit",
                event = "audit.retention.purged",
                trigger,
                cutoff = %report.cutoff.to_rfc3339(),
                exported = report.exported,
                deleted = report.deleted,
                export_path = %export_path,
                "audit rows past retention purged"
            );
            Ok(report)
        }
        Err(e) => {
            tracing::warn!(
                target: "audit",
                event = "audit.retention.failed",
                trigger,
                cutoff = %cutoff.to_rfc3339(),
                error = %e,
                "audit retention purge stopped"
            );
            Err(e)
        }
    }
}

/// Purge rows older than `retention_days` a minute after boot and then once a day, until
/// `shutdown` changes or its sender is dropped.
pub async fn run_daily(
    repo: Arc<dyn AuditLogRepository>,
    retention_days: u32,
    export_dir: PathBuf,
    mut shutdown: watch::Receiver<bool>,
) {
    let mut tick =
        tokio::time::interval_at(tokio::time::Instant::now() + FIRST_RUN_DELAY, RUN_INTERVAL);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let cutoff = cutoff(Utc::now(), retention_days);
                // `run` records the outcome, including a failure, on the audit target.
                let _ = run(repo.as_ref(), cutoff, &export_dir, "job").await;
            }
            _ = shutdown.changed() => break,
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::{BufRead, BufReader};
    use std::sync::Mutex;

    use flate2::read::GzDecoder;
    use rim_core::audit::repository::MockAuditLogRepository;
    use rim_core::repository::error::RepoErrorKind;

    use super::*;
    use crate::metrics::test_support::{block_on, recorded};

    fn row(minutes_ago: i64, now: DateTime<Utc>) -> AuditLogEntryRecord {
        AuditLogEntryRecord {
            id: Uuid::now_v7(),
            actor_user_id: Uuid::now_v7(),
            server_id: Uuid::now_v7(),
            command_id: "/ping".to_string(),
            initiator: "palette".to_string(),
            args_redacted: serde_json::json!({"text": "[redacted]"}),
            sensitivity: "low".to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: Some("read".to_string()),
            outcome: "success".to_string(),
            error_kind: None,
            capability_check_outcome: Some("allowed".to_string()),
            confirmation_outcome: None,
            parent_audit_id: None,
            created_at: now - chrono::Duration::minutes(minutes_ago),
        }
    }

    fn read_lines(path: &Path) -> Vec<serde_json::Value> {
        let file = File::open(path).expect("open export");
        BufReader::new(GzDecoder::new(file))
            .lines()
            .map(|l| serde_json::from_str(&l.expect("line")).expect("json"))
            .collect()
    }

    fn files_in(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .map(|entries| {
                entries
                    .filter_map(Result::ok)
                    .map(|e| e.file_name().to_string_lossy().into_owned())
                    .collect()
            })
            .unwrap_or_default();
        names.sort();
        names
    }

    /// A mock repository holding `rows` (oldest first) that serves pages the way the database
    /// does and deletes up to the bound it is given.
    fn repo_with(rows: Vec<AuditLogEntryRecord>) -> (MockAuditLogRepository, Arc<Mutex<Vec<u32>>>) {
        let store = Arc::new(Mutex::new(rows));
        let deletes = Arc::new(Mutex::new(Vec::new()));
        let mut repo = MockAuditLogRepository::new();
        let pages = Arc::clone(&store);
        repo.expect_list_older_than()
            .returning(move |cutoff, after, limit| {
                let rows = pages.lock().expect("lock");
                Ok(rows
                    .iter()
                    .filter(|r| r.created_at < cutoff)
                    .filter(|r| after.is_none_or(|key| (r.created_at, r.id) > key))
                    .take(limit as usize)
                    .cloned()
                    .collect())
            });
        let gone = Arc::clone(&store);
        let seen = Arc::clone(&deletes);
        repo.expect_delete_older_than_batch()
            .returning(move |cutoff, up_to, limit| {
                let mut rows = gone.lock().expect("lock");
                let mut n = 0u32;
                rows.retain(|r| {
                    let hit = n < limit && r.created_at < cutoff && (r.created_at, r.id) <= up_to;
                    if hit {
                        n += 1;
                    }
                    !hit
                });
                seen.lock().expect("lock").push(n);
                Ok(u64::from(n))
            });
        (repo, deletes)
    }

    #[test]
    fn the_writer_writes_one_json_line_per_row_and_renames_on_finish() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let mut parent = row(10, now);
        let mut child = row(9, now);
        child.parent_audit_id = Some(parent.id);
        parent.typed_verb = None;

        let mut writer = ExportWriter::create(dir.path()).expect("create");
        writer
            .write_rows(&[parent.clone(), child.clone()])
            .expect("write");
        let path = writer
            .finish(parent.created_at, child.created_at)
            .expect("finish");

        let name = path
            .file_name()
            .expect("name")
            .to_string_lossy()
            .into_owned();
        assert!(
            name.starts_with("audit-") && name.ends_with(".jsonl.gz"),
            "{name}"
        );
        assert_eq!(files_in(dir.path()), vec![name]);
        let lines = read_lines(&path);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[0]["id"], parent.id.to_string());
        assert_eq!(lines[0]["typedVerb"], serde_json::Value::Null);
        assert_eq!(lines[0]["argsRedacted"]["text"], "[redacted]");
        assert_eq!(lines[1]["parentAuditId"], parent.id.to_string());
        assert_eq!(lines[1]["capabilityCheckOutcome"], "allowed");
        assert_eq!(lines[1]["createdAt"], child.created_at.to_rfc3339());
    }

    #[test]
    fn an_unfinished_writer_leaves_no_file_and_a_taken_name_gets_a_suffix() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let r = row(5, now);
        {
            let mut writer = ExportWriter::create(dir.path()).expect("create");
            writer.write_rows(std::slice::from_ref(&r)).expect("write");
        }
        assert!(files_in(dir.path()).is_empty());

        let first = ExportWriter::create(dir.path())
            .expect("create")
            .finish(r.created_at, r.created_at)
            .expect("finish");
        let second = ExportWriter::create(dir.path())
            .expect("create")
            .finish(r.created_at, r.created_at)
            .expect("finish");
        assert_ne!(first, second);
        assert!(
            second.to_string_lossy().ends_with("-2.jsonl.gz"),
            "{second:?}"
        );
    }

    #[test]
    fn the_purge_exports_then_deletes_in_batches() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let cutoff = now - chrono::Duration::minutes(10);
        // Seven old rows and two inside the retention, oldest first.
        let mut rows: Vec<_> = (0..7).map(|i| row(100 - i, now)).collect();
        rows.push(row(5, now));
        rows.push(row(1, now));
        let old_ids: Vec<String> = rows[..7].iter().map(|r| r.id.to_string()).collect();
        let (repo, deletes) = repo_with(rows.clone());

        let (report, got) = recorded(|| block_on(purge(&repo, cutoff, dir.path(), 3)));
        let report = report.expect("purge");

        assert_eq!(report.exported, 7);
        assert_eq!(report.deleted, 7);
        assert_eq!(report.oldest, Some(rows[0].created_at));
        assert_eq!(report.newest, Some(rows[6].created_at));
        assert_eq!(*deletes.lock().expect("lock"), vec![3, 3, 1]);
        assert_eq!(got.counter(crate::metrics::AUDIT_ROWS_PURGED, &[]), 7);
        let path = report.export_path.expect("an export");
        let exported: Vec<String> = read_lines(&path)
            .iter()
            .map(|l| l["id"].as_str().expect("id").to_string())
            .collect();
        assert_eq!(exported, old_ids);
    }

    #[test]
    fn a_batch_that_divides_the_rows_evenly_ends_on_an_empty_delete() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let rows: Vec<_> = (0..4).map(|i| row(100 - i, now)).collect();
        let (repo, deletes) = repo_with(rows);

        let report = block_on(purge(&repo, now, dir.path(), 2)).expect("purge");

        assert_eq!((report.exported, report.deleted), (4, 4));
        assert_eq!(*deletes.lock().expect("lock"), vec![2, 2, 0]);
    }

    #[test]
    fn nothing_older_than_the_cutoff_writes_no_file() {
        let dir = tempfile::tempdir().expect("tempdir");
        let now = Utc::now();
        let mut repo = MockAuditLogRepository::new();
        repo.expect_list_older_than()
            .returning(|_, _, _| Ok(vec![]));
        repo.expect_delete_older_than_batch().never();

        let report = block_on(purge(
            &repo,
            now - chrono::Duration::days(1),
            dir.path(),
            10,
        ))
        .expect("ok");

        assert_eq!((report.exported, report.deleted), (0, 0));
        assert_eq!(report.export_path, None);
        assert!(!export_dir(dir.path()).exists());
        assert!(files_in(dir.path()).is_empty());
    }

    #[test]
    fn a_failed_read_or_export_deletes_nothing() {
        let now = Utc::now();
        let dir = tempfile::tempdir().expect("tempdir");
        let mut repo = MockAuditLogRepository::new();
        let mut calls = 0;
        let first = row(100, now);
        repo.expect_list_older_than().returning(move |_, _, _| {
            calls += 1;
            if calls == 1 {
                Ok(vec![first.clone()])
            } else {
                Err(RepoError::with_message(RepoErrorKind::Internal, "down"))
            }
        });
        repo.expect_delete_older_than_batch().never();
        let err = block_on(purge(&repo, now, dir.path(), 1)).expect_err("read failed");
        assert!(matches!(err, PurgeError::Read(_)), "{err}");
        assert!(
            files_in(dir.path()).is_empty(),
            "the partial file is removed"
        );

        // The export directory's place is taken by a file, so the export cannot be created.
        let blocked = tempfile::NamedTempFile::new().expect("file");
        let (repo, deletes) = repo_with(vec![row(100, now)]);
        let err = block_on(purge(&repo, now, blocked.path(), 10)).expect_err("export failed");
        assert!(matches!(err, PurgeError::Export(_)), "{err}");
        assert!(deletes.lock().expect("lock").is_empty());
    }

    #[test]
    fn the_cutoff_is_the_retention_before_now() {
        let now = Utc::now();
        assert_eq!(cutoff(now, 90), now - chrono::Duration::days(90));
        assert_eq!(
            export_dir(Path::new("/var/lib/rim")),
            PathBuf::from("/var/lib/rim/audit-exports")
        );
    }
}
