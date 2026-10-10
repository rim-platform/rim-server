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

fn nothing(cutoff: DateTime<Utc>, days: u32) -> String {
    format!(
        "no audit rows older than {} ({days} days)",
        cutoff.to_rfc3339()
    )
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
        if range.count == 0 {
            let _ = writeln!(out, "{}", nothing(cutoff, days));
        } else {
            let _ = writeln!(
                out,
                "would purge {} audit rows older than {} ({days} days), from {} to {}",
                range.count,
                cutoff.to_rfc3339(),
                time(range.oldest),
                time(range.newest),
            );
        }
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
    let Some(path) = export_path else {
        let _ = writeln!(out, "{}", nothing(cutoff, days));
        return Ok(out);
    };
    let _ = writeln!(
        out,
        "purged {deleted} audit rows older than {} ({days} days), from {} to {}",
        cutoff.to_rfc3339(),
        time(oldest),
        time(newest),
    );
    let _ = writeln!(out, "exported {exported} rows to {}", path.display());
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
        assert!(out.starts_with("no audit rows older than"), "{out}");
    }

    #[tokio::test]
    async fn a_real_run_exports_into_the_data_directory_and_deletes() {
        let now = Utc::now();
        let row = rim_core::audit::repository::AuditLogEntryRecord {
            id: uuid::Uuid::now_v7(),
            actor_user_id: uuid::Uuid::now_v7(),
            server_id: uuid::Uuid::now_v7(),
            command_id: "/ping".to_string(),
            initiator: "palette".to_string(),
            args_redacted: serde_json::json!({}),
            sensitivity: "low".to_string(),
            dangerous: false,
            confirmation_kind: "none".to_string(),
            typed_verb: None,
            outcome: "success".to_string(),
            error_kind: None,
            capability_check_outcome: None,
            confirmation_outcome: None,
            parent_audit_id: None,
            created_at: now - chrono::Duration::days(40),
        };
        let mut repo = MockAuditLogRepository::new();
        let page = row.clone();
        repo.expect_list_older_than()
            .returning(move |_, after, _| Ok(after.map_or_else(|| vec![page.clone()], |_| vec![])));
        repo.expect_delete_older_than_batch()
            .withf(move |_, up_to, _| *up_to == (row.created_at, row.id))
            .times(1)
            .returning(|_, _, _| Ok(1));
        let data_dir = std::env::temp_dir().join(format!("rim-audit-cmd-{}", uuid::Uuid::now_v7()));
        let config = RIMConfig {
            data_dir: data_dir.to_string_lossy().into_owned(),
            ..Default::default()
        };

        let out = execute_on(
            Command::Purge {
                dry_run: false,
                older_than_days: Some(30),
            },
            &config,
            &repo,
            now,
        )
        .await;
        let exports: Vec<_> = std::fs::read_dir(data_dir.join("audit-exports"))
            .map(|d| d.filter_map(Result::ok).map(|e| e.path()).collect())
            .unwrap_or_default();
        let _ = std::fs::remove_dir_all(&data_dir);

        let out = out.expect("purged");
        assert!(out.starts_with("purged 1 audit rows"), "{out}");
        assert_eq!(exports.len(), 1, "{exports:?}");
        assert!(out.contains(&exports[0].display().to_string()), "{out}");
    }
}
