//! CLI subcommands for the operator's instance invites, which admit a signup when registration
//! is invite-only or proof-of-work. They need no account and no workspace, so they work on a
//! server that runs only the relay role.
//!
//! ```text
//! rim_server invite create [--max-uses <1-50>] [--expires-in-days <1-30>] [--public-url <url>]
//! rim_server invite list
//! rim_server invite revoke <invite-id>
//! ```
//!
//! `create` prints the signup link (`<public url>/join/<token>`) on its first line; the token is
//! shown once and only its hash is stored. Exit code 0 on success, 1 on any error, 2 on a usage
//! error.

use std::fmt::Write as _;

use chrono::{Duration, Utc};
use rim_core::auth::model::{InstanceInvite, InstanceInviteId};
use rim_core::auth::repository::InstanceInviteRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::instance_invite::SeaOrmInstanceInviteRepository;
use rim_infra_services::instance_invite::{MAX_TTL_DAYS, issue};
use uuid::Uuid;

use crate::config::RIMConfig;

const USAGE: &str = "usage: rim_server invite create [--max-uses <1-50>] \
     [--expires-in-days <1-30>] [--public-url <url>] | invite list | invite revoke <invite-id>";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Create {
        max_uses: Option<i32>,
        expires_in_days: Option<i64>,
        public_url: Option<String>,
    },
    List,
    Revoke {
        id: Uuid,
    },
}

fn value(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn number<T: std::str::FromStr>(flag: &str, raw: &str) -> Result<T, String> {
    raw.parse()
        .map_err(|_| format!("{flag} takes a whole number, not {raw}"))
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let action = args.next().ok_or(USAGE)?;
    let command = match action.as_str() {
        "create" => {
            let mut max_uses = None;
            let mut expires_in_days = None;
            let mut public_url = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--max-uses" => {
                        max_uses = Some(number(&flag, &value(&flag, &mut args)?)?);
                    }
                    "--expires-in-days" => {
                        let days: i64 = number(&flag, &value(&flag, &mut args)?)?;
                        if !(1..=MAX_TTL_DAYS).contains(&days) {
                            return Err(format!(
                                "--expires-in-days must be between 1 and {MAX_TTL_DAYS}"
                            ));
                        }
                        expires_in_days = Some(days);
                    }
                    "--public-url" => public_url = Some(value(&flag, &mut args)?),
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            return Ok(Command::Create {
                max_uses,
                expires_in_days,
                public_url,
            });
        }
        "list" => Command::List,
        "revoke" => {
            let id = args.next().ok_or("invite revoke requires an invite id")?;
            Command::Revoke {
                id: Uuid::parse_str(&id).map_err(|_| format!("{id} is not an invite id"))?,
            }
        }
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

/// Run `rim_server invite …`. Returns the exit code.
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
/// two-server tests drive the command through it.
///
/// # Errors
/// A usage error or what the command failed with.
pub async fn run_with(
    config: &RIMConfig,
    args: impl Iterator<Item = String>,
) -> Result<String, String> {
    execute(parse(args)?, config).await
}

/// Where a signup link points: `--public-url`, else `RIM_PUBLIC_BASE_URL`, else the bind
/// address.
fn base_url(config: &RIMConfig, public_url: Option<String>) -> String {
    let base = public_url
        .or_else(|| config.public_base_url())
        .unwrap_or_else(|| format!("http://{}:{}", config.server_host, config.server_port));
    base.trim_end_matches('/').to_string()
}

fn describe(out: &mut String, invite: &InstanceInvite) {
    let state = match invite.revoked_at {
        Some(at) => format!("revoked {}", at.to_rfc3339()),
        None if invite.expires_at <= Utc::now() => "expired".to_string(),
        None if invite.use_count >= invite.max_uses => "used up".to_string(),
        None => "active".to_string(),
    };
    let issuer = invite
        .created_by_user_id
        .map_or("operator".to_string(), |id| format!("user {id}"));
    let _ = writeln!(
        out,
        "{}  used {}/{}  expires {}  {state}  issued by {issuer}",
        invite.id,
        invite.use_count,
        invite.max_uses,
        invite.expires_at.to_rfc3339(),
    );
}

async fn execute(command: Command, config: &RIMConfig) -> Result<String, String> {
    let db = config
        .connect_database()
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let handle: DbHandle = db.clone().into();
    let repo = SeaOrmInstanceInviteRepository::new(handle);
    let result = execute_on(command, config, &repo).await;
    let _ = db.close().await;
    result
}

async fn execute_on(
    command: Command,
    config: &RIMConfig,
    repo: &dyn InstanceInviteRepository,
) -> Result<String, String> {
    let mut out = String::new();
    match command {
        Command::Create {
            max_uses,
            expires_in_days,
            public_url,
        } => {
            let now = Utc::now();
            let issued = issue(
                repo,
                None,
                expires_in_days.map(|days| now + Duration::days(days)),
                max_uses,
                now,
            )
            .await
            .map_err(|e| e.to_string())?;
            tracing::info!(
                target: "audit",
                event = "instance_invite_issued",
                instance_invite_id = %issued.invite.id,
                issued_by = "operator",
                "instance invite issued via CLI"
            );
            let _ = writeln!(
                out,
                "{}/join/{}",
                base_url(config, public_url),
                issued.token
            );
            describe(&mut out, &issued.invite);
        }
        Command::List => {
            for invite in repo.list_active().await.map_err(|e| e.to_string())? {
                describe(&mut out, &invite);
            }
        }
        Command::Revoke { id } => {
            let id = InstanceInviteId::from(id);
            let invite = repo
                .get_by_id(id)
                .await
                .map_err(|e| e.to_string())?
                .ok_or_else(|| format!("no invite {id}"))?;
            repo.revoke(id).await.map_err(|e| e.to_string())?;
            tracing::info!(
                target: "audit",
                event = "instance_invite_revoked",
                invite_id = %id,
                actor = "operator",
                "instance invite revoked via CLI"
            );
            let _ = writeln!(out, "revoked {}", invite.id);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, Mutex};

    use rim_core::auth::repository::MockInstanceInviteRepository;

    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    const ID: &str = "01928c4e-8f6a-7b3c-9d2e-1f0a2b3c4d5e";

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse(args(&["create"])),
            Ok(Command::Create {
                max_uses: None,
                expires_in_days: None,
                public_url: None
            })
        );
        assert_eq!(
            parse(args(&[
                "create",
                "--max-uses",
                "5",
                "--expires-in-days",
                "30",
                "--public-url",
                "https://relay.example"
            ])),
            Ok(Command::Create {
                max_uses: Some(5),
                expires_in_days: Some(30),
                public_url: Some("https://relay.example".to_string())
            })
        );
        assert_eq!(parse(args(&["list"])), Ok(Command::List));
        assert_eq!(
            parse(args(&["revoke", ID])),
            Ok(Command::Revoke {
                id: Uuid::parse_str(ID).expect("uuid")
            })
        );
    }

    #[test]
    fn refuses_bad_usage() {
        assert!(parse(args(&[])).is_err());
        assert!(parse(args(&["delete"])).is_err());
        assert!(parse(args(&["create", "--max-uses"])).is_err());
        assert!(parse(args(&["create", "--max-uses", "many"])).is_err());
        assert!(parse(args(&["create", "--expires-in-days", "0"])).is_err());
        assert!(parse(args(&["create", "--expires-in-days", "31"])).is_err());
        assert!(parse(args(&["create", "--uses", "2"])).is_err());
        assert!(parse(args(&["list", "--all"])).is_err());
        assert!(parse(args(&["revoke"])).is_err());
        assert!(parse(args(&["revoke", "not-a-uuid"])).is_err());
    }

    #[test]
    fn the_link_prefers_the_flag_then_the_public_url_then_the_bind_address() {
        let mut config = RIMConfig {
            server_host: "127.0.0.1".to_string(),
            server_port: 8090,
            ..Default::default()
        };
        assert_eq!(base_url(&config, None), "http://127.0.0.1:8090");
        config.public_base_url = Some("https://relay.example/".to_string());
        assert_eq!(base_url(&config, None), "https://relay.example");
        assert_eq!(
            base_url(&config, Some("https://other.example".to_string())),
            "https://other.example"
        );
    }

    #[tokio::test]
    async fn create_issues_an_operator_invite_on_a_relay_with_no_users() {
        let stored = Arc::new(Mutex::new(None));
        let mut repo = MockInstanceInviteRepository::new();
        let seen = Arc::clone(&stored);
        repo.expect_create().returning(move |new| {
            let invite = InstanceInvite {
                id: InstanceInviteId::new(),
                created_by_user_id: new.created_by_user_id,
                token_hash: new.token_hash,
                created_at: Utc::now(),
                expires_at: new.expires_at,
                max_uses: new.max_uses,
                use_count: 0,
                revoked_at: None,
            };
            if let Ok(mut slot) = seen.lock() {
                *slot = Some(invite.clone());
            }
            Ok(invite)
        });
        let config = RIMConfig {
            role: Some("relay".to_string()),
            public_base_url: Some("https://relay.example".to_string()),
            ..Default::default()
        };

        let out = execute_on(
            Command::Create {
                max_uses: Some(3),
                expires_in_days: Some(2),
                public_url: None,
            },
            &config,
            &repo,
        )
        .await
        .expect("created");

        let link = out.lines().next().expect("a link");
        let token = link
            .strip_prefix("https://relay.example/join/")
            .expect("a join link");
        let invite = stored.lock().expect("lock").clone().expect("stored");
        assert_eq!(
            invite.token_hash,
            rim_auth::session_token::hash_token(token)
        );
        assert_eq!(invite.created_by_user_id, None);
        assert_eq!(invite.max_uses, 3);
        assert!(out.contains("issued by operator"), "{out}");
    }

    #[tokio::test]
    async fn revoke_names_a_missing_invite() {
        let mut repo = MockInstanceInviteRepository::new();
        repo.expect_get_by_id().returning(|_| Ok(None));
        repo.expect_revoke().never();
        let err = execute_on(
            Command::Revoke {
                id: Uuid::parse_str(ID).expect("uuid"),
            },
            &RIMConfig::default(),
            &repo,
        )
        .await
        .expect_err("missing");
        assert!(err.contains("no invite"), "{err}");
    }
}
