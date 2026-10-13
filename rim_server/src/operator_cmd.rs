//! CLI subcommands for instance operators, the accounts that run the moderation queue from the
//! client.
//!
//! ```text
//! rim_server operator grant <username>
//! rim_server operator revoke <username>
//! rim_server operator list
//! ```
//!
//! A blocked account or one scheduled for deletion cannot be granted the role. Exit code 0 on
//! success, 1 on any error, 2 on a usage error.

use std::fmt::Write as _;
use std::sync::Arc;

use rim_infra_db::DbHandle;
use rim_infra_services::AppRepos;
use rim_infra_services::operator::DefaultInstanceOperatorService;
use rim_services::operator::InstanceOperatorService;

use crate::config::RIMConfig;

const USAGE: &str =
    "usage: rim_server operator grant <username> | operator revoke <username> | operator list";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Grant { username: String },
    Revoke { username: String },
    List,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let action = args.next().ok_or(USAGE)?;
    let command = match action.as_str() {
        "grant" => Command::Grant {
            username: args.next().ok_or("operator grant requires a username")?,
        },
        "revoke" => Command::Revoke {
            username: args.next().ok_or("operator revoke requires a username")?,
        },
        "list" => Command::List,
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

/// Run `rim_server operator …`. Returns the exit code.
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

/// Run a subcommand against the database `config` names and return what it prints. The
/// end-to-end tests make their operators with it.
///
/// # Errors
/// A usage error or what the command failed with.
pub async fn run_with(
    config: &RIMConfig,
    args: impl Iterator<Item = String>,
) -> Result<String, String> {
    execute(parse(args)?, config).await
}

async fn execute(command: Command, config: &RIMConfig) -> Result<String, String> {
    let db = config
        .connect_database()
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let handle = DbHandle::from(db.clone());
    let service: Arc<dyn InstanceOperatorService> = Arc::new(DefaultInstanceOperatorService::new(
        AppRepos::with_handle(&handle).for_relay_abuse(),
    ));
    let result = execute_on(command, service.as_ref()).await;
    let _ = db.close().await;
    result
}

async fn execute_on(
    command: Command,
    service: &dyn InstanceOperatorService,
) -> Result<String, String> {
    let mut out = String::new();
    match command {
        Command::Grant { username } => {
            let change = service
                .grant(username.clone())
                .await
                .map_err(|e| failure(e, &username))?;
            if change.changed {
                let _ = writeln!(out, "{username} is now an instance operator");
            } else {
                let _ = writeln!(out, "{username} was an instance operator already");
            }
        }
        Command::Revoke { username } => {
            let change = service
                .revoke(username.clone())
                .await
                .map_err(|e| failure(e, &username))?;
            if change.changed {
                let _ = writeln!(out, "{username} is no longer an instance operator");
            } else {
                let _ = writeln!(out, "{username} was not an instance operator");
            }
        }
        Command::List => {
            let operators = service.operators().await.map_err(|e| e.to_string())?;
            if operators.is_empty() {
                let _ = writeln!(out, "no instance operators");
            }
            for account in operators {
                let _ = writeln!(
                    out,
                    "{}  {}  user {}",
                    account.username,
                    account.principal_id.as_deref().unwrap_or("no key identity"),
                    account.user_id
                );
            }
        }
    }
    Ok(out)
}

/// What a grant or revoke failed with, naming the account when there is none.
fn failure(error: rim_services::ServiceError, username: &str) -> String {
    match error {
        rim_services::ServiceError::NotFound => format!("no account {username}"),
        other => other.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use chrono::Utc;
    use rim_core::user::model::UserId;
    use rim_services::ServiceError;
    use rim_services::operator::{MockInstanceOperatorService, OperatorChangeDto};
    use rim_services::relay::dto::AccountSummaryDto;

    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn ada() -> AccountSummaryDto {
        AccountSummaryDto {
            user_id: UserId::new(),
            username: "ada".to_string(),
            principal_id: None,
            created_at: Utc::now(),
        }
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse(args(&["grant", "ada"])),
            Ok(Command::Grant {
                username: "ada".to_string()
            })
        );
        assert_eq!(
            parse(args(&["revoke", "ada"])),
            Ok(Command::Revoke {
                username: "ada".to_string()
            })
        );
        assert_eq!(parse(args(&["list"])), Ok(Command::List));
    }

    #[test]
    fn refuses_bad_usage() {
        let usages: &[&[&str]] = &[
            &[],
            &["promote", "ada"],
            &["grant"],
            &["revoke"],
            &["grant", "ada", "bob"],
            &["list", "--all"],
        ];
        for bad in usages {
            assert!(parse(args(bad)).is_err(), "{bad:?}");
        }
    }

    #[tokio::test]
    async fn grant_and_revoke_say_what_changed() {
        let mut service = MockInstanceOperatorService::new();
        let mut first = true;
        service
            .expect_grant()
            .withf(|username| username == "ada")
            .times(2)
            .returning(move |_| {
                let changed = std::mem::replace(&mut first, false);
                Box::pin(async move {
                    Ok(OperatorChangeDto {
                        account: ada(),
                        changed,
                    })
                })
            });
        service.expect_revoke().returning(|_| {
            Box::pin(async {
                Ok(OperatorChangeDto {
                    account: ada(),
                    changed: false,
                })
            })
        });
        let grant = || Command::Grant {
            username: "ada".to_string(),
        };

        assert_eq!(
            execute_on(grant(), &service).await.expect("grant"),
            "ada is now an instance operator\n"
        );
        assert_eq!(
            execute_on(grant(), &service).await.expect("again"),
            "ada was an instance operator already\n"
        );
        let revoke = Command::Revoke {
            username: "ada".to_string(),
        };
        assert_eq!(
            execute_on(revoke, &service).await.expect("revoke"),
            "ada was not an instance operator\n"
        );
    }

    #[tokio::test]
    async fn a_refused_grant_says_why() {
        let mut service = MockInstanceOperatorService::new();
        service.expect_grant().returning(|username| {
            let error = if username == "ghost" {
                ServiceError::NotFound
            } else {
                ServiceError::Validation(format!("{username} is blocked"))
            };
            Box::pin(async move { Err(error) })
        });
        let grant = |username: &str| Command::Grant {
            username: username.to_string(),
        };
        assert_eq!(
            execute_on(grant("ghost"), &service).await,
            Err("no account ghost".to_string())
        );
        assert_eq!(
            execute_on(grant("spammer"), &service).await,
            Err("validation error: spammer is blocked".to_string())
        );
    }

    #[tokio::test]
    async fn list_prints_one_line_per_operator() {
        let mut service = MockInstanceOperatorService::new();
        service
            .expect_operators()
            .returning(|| Box::pin(async { Ok(vec![ada()]) }));
        let out = execute_on(Command::List, &service).await.expect("list");
        assert!(out.starts_with("ada  no key identity  user "), "{out}");

        let mut empty = MockInstanceOperatorService::new();
        empty
            .expect_operators()
            .returning(|| Box::pin(async { Ok(vec![]) }));
        assert_eq!(
            execute_on(Command::List, &empty).await.expect("list"),
            "no instance operators\n"
        );
    }
}
