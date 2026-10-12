//! CLI subcommand that purges the accounts whose deletion grace period ended, with the same code
//! the server runs hourly.
//!
//! ```text
//! rim_server account purge [--dry-run]
//! ```
//!
//! Prints one line per account with what was removed, then a summary. `--dry-run` runs every
//! account's purge in a transaction that is rolled back, and touches no file and no other
//! server. Exit code 0 on success, 1 on any error or when an account's purge failed, 2 on a
//! usage error.

use std::fmt::Write as _;
use std::sync::Arc;

use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::account_deletion::SeaOrmAccountDeletionRepository;
use rim_infra_services::AppRepos;
use rim_infra_services::account_deletion::DefaultAccountDeletionService;
use rim_services::account_deletion::dto::PurgeRunDto;
use rim_services::account_deletion::service::AccountDeletionService;
use rim_services::events::{NoopBroadcaster, Observers};
use rim_services::remote_workspace::service::WorkspaceRelayService;

use crate::config::RIMConfig;

const USAGE: &str = "usage: rim_server account purge [--dry-run]";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Purge { dry_run: bool },
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    match args.next().as_deref() {
        Some("purge") => {}
        _ => return Err(USAGE.to_string()),
    }
    let mut dry_run = false;
    for arg in args {
        match arg.as_str() {
            "--dry-run" => dry_run = true,
            other => return Err(format!("unknown argument: {other}")),
        }
    }
    Ok(Command::Purge { dry_run })
}

/// Run `rim_server account …`. Returns the exit code.
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
        Ok(run) => {
            print!("{}", describe(&run));
            i32::from(!run.failed.is_empty())
        }
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn execute(command: Command, config: &RIMConfig) -> Result<PurgeRunDto, String> {
    let Command::Purge { dry_run } = command;
    let db = config
        .connect_database()
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let result = purge(config, &db, dry_run).await;
    let _ = db.close().await;
    result
}

/// Build the purge as the server does and run it once.
async fn purge(
    config: &RIMConfig,
    db: &sea_orm::DatabaseConnection,
    dry_run: bool,
) -> Result<PurgeRunDto, String> {
    let handle: DbHandle = db.clone().into();
    let repos = AppRepos::with_handle(&handle);
    let (_, storage) = crate::app::build_storage(config, &repos).map_err(|e| e.to_string())?;
    let mut service = DefaultAccountDeletionService::new(
        Arc::new(SeaOrmAccountDeletionRepository::new(handle.clone())),
        Arc::clone(&repos.session_repo),
        Some(db.clone()),
        storage,
        Observers::new(Arc::new(NoopBroadcaster)),
    );
    if !dry_run {
        let registration = config.registration_mode()?;
        let s2s = crate::app::build_s2s(config, &handle, db, &registration)
            .await
            .map_err(|e| e.to_string())?;
        service =
            service.with_relay_departure(s2s.workspace_relay as Arc<dyn WorkspaceRelayService>);
    }
    service
        .purge_due(chrono::Utc::now(), dry_run)
        .await
        .map_err(|e| e.to_string())
}

/// One line per purged and failed account, then the totals.
fn describe(run: &PurgeRunDto) -> String {
    let mut out = String::new();
    let verb = if run.dry_run { "would purge" } else { "purged" };
    for purged in &run.purged {
        let counts: Vec<String> = purged
            .counts
            .iter()
            .map(|(name, n)| format!("{name}={n}"))
            .collect();
        let _ = writeln!(
            out,
            "{verb} {}  {}  files={}",
            purged.user_id,
            counts.join(" "),
            purged.files
        );
    }
    for failed in &run.failed {
        let _ = writeln!(out, "failed {}  {}", failed.user_id, failed.error);
    }
    let (done, note) = if run.dry_run {
        ("to purge", " (dry run: nothing was changed)")
    } else {
        ("purged", "")
    };
    let _ = writeln!(
        out,
        "{} account(s) {done}, {} failed{note}",
        run.purged.len(),
        run.failed.len(),
    );
    out
}

#[cfg(test)]
mod tests {
    use rim_core::user::model::UserId;
    use rim_services::account_deletion::dto::{AccountPurgeDto, AccountPurgeFailureDto};

    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>()
            .into_iter()
    }

    #[test]
    fn purge_parses_with_and_without_dry_run() {
        assert_eq!(
            parse(args(&["purge"])),
            Ok(Command::Purge { dry_run: false })
        );
        assert_eq!(
            parse(args(&["purge", "--dry-run"])),
            Ok(Command::Purge { dry_run: true })
        );
        assert_eq!(parse(args(&[])), Err(USAGE.to_string()));
        assert_eq!(parse(args(&["delete"])), Err(USAGE.to_string()));
        assert_eq!(
            parse(args(&["purge", "--now"])),
            Err("unknown argument: --now".to_string())
        );
    }

    #[test]
    fn a_run_prints_a_line_per_account_and_the_totals() {
        let (purged, failed) = (UserId::new(), UserId::new());
        let mut run = PurgeRunDto {
            dry_run: false,
            purged: vec![AccountPurgeDto {
                user_id: purged,
                counts: vec![("sessions", 2), ("messages_deleted", 5)],
                files: 1,
            }],
            failed: vec![AccountPurgeFailureDto {
                user_id: failed,
                error: "internal error".to_string(),
            }],
        };
        assert_eq!(
            describe(&run),
            format!(
                "purged {purged}  sessions=2 messages_deleted=5  files=1\n\
                 failed {failed}  internal error\n\
                 1 account(s) purged, 1 failed\n"
            )
        );
        run.dry_run = true;
        run.failed.clear();
        assert_eq!(
            describe(&run),
            format!(
                "would purge {purged}  sessions=2 messages_deleted=5  files=1\n\
                 1 account(s) to purge, 0 failed (dry run: nothing was changed)\n"
            )
        );
    }
}
