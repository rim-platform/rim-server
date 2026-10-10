use rim_logging::{LoggingHandle, init_tracing_json};
use rim_server::server::start_server;
use rim_server::{bootstrap_cmd, config, franking_key_cmd, server_key_cmd};

fn initialize() -> Option<LoggingHandle> {
    // Avoid panicking if logger is already initialized (tests, embedding, etc.).
    // Return the handle on success — its `WorkerGuard` field keeps the
    // audit-log background appender alive (P4-A1 §0.5 sink). Dropping it
    // would silently drop pending audit lines.
    let handle = init_tracing_json().ok();
    tracing::info!("Initializing application...");
    tracing::info!("Initializing DB");
    handle
}

#[actix_web::main]
async fn main() {
    // Bind the logging handle for the entire lifetime of `main` — must
    // outlive every `tracing::info!(target = "audit", …)` emit and
    // remain alive until after any error is printed so the final log
    // lines are flushed before the process exits.
    let _logging_handle = initialize();

    let mut args = std::env::args().skip(1);
    if let Some(subcmd) = args.next() {
        let exit_code = match subcmd.as_str() {
            "bootstrap-user" => Some(bootstrap_cmd::run(args).await),
            "server-key" | "peer" => Some(server_key_cmd::run(&subcmd, args).await),
            "franking-key" => Some(franking_key_cmd::run(args)),
            _ => None,
        };
        if let Some(code) = exit_code {
            std::process::exit(code);
        }
    }

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
