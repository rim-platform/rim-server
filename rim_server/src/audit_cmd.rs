//! CLI subcommand that purges command audit rows past the retention: the same export-then-delete
//! the daily job runs, or with `--dry-run` only a count of what would go.
//!
//! ```text
//! rim_server audit purge [--dry-run] [--older-than-days <1-3650>]
//! ```
//!
//! Without `--older-than-days` the purge uses `RIM_AUDIT_RETENTION_DAYS` and refuses to run when
//! it is unset. Exit code 0 on success, 1 on any error, 2 on a usage error.

use std::fmt::Write as _;
use std::path::Path;

use chrono::{DateTime, Utc};
use rim_core::audit::repository::AuditLogRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::audit::SeaOrmAuditLogRepository;
use rim_infra_services::audit_retention::{
    self, MAX_RETENTION_DAYS, MIN_RETENTION_DAYS, PurgeReport,
};

use crate::config::RIMConfig;

const USAGE: &str = "usage: rim_server audit purge [--dry-run] [--older-than-days <1-3650>]";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Purge {
        dry_run: bool,
        older_than_days: Option<u32>,
    },
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    match args.next().as_deref() {
        Some("purge") => {}
        _ => return Err(USAGE.to_string()),
    }
    let mut dry_run = false;
    let mut older_than_days = None;
    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--dry-run" => dry_run = true,
            "--older-than-days" => {
                let raw = args
                    .next()
                    .ok_or_else(|| format!("{flag} requires a value"))?;
                let days: u32 = raw
                    .parse()
                    .map_err(|_| format!("{flag} takes a whole number, not {raw}"))?;
                if !(MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days) {
                    return Err(format!(
                        "{flag} must be between {MIN_RETENTION_DAYS} and {MAX_RETENTION_DAYS}"
                    ));
                }
                older_than_days = Some(days);
            }
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Command::Purge {
        dry_run,
        older_than_days,
    })
}

/// Run `rim_server audit …`. Returns the exit code.
pub async fn run(args: impl Iterator<Item = String>) -> i32 {
    let command = match parse(args) {
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

/// Run a subcommand against the database and data directory `config` names and return what
/// it prints.
///
/// # Errors
/// A usage error or what the command failed with.
pub async fn run_with(
    config: &RIMConfig,
    args: impl Iterator<Item = String>,
) -> Result<String, String> {
    execute(parse(args)?, config).await
}

/// The retention to purge with: the flag, else `RIM_AUDIT_RETENTION_DAYS`.
fn retention_days(config: &RIMConfig, older_than_days: Option<u32>) -> Result<u32, String> {
    match older_than_days {
        Some(days) => Ok(days),
        None => config.audit_retention_days()?.ok_or_else(|| {
            "no audit retention is configured: set RIM_AUDIT_RETENTION_DAYS or pass \
             --older-than-days"
                .to_string()
        }),
    }
}

async fn execute(command: Command, config: &RIMConfig) -> Result<String, String> {
    let Command::Purge {
        older_than_days, ..
    } = &command;
    retention_days(config, *older_than_days)?;
    let db = config
        .connect_database()
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let repo = SeaOrmAuditLogRepository::new(DbHandle::from(db.clone()));
    let result = execute_on(command, config, &repo, Utc::now()).await;
    let _ = db.close().await;
    result
}

fn time(at: Option<DateTime<Utc>>) -> String {
    at.map_or_else(|| "-".to_string(), |t| t.to_rfc3339())
}

async fn execute_on(
    command: Command,
    config: &RIMConfig,
    repo: &dyn AuditLogRepository,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let Command::Purge {
        dry_run,
        older_than_days,
    } = command;
    let days = retention_days(config, older_than_days)?;
    let cutoff = audit_retention::cutoff(now, days);
    let mut out = String::new();
    if dry_run {
        let range = repo
            .range_older_than(cutoff)
            .await
            .map_err(|e| e.to_string())?;
        let _ = writeln!(
            out,
            "would purge {} audit rows older than {} ({days} days), from {} to {}",
            range.count,
            cutoff.to_rfc3339(),
            time(range.oldest),
            time(range.newest),
        );
        return Ok(out);
    }
    let export_dir = audit_retention::export_dir(Path::new(&config.data_dir));
    let PurgeReport {
        exported,
        deleted,
        oldest,
        newest,
        export_path,
        ..
    } = audit_retention::run(repo, cutoff, &export_dir, "cli")
        .await
        .map_err(|e| e.to_string())?;
    let _ = writeln!(
        out,
        "purged {deleted} audit rows older than {} ({days} days), from {} to {}",
        cutoff.to_rfc3339(),
        time(oldest),
        time(newest),
    );
    if let Some(path) = export_path {
        let _ = writeln!(out, "exported {exported} rows to {}", path.display());
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use rim_core::audit::repository::{AuditLogRange, MockAuditLogRepository};

    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn parses_the_purge_flags() {
        assert_eq!(
            parse(args(&["purge"])),
            Ok(Command::Purge {
                dry_run: false,
                older_than_days: None
            })
        );
        assert_eq!(
            parse(args(&["purge", "--older-than-days", "30", "--dry-run"])),
            Ok(Command::Purge {
                dry_run: true,
                older_than_days: Some(30)
            })
        );
    }

    #[test]
    fn refuses_bad_usage() {
        assert!(parse(args(&[])).is_err());
        assert!(parse(args(&["prune"])).is_err());
        assert!(parse(args(&["purge", "--older-than-days"])).is_err());
        assert!(parse(args(&["purge", "--older-than-days", "a"])).is_err());
        assert!(parse(args(&["purge", "--older-than-days", "0"])).is_err());
        assert!(parse(args(&["purge", "--older-than-days", "3651"])).is_err());
        assert!(parse(args(&["purge", "--all"])).is_err());
    }

    #[tokio::test]
    async fn a_dry_run_counts_with_the_configured_retention() {
        let now = Utc::now();
        let oldest = now - chrono::Duration::days(200);
        let newest = now - chrono::Duration::days(95);
        let mut repo = MockAuditLogRepository::new();
        repo.expect_range_older_than()
            .withf(move |cutoff| *cutoff == now - chrono::Duration::days(90))
            .returning(move |_| {
                Ok(AuditLogRange {
                    count: 42,
                    oldest: Some(oldest),
                    newest: Some(newest),
                })
            });
        repo.expect_list_older_than().never();
        repo.expect_delete_older_than_batch().never();
        let config = RIMConfig {
            audit_retention_days: Some(90),
            ..Default::default()
        };

        let out = execute_on(
            Command::Purge {
                dry_run: true,
                older_than_days: None,
            },
            &config,
            &repo,
            now,
        )
        .await
        .expect("dry run");

        assert!(out.starts_with("would purge 42 audit rows"), "{out}");
        assert!(out.contains(&oldest.to_rfc3339()), "{out}");
        assert!(out.contains(&newest.to_rfc3339()), "{out}");
    }

    #[tokio::test]
    async fn without_a_retention_the_purge_refuses_unless_given_one() {
        let mut repo = MockAuditLogRepository::new();
        repo.expect_range_older_than().times(1).returning(|_| {
            Ok(AuditLogRange {
                count: 0,
                oldest: None,
                newest: None,
            })
        });
        let config = RIMConfig::default();

        let err = execute_on(
            Command::Purge {
                dry_run: true,
                older_than_days: None,
            },
            &config,
            &repo,
            Utc::now(),
        )
        .await
        .expect_err("no retention");
        assert!(err.contains("RIM_AUDIT_RETENTION_DAYS"), "{err}");

        let out = execute_on(
            Command::Purge {
                dry_run: true,
                older_than_days: Some(7),
            },
            &config,
            &repo,
            Utc::now(),
        )
        .await
        .expect("the flag is enough");
        assert!(out.starts_with("would purge 0 audit rows"), "{out}");
    }
}
