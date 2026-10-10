//! CLI subcommands for a relay's abuse tools.
//!
//! ```text
//! rim_server principal block <principal-id> --reason <text>
//! rim_server principal unblock <principal-id>
//! rim_server principal list
//! rim_server abuse-reports list [--open]
//! rim_server abuse-reports add [--principal <principal-id>] [--server <server-id>] --reason <text>
//! rim_server abuse-reports resolve <report-id>
//! ```
//!
//! A block applies at once on every node: its sessions end and the next signup, login or
//! delivery is refused. Exit code 0 on success, 1 on any error, 2 on a usage error.

use std::fmt::Write as _;
use std::sync::Arc;

use rim_core::relay::model::AbuseReportId;
use rim_infra_db::DbHandle;
use rim_infra_services::AppRepos;
use rim_infra_services::relay::abuse::DefaultRelayAbuseService;
use rim_services::relay::dto::{AbuseReportDto, FileAbuseReport};
use rim_services::relay::service::RelayAbuseService;
use sea_orm::Database;
use uuid::Uuid;

use crate::config::RIMConfig;

const USAGE: &str = "usage: rim_server principal block <principal-id> --reason <text> | \
     principal unblock <principal-id> | principal list | abuse-reports list [--open] | \
     abuse-reports add [--principal <principal-id>] [--server <server-id>] --reason <text> | \
     abuse-reports resolve <report-id>";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Block {
        principal_id: String,
        reason: String,
    },
    Unblock {
        principal_id: String,
    },
    ListBlocks,
    ListReports {
        open_only: bool,
    },
    AddReport(FileAbuseReport),
    Resolve {
        id: Uuid,
    },
}

fn value(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn parse(group: &str, mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let action = args.next().ok_or(USAGE)?;
    let command = match (group, action.as_str()) {
        ("principal", "block") => {
            let principal_id = args
                .next()
                .ok_or("principal block requires a principal id")?;
            let mut reason = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--reason" => reason = Some(value("--reason", &mut args)?),
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            return Ok(Command::Block {
                principal_id,
                reason: reason.ok_or("--reason <text> is required")?,
            });
        }
        ("principal", "unblock") => Command::Unblock {
            principal_id: args
                .next()
                .ok_or("principal unblock requires a principal id")?,
        },
        ("principal", "list") => Command::ListBlocks,
        ("abuse-reports", "list") => {
            let mut open_only = false;
            for flag in args.by_ref() {
                match flag.as_str() {
                    "--open" => open_only = true,
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            return Ok(Command::ListReports { open_only });
        }
        ("abuse-reports", "add") => {
            let mut report = FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: None,
                reason: String::new(),
            };
            let mut reason = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--principal" => {
                        report.subject_principal_id = Some(value("--principal", &mut args)?);
                    }
                    "--server" => report.subject_server_id = Some(value("--server", &mut args)?),
                    "--reason" => reason = Some(value("--reason", &mut args)?),
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            if report.subject_principal_id.is_none() && report.subject_server_id.is_none() {
                return Err("give --principal, --server or both".to_string());
            }
            report.reason = reason.ok_or("--reason <text> is required")?;
            return Ok(Command::AddReport(report));
        }
        ("abuse-reports", "resolve") => {
            let id = args
                .next()
                .ok_or("abuse-reports resolve requires a report id")?;
            Command::Resolve {
                id: Uuid::parse_str(&id).map_err(|_| format!("{id} is not a report id"))?,
            }
        }
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

/// Run `rim_server <group> …`, where `group` is `principal` or `abuse-reports`. Returns the
/// exit code.
pub async fn run(group: &str, args: impl Iterator<Item = String>) -> i32 {
    let command = match parse(group, args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    let config = match crate::config::load_config() {
        Ok(config) => config,
        Err(e) => {
            eprintln!("error: failed to load configuration: {e}");
            return 1;
        }
    };
    match execute(command, &config).await {
        Ok(output) => {
            print!("{output}");
            0
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

/// Run a subcommand against the database `config` names and return what it prints. The
/// two-server tests drive the commands through it.
///
/// # Errors
/// A usage error or what the command failed with.
pub async fn run_with(
    config: &RIMConfig,
    group: &str,
    args: impl Iterator<Item = String>,
) -> Result<String, String> {
    execute(parse(group, args)?, config).await
}

fn describe_report(out: &mut String, report: &AbuseReportDto) {
    let state = match report.resolved_at {
        Some(at) => format!("resolved {}", at.to_rfc3339()),
        None => "open".to_string(),
    };
    let _ = writeln!(
        out,
        "{}  {}  {state}",
        report.id,
        report.created_at.to_rfc3339()
    );
    for (name, value) in [
        ("principal", report.subject_principal_id.clone()),
        ("server", report.subject_server_id.clone()),
        ("request", report.request_id.map(|id| id.to_string())),
        (
            "reporter",
            Some(
                report
                    .reporter_user_id
                    .map_or("operator".to_string(), |id| format!("user {id}")),
            ),
        ),
    ] {
        if let Some(value) = value {
            let _ = writeln!(out, "    {name:<9}: {value}");
        }
    }
    let _ = writeln!(out, "    reason   : {}", report.reason);
}

async fn execute(command: Command, config: &RIMConfig) -> Result<String, String> {
    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let handle: DbHandle = db.into();
    let service: Arc<dyn RelayAbuseService> = Arc::new(DefaultRelayAbuseService::new(
        AppRepos::with_handle(&handle).for_relay_abuse(),
    ));
    let mut out = String::new();
    match command {
        Command::Block {
            principal_id,
            reason,
        } => {
            let outcome = service
                .block_principal(principal_id.clone(), reason)
                .await
                .map_err(|e| e.to_string())?;
            if outcome.newly_blocked {
                let _ = writeln!(out, "blocked {principal_id}");
            } else {
                let _ = writeln!(out, "{principal_id} was blocked already; reason replaced");
            }
            let _ = writeln!(
                out,
                "ended {} sessions; ignored {} pending DM requests from it",
                outcome.ended_sessions, outcome.ignored_requests
            );
        }
        Command::Unblock { principal_id } => {
            if service
                .unblock_principal(principal_id.clone())
                .await
                .map_err(|e| e.to_string())?
            {
                let _ = writeln!(out, "unblocked {principal_id}");
            } else {
                let _ = writeln!(out, "{principal_id} was not blocked");
            }
        }
        Command::ListBlocks => {
            for block in service
                .principal_blocks()
                .await
                .map_err(|e| e.to_string())?
            {
                let _ = writeln!(
                    out,
                    "{}  {}  {}",
                    block.principal_id,
                    block.created_at.to_rfc3339(),
                    block.reason
                );
            }
        }
        Command::ListReports { open_only } => {
            for report in service
                .reports(open_only)
                .await
                .map_err(|e| e.to_string())?
            {
                describe_report(&mut out, &report);
            }
        }
        Command::AddReport(report) => {
            let stored = service
                .file_report(report)
                .await
                .map_err(|e| e.to_string())?;
            describe_report(&mut out, &stored);
        }
        Command::Resolve { id } => {
            match service
                .resolve_report(AbuseReportId::from(id))
                .await
                .map_err(|e| e.to_string())?
            {
                Some(report) => describe_report(&mut out, &report),
                None => return Err(format!("no report {id}")),
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    const P: &str = "rimp1_txDLbnipi5i3kCJtJwBOUixzv_MY8OnU0mcy--Tyje0";
    const S: &str = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse("principal", args(&["block", P, "--reason", "spam"])),
            Ok(Command::Block {
                principal_id: P.to_string(),
                reason: "spam".to_string()
            })
        );
        assert_eq!(
            parse("principal", args(&["unblock", P])),
            Ok(Command::Unblock {
                principal_id: P.to_string()
            })
        );
        assert_eq!(parse("principal", args(&["list"])), Ok(Command::ListBlocks));
        assert_eq!(
            parse("abuse-reports", args(&["list", "--open"])),
            Ok(Command::ListReports { open_only: true })
        );
        assert_eq!(
            parse("abuse-reports", args(&["list"])),
            Ok(Command::ListReports { open_only: false })
        );
        assert_eq!(
            parse(
                "abuse-reports",
                args(&["add", "--server", S, "--reason", "mail from abuse@"])
            ),
            Ok(Command::AddReport(FileAbuseReport {
                subject_principal_id: None,
                subject_server_id: Some(S.to_string()),
                reason: "mail from abuse@".to_string(),
            }))
        );
        let id = Uuid::now_v7();
        assert_eq!(
            parse("abuse-reports", args(&["resolve", &id.to_string()])),
            Ok(Command::Resolve { id })
        );
    }

    #[test]
    fn refuses_bad_usage() {
        assert!(parse("principal", args(&[])).is_err());
        assert!(parse("principal", args(&["block", P])).is_err());
        assert!(parse("principal", args(&["block", P, "--reason"])).is_err());
        assert!(parse("principal", args(&["unblock"])).is_err());
        assert!(parse("principal", args(&["list", "extra"])).is_err());
        assert!(parse("abuse-reports", args(&["list", "--all"])).is_err());
        assert!(parse("abuse-reports", args(&["add", "--reason", "x"])).is_err());
        assert!(parse("abuse-reports", args(&["add", "--principal", P])).is_err());
        assert!(parse("abuse-reports", args(&["resolve", "42"])).is_err());
        assert!(parse("abuse-reports", args(&["delete"])).is_err());
    }
}
