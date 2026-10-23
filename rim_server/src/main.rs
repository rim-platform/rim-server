use std::process::ExitCode;

use rim_logging::{LoggingHandle, init_cli_logging, init_logging};
use rim_server::server::start_server;
use rim_server::{
    abuse_cmd, account_cmd, audit_cmd, config, franking_key_cmd, invite_cmd, ip_cmd, operator_cmd,
    server_key_cmd,
};

/// Starts logging and returns the handle that keeps the audit file writer alive. Without
/// logging the audit trail would be lost, so a failure ends the process.
fn initialize(command: bool) -> Result<LoggingHandle, ExitCode> {
    let logging = if command {
        init_cli_logging()
    } else {
        init_logging()
    };
    let handle = logging.map_err(|e| {
        eprintln!("fatal: could not start logging: {e}");
        ExitCode::FAILURE
    })?;
    if !command {
        tracing::info!(
            version = config::VERSION,
            "Starting rim-server {}",
            config::VERSION
        );
    }
    Ok(handle)
}

fn usage() {
    eprintln!(
        "Usage: rim_server [server-key|peer|franking-key|principal|abuse-reports|invite|audit|ip|account|operator] [arguments]"
    );
}

#[actix_web::main]
async fn main() -> ExitCode {
    // Answered before logging starts, so the output is the version line alone.
    if matches!(std::env::args().nth(1).as_deref(), Some("--version" | "-V")) {
        println!("{}", config::version_line());
        return ExitCode::SUCCESS;
    }

    let mut args = std::env::args().skip(1);
    let subcommand = args.next();
    if let Some(subcmd) = subcommand.as_deref() {
        match subcmd {
            "--help" | "-h" => {
                usage();
                return ExitCode::SUCCESS;
            }
            "server-key" | "peer" | "franking-key" | "principal" | "abuse-reports" | "invite"
            | "audit" | "ip" | "account" | "operator" => {}
            _ => {
                eprintln!("error: unknown command '{subcmd}'");
                usage();
                return ExitCode::from(2);
            }
        }
    }
    let _logging_handle = match initialize(subcommand.is_some()) {
        Ok(handle) => handle,
        Err(code) => return code,
    };
    if let Some(subcmd) = subcommand {
        let code = match subcmd.as_str() {
            "server-key" | "peer" => server_key_cmd::run(&subcmd, args).await,
            "franking-key" => franking_key_cmd::run(args),
            "principal" | "abuse-reports" => abuse_cmd::run(&subcmd, args).await,
            "invite" => invite_cmd::run(args).await,
            "audit" => audit_cmd::run(args).await,
            "ip" => ip_cmd::run(args).await,
            "account" => account_cmd::run(args).await,
            "operator" => operator_cmd::run(args).await,
            _ => 2,
        };
        return ExitCode::from(u8::try_from(code).unwrap_or(1));
    }

    let config = match config::load_config() {
        Ok(c) => c,
        Err(e) => {
            tracing::error!(target: "audit", error = %e, "fatal: failed to load configuration");
            eprintln!("fatal: failed to load configuration: {e}");
            return ExitCode::FAILURE;
        }
    };

    if let Err(e) = start_server(&config).await {
        tracing::error!(target: "audit", error = %e, "fatal: server exited with error");
        eprintln!("fatal: server exited with error: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
