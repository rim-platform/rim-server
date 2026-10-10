use rim_logging::{LoggingHandle, init_logging};
use rim_server::server::start_server;
use rim_server::{
    abuse_cmd, audit_cmd, bootstrap_cmd, config, franking_key_cmd, invite_cmd, server_key_cmd,
};

/// Starts logging and returns the handle that keeps the audit file writer alive. Without
/// logging the audit trail would be lost, so a failure ends the process.
fn initialize() -> LoggingHandle {
    let handle = match init_logging() {
        Ok(handle) => handle,
        Err(e) => {
            eprintln!("fatal: could not start logging: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        version = config::VERSION,
        "Starting rim-server {}",
        config::VERSION
    );
    handle
}

#[actix_web::main]
async fn main() {
    // Answered before logging starts, so the output is the version line alone.
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("{}", config::version_line());
        return;
    }

    // Bind the logging handle for the entire lifetime of `main` — must
    // outlive every `tracing::info!(target: "audit", …)` emit and
    // remain alive until after any error is printed so the final log
    // lines are flushed before the process exits.
    let _logging_handle = initialize();

    let mut args = std::env::args().skip(1);
    if let Some(subcmd) = args.next() {
        let exit_code = match subcmd.as_str() {
            "bootstrap-user" => Some(bootstrap_cmd::run(args).await),
            "server-key" | "peer" => Some(server_key_cmd::run(&subcmd, args).await),
            "franking-key" => Some(franking_key_cmd::run(args)),
            "principal" | "abuse-reports" => Some(abuse_cmd::run(&subcmd, args).await),
            "invite" => Some(invite_cmd::run(args).await),
            "audit" => Some(audit_cmd::run(args).await),
            _ => None,
        };
        if let Some(code) = exit_code {
            std::process::exit(code);
        }
    }

    let config = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(target: "audit", error = %e, "fatal: failed to load configuration");
            eprintln!("fatal: failed to load configuration: {e}");
            std::process::exit(1);
        }
    };

    if let Err(e) = start_server(&config).await {
        tracing::error!(target: "audit", error = %e, "fatal: server exited with error");
        eprintln!("fatal: server exited with error: {e}");
        std::process::exit(1);
    }
}
