use dotenvy::dotenv;
use std::error::Error as StdError;
use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::Registry;
use tracing_subscriber::filter::filter_fn;
use tracing_subscriber::fmt::MakeWriter;
use tracing_subscriber::fmt::time::UtcTime;
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::{EnvFilter, Layer};

/// Tracing target reserved for audit-grade events (Phase 4 P4-A1, design
/// §0.5). Every `execute_command` invocation emits exactly one line at this
/// target; the audit layer routes those lines into a dedicated `audit.log`
/// file in addition to the general stdout sink. Operators tail and ship
/// this file separately from general logs (different retention rules per
/// design §5).
pub const AUDIT_TARGET: &str = "audit";

/// Default file name for the audit log when `RIM_AUDIT_LOG_PATH` is unset.
/// Lives in the cwd so dev builds get a visible local file; production
/// deployments override via env to a logrotate-managed directory.
const DEFAULT_AUDIT_LOG_PATH: &str = "audit.log";

/// Env var that overrides the audit log path. Accepts either a directory
/// (file is named `audit.log` inside) or a full file path; we always pass
/// a (directory, file_name) tuple to `tracing_appender` so the override is
/// directory-only — full-path is interpreted as the parent dir's
/// `audit.log` to keep the convention stable.
const AUDIT_LOG_PATH_ENV: &str = "RIM_AUDIT_LOG_PATH";

/// Held-alive token returned by [`init_logging`]. The audit appender uses
/// `tracing_appender::non_blocking`, which spawns a background flusher and
/// returns a `WorkerGuard`. **The guard must outlive every `info!` emit on
/// the audit target** — dropping it shuts the worker down and pending
/// audit lines are lost. `main.rs` binds the return into a `let _guard =
/// init_tracing_json()?;` so the guard lives for the whole program.
///
/// Wrapped (rather than returning `WorkerGuard` directly) so we can grow
/// the structure without breaking callers — Phase 5 may add a second
/// guard for a structured event sink, etc.
#[must_use = "drop of LoggingHandle stops the audit appender — bind it in main"]
pub struct LoggingHandle {
    _audit_guard: WorkerGuard,
}

pub fn init_logging() -> Result<LoggingHandle, Box<dyn StdError + Send + Sync + 'static>> {
    dotenv().ok();

    // Capture `log::info!` etc. and route into tracing.
    tracing_log::LogTracer::init()?;

    // ── General sink: JSON to stdout, env-filter (default INFO).
    // Captures EVERY event including audit ones — operators get audit
    // visibility in the unified log too. The audit-only file sink below
    // is additive, not a redirect.
    let env_filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info"));
    let general_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_timer(UtcTime::rfc_3339())
        .with_target(true)
        .with_thread_ids(true)
        .with_thread_names(true)
        .with_current_span(true)
        .with_span_list(true)
        .with_filter(env_filter);

    // ── Audit sink: JSON to dedicated rolling file, filtered to
    // `target: "audit"` events only. `rolling::never` rotates daily by
    // calendar date inside the chosen directory — a single file is fine
    // for Phase 4; logrotate / external retention takes over from there.
    //
    // `non_blocking` keeps the audit emit off the request hot path: the
    // tracing call enqueues a serialized line and a background worker
    // drains to disk. The returned `WorkerGuard` MUST be held alive for
    // the program's lifetime — see `LoggingHandle`.
    let audit_dir = audit_log_dir();
    let file_appender = rolling::never(&audit_dir, DEFAULT_AUDIT_LOG_PATH);
    let (audit_writer, audit_guard) = tracing_appender::non_blocking(file_appender);

    tracing_subscriber::registry()
        .with(audit_layer(audit_writer))
        .with(general_layer)
        .try_init()?;

    Ok(LoggingHandle {
        _audit_guard: audit_guard,
    })
}

/// JSON layer that writes only events whose target is [`AUDIT_TARGET`] to `writer`.
fn audit_layer<W>(writer: W) -> impl Layer<Registry>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    tracing_subscriber::fmt::layer()
        .json()
        .with_timer(UtcTime::rfc_3339())
        .with_target(true)
        .with_writer(writer)
        .with_filter(filter_fn(|metadata| metadata.target() == AUDIT_TARGET))
}

pub fn init_tracing_json() -> Result<LoggingHandle, Box<dyn StdError + Send + Sync + 'static>> {
    init_logging()
}

/// Resolves the directory to write `audit.log` into. `RIM_AUDIT_LOG_PATH`
/// override is interpreted as the parent directory; if it points at a
/// file, we strip the file name and use the parent. Empty / unset →
/// current working directory.
fn audit_log_dir() -> PathBuf {
    match std::env::var(AUDIT_LOG_PATH_ENV) {
        Ok(s) if !s.is_empty() => {
            let p = PathBuf::from(s);
            if p.is_dir() {
                p
            } else {
                p.parent()
                    .map(PathBuf::from)
                    .unwrap_or_else(|| PathBuf::from("."))
            }
        }
        _ => PathBuf::from("."),
    }
}

#[cfg(test)]
mod tests {
    use std::io;
    use std::path::{Path, PathBuf};
    use std::sync::{Arc, Mutex};

    use tracing_subscriber::layer::SubscriberExt;

    use super::{AUDIT_TARGET, audit_layer};

    /// In-memory writer shared between the audit layer and the test.
    #[derive(Clone, Default)]
    struct SharedBuf(Arc<Mutex<Vec<u8>>>);

    impl io::Write for SharedBuf {
        fn write(&mut self, data: &[u8]) -> io::Result<usize> {
            self.0
                .lock()
                .map_err(|_| io::Error::other("poisoned"))?
                .extend_from_slice(data);
            Ok(data.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    #[test]
    fn audit_layer_routes_by_target_not_by_field() {
        let buf = SharedBuf::default();
        let writer = buf.clone();
        let subscriber = tracing_subscriber::registry().with(audit_layer(move || writer.clone()));
        let field_value = AUDIT_TARGET;

        tracing::subscriber::with_default(subscriber, || {
            tracing::info!(target: "audit", marker = "routed_by_target", "audit event");
            tracing::info!(
                target = field_value,
                marker = "routed_by_field",
                "not an audit event"
            );
        });

        let out = String::from_utf8(buf.0.lock().expect("buffer lock").clone()).expect("utf8");
        assert!(out.contains("routed_by_target"), "audit log: {out}");
        assert!(!out.contains("routed_by_field"), "audit log: {out}");
    }

    /// Collects every `.rs` file under `dir`.
    fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
        let entries = std::fs::read_dir(dir).expect("read dir");
        for entry in entries {
            let path = entry.expect("dir entry").path();
            if path.is_dir() {
                rust_files(&path, out);
            } else if path.extension().is_some_and(|ext| ext == "rs") {
                out.push(path);
            }
        }
    }

    /// Fails on any audit target written as a field, which leaves the event on its module
    /// target so it never reaches the audit log.
    #[test]
    fn no_audit_target_written_as_a_field() {
        let workspace = Path::new(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .expect("workspace root");
        let mut files = Vec::new();
        for entry in std::fs::read_dir(workspace).expect("read workspace") {
            let src = entry.expect("dir entry").path().join("src");
            if src.is_dir() {
                rust_files(&src, &mut files);
            }
        }
        assert!(
            !files.is_empty(),
            "no sources found under {}",
            workspace.display()
        );

        let needle = "target=\"audit\"";
        let mut offenders = Vec::new();
        for file in &files {
            let text = std::fs::read_to_string(file).expect("read source");
            for (idx, line) in text.lines().enumerate() {
                let compact: String = line.chars().filter(|c| !c.is_whitespace()).collect();
                if compact.contains(needle) {
                    offenders.push(format!("{}:{}", file.display(), idx + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "write the audit target as `target: \"audit\"`, first in the macro:\n{}",
            offenders.join("\n")
        );
    }
}
