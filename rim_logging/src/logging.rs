use dotenvy::dotenv;
use std::error::Error as StdError;
use std::path::PathBuf;

use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling;
use tracing_subscriber::filter::filter_fn;
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
    // `target = "audit"` events only. `rolling::never` rotates daily by
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
    let audit_layer = tracing_subscriber::fmt::layer()
        .json()
        .with_timer(UtcTime::rfc_3339())
        .with_target(true)
        .with_writer(audit_writer)
        .with_filter(filter_fn(|metadata| metadata.target() == AUDIT_TARGET));

    tracing_subscriber::registry()
        .with(general_layer)
        .with(audit_layer)
        .try_init()?;

    Ok(LoggingHandle {
        _audit_guard: audit_guard,
    })
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
