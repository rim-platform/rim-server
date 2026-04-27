use crate::server::start_server;
use rim_logging::{LoggingHandle, init_tracing_json};

mod app;
pub(crate) mod config;
mod file_handlers;
mod graphql_support;
mod server;

fn initialize() -> Option<LoggingHandle> {
    // Avoid panicking if logger is already initialized (tests, embedding, etc.).
    // Return the handle on success — its `WorkerGuard` field keeps the
    // audit-log background appender alive (P4-A1 §0.5 sink). Dropping it
    // would silently drop pending audit lines.
    let handle = init_tracing_json().ok();
    log::info!("Initializing application...");
    log::info!("Initializing DB");
    handle
}

#[actix_web::main]
async fn main() {
    // Bind the logging handle for the entire lifetime of `main` — must
    // outlive every `tracing::info!(target = "audit", …)` emit and
    // remain alive until after any error is printed so the final log
    // lines are flushed before the process exits.
    let _logging_handle = initialize();

    let config = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(target = "audit", error = %e, "fatal: failed to load configuration");
            eprintln!("fatal: failed to load configuration: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = start_server(&config).await {
        tracing::error!(target = "audit", error = %e, "fatal: server exited with error");
        eprintln!("fatal: server exited with error: {e}");
        std::process::exit(1);
    }
}
