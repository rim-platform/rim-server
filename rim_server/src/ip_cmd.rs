//! CLI subcommands for the IP deny list.
//!
//! ```text
//! rim_server ip block <cidr> --reason <text> [--expires-in-days <1-365>]
//! rim_server ip unblock <cidr>
//! rim_server ip list
//! ```
//!
//! `<cidr>` is an address or a CIDR range, IPv4 or IPv6. A blocked address is answered 403 on
//! every route but `/healthz`; a running server picks up a block or unblock within 30 seconds.
//! Exit code 0 on success, 1 on any error, 2 on a usage error.

use std::fmt::Write as _;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rim_core::ip_block::model::IpRange;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::ip_block::SeaOrmIpBlockRepository;
use rim_infra_db::sea_orm_impl::user::SeaOrmUserRepository;
use rim_infra_services::ip_block::DefaultIpBlockService;
use rim_services::ip_block::dto::{BlockIpCmd, IpBlockDto};
use rim_services::ip_block::service::{IpBlockService, MAX_BLOCK_DAYS};
use rim_services::operator::OperatorCaller;

use crate::config::RIMConfig;

/// The commands run as the operator at the command line.
const CLI: OperatorCaller = OperatorCaller::CommandLine;

const USAGE: &str = "usage: rim_server ip block <cidr> --reason <text> \
     [--expires-in-days <1-365>] | ip unblock <cidr> | ip list";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Block {
        range: IpRange,
        reason: String,
        expires_in_days: Option<u32>,
    },
    Unblock {
        range: IpRange,
    },
    List,
}

fn value(flag: &str, args: &mut impl Iterator<Item = String>) -> Result<String, String> {
    args.next()
        .ok_or_else(|| format!("{flag} requires a value"))
}

fn range(action: &str, args: &mut impl Iterator<Item = String>) -> Result<IpRange, String> {
    args.next()
        .ok_or_else(|| format!("ip {action} requires an address or CIDR range"))?
        .parse()
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let action = args.next().ok_or(USAGE)?;
    let command = match action.as_str() {
        "block" => {
            let range = range("block", &mut args)?;
            let mut reason = None;
            let mut expires_in_days = None;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--reason" => reason = Some(value("--reason", &mut args)?),
                    "--expires-in-days" => {
                        let raw = value("--expires-in-days", &mut args)?;
                        let days: u32 = raw
                            .parse()
                            .map_err(|_| format!("{flag} takes a whole number, not {raw}"))?;
                        if !(1..=MAX_BLOCK_DAYS).contains(&days) {
                            return Err(format!("{flag} must be between 1 and {MAX_BLOCK_DAYS}"));
                        }
                        expires_in_days = Some(days);
                    }
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            return Ok(Command::Block {
                range,
                reason: reason.ok_or("--reason <text> is required")?,
                expires_in_days,
            });
        }
        "unblock" => Command::Unblock {
            range: range("unblock", &mut args)?,
        },
        "list" => Command::List,
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

/// Run `rim_server ip …`. Returns the exit code.
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

async fn execute(command: Command, config: &RIMConfig) -> Result<String, String> {
    let db = config
        .connect_database()
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let handle = DbHandle::from(db.clone());
    let service: Arc<dyn IpBlockService> = Arc::new(DefaultIpBlockService::new(
        Arc::new(SeaOrmIpBlockRepository::new(handle.clone())),
        Arc::new(SeaOrmUserRepository::new(handle)),
    ));
    let result = execute_on(command, service.as_ref(), Utc::now()).await;
    let _ = db.close().await;
    result
}

fn expiry(at: Option<DateTime<Utc>>, now: DateTime<Utc>) -> String {
    match at {
        None => "never".to_string(),
        Some(at) if at <= now => format!("{} (expired)", at.to_rfc3339()),
        Some(at) => at.to_rfc3339(),
    }
}

fn table(blocks: &[IpBlockDto], now: DateTime<Utc>) -> String {
    let mut out = String::new();
    if blocks.is_empty() {
        let _ = writeln!(out, "no IP blocks");
        return out;
    }
    let rows: Vec<[String; 4]> = blocks
        .iter()
        .map(|b| {
            [
                b.range.to_string(),
                expiry(b.expires_at, now),
                b.created_at.to_rfc3339(),
                b.reason.clone(),
            ]
        })
        .collect();
    let header = ["CIDR", "EXPIRES", "CREATED", "REASON"];
    let width = |column: usize| {
        rows.iter()
            .map(|row| row[column].len())
            .chain([header[column].len()])
            .max()
            .unwrap_or(0)
    };
    let (cidr, expires, created) = (width(0), width(1), width(2));
    let _ = writeln!(
        out,
        "{:<cidr$}  {:<expires$}  {:<created$}  {}",
        header[0], header[1], header[2], header[3]
    );
    for [c, e, t, r] in rows {
        let _ = writeln!(out, "{c:<cidr$}  {e:<expires$}  {t:<created$}  {r}");
    }
    out
}

async fn execute_on(
    command: Command,
    service: &dyn IpBlockService,
    now: DateTime<Utc>,
) -> Result<String, String> {
    let mut out = String::new();
    match command {
        Command::Block {
            range,
            reason,
            expires_in_days,
        } => {
            let outcome = service
                .block(
                    CLI,
                    BlockIpCmd {
                        cidr: range.to_string(),
                        reason,
                        expires_in_days,
                    },
                )
                .await
                .map_err(|e| e.to_string())?;
            let until = match outcome.block.expires_at {
                Some(at) => format!("until {}", at.to_rfc3339()),
                None => "until unblocked".to_string(),
            };
            if outcome.newly_blocked {
                let _ = writeln!(out, "blocked {} {until}", outcome.block.range);
            } else {
                let _ = writeln!(
                    out,
                    "{} was blocked already; reason replaced, blocked {until}",
                    outcome.block.range
                );
            }
        }
        Command::Unblock { range } => {
            if service
                .unblock(CLI, range.to_string())
                .await
                .map_err(|e| e.to_string())?
            {
                let _ = writeln!(out, "unblocked {range}");
            } else {
                let _ = writeln!(out, "{range} was not blocked");
            }
        }
        Command::List => {
            let blocks = service.list(CLI).await.map_err(|e| e.to_string())?;
            out = table(&blocks, now);
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use rim_services::ip_block::dto::IpBlockOutcome;
    use rim_services::ip_block::service::MockIpBlockService;

    use super::*;

    fn args(list: &[&str]) -> impl Iterator<Item = String> {
        list.iter()
            .map(|s| s.to_string())
            .collect::<Vec<_>>()
            .into_iter()
    }

    fn range(s: &str) -> IpRange {
        s.parse().expect("a range")
    }

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(
            parse(args(&["block", "203.0.113.7/24", "--reason", "scraping"])),
            Ok(Command::Block {
                range: range("203.0.113.0/24"),
                reason: "scraping".to_string(),
                expires_in_days: None,
            })
        );
        assert_eq!(
            parse(args(&[
                "block",
                "2001:db8::1",
                "--expires-in-days",
                "365",
                "--reason",
                "spam"
            ])),
            Ok(Command::Block {
                range: range("2001:db8::1/128"),
                reason: "spam".to_string(),
                expires_in_days: Some(365),
            })
        );
        assert_eq!(
            parse(args(&["unblock", "127.0.0.1"])),
            Ok(Command::Unblock {
                range: range("127.0.0.1/32")
            })
        );
        assert_eq!(parse(args(&["list"])), Ok(Command::List));
    }

    #[test]
    fn refuses_bad_usage() {
        let usages: &[&[&str]] = &[
            &[],
            &["ban"],
            &["block"],
            &["block", "localhost", "--reason", "x"],
            &["block", "192.0.2.0/33", "--reason", "x"],
            &["block", "192.0.2.1"],
            &["block", "192.0.2.1", "--reason"],
            &["block", "192.0.2.1", "--reason", "x", "--expires-in-days"],
            &[
                "block",
                "192.0.2.1",
                "--reason",
                "x",
                "--expires-in-days",
                "0",
            ],
            &[
                "block",
                "192.0.2.1",
                "--reason",
                "x",
                "--expires-in-days",
                "366",
            ],
            &[
                "block",
                "192.0.2.1",
                "--reason",
                "x",
                "--expires-in-days",
                "a",
            ],
            &["block", "192.0.2.1", "--reason", "x", "--forever"],
            &["unblock"],
            &["unblock", "192.0.2.1", "extra"],
            &["list", "--all"],
        ];
        for bad in usages {
            assert!(parse(args(bad)).is_err(), "{bad:?}");
        }
    }

    fn dto(cidr: &str, reason: &str, expires_at: Option<DateTime<Utc>>) -> IpBlockDto {
        IpBlockDto {
            range: range(cidr),
            reason: reason.to_string(),
            expires_at,
            created_at: DateTime::parse_from_rfc3339("2026-10-01T00:00:00Z")
                .expect("time")
                .with_timezone(&Utc),
        }
    }

    #[tokio::test]
    async fn block_and_unblock_say_what_changed() {
        let mut service = MockIpBlockService::new();
        let mut first = true;
        service
            .expect_block()
            .withf(|by, cmd| {
                *by == CLI
                    && cmd.cidr == "203.0.113.0/24"
                    && cmd.reason == "scraping"
                    && cmd.expires_in_days == Some(7)
            })
            .times(2)
            .returning(move |_, _| {
                let newly_blocked = std::mem::replace(&mut first, false);
                Box::pin(async move {
                    Ok(IpBlockOutcome {
                        block: dto("203.0.113.0/24", "scraping", None),
                        newly_blocked,
                    })
                })
            });
        service
            .expect_unblock()
            .withf(|by, cidr| *by == CLI && cidr == "203.0.113.0/24")
            .returning(|_, _| Box::pin(async { Ok(true) }));
        let now = Utc::now();
        let block = || Command::Block {
            range: range("203.0.113.0/24"),
            reason: "scraping".to_string(),
            expires_in_days: Some(7),
        };

        let out = execute_on(block(), &service, now).await.expect("block");
        assert_eq!(out, "blocked 203.0.113.0/24 until unblocked\n");
        let out = execute_on(block(), &service, now).await.expect("again");
        assert!(
            out.starts_with("203.0.113.0/24 was blocked already"),
            "{out}"
        );
        let out = execute_on(
            Command::Unblock {
                range: range("203.0.113.0/24"),
            },
            &service,
            now,
        )
        .await
        .expect("unblock");
        assert_eq!(out, "unblocked 203.0.113.0/24\n");
    }

    #[tokio::test]
    async fn list_prints_a_table_with_each_expiry() {
        let now = DateTime::parse_from_rfc3339("2026-10-12T00:00:00Z")
            .expect("time")
            .with_timezone(&Utc);
        let mut service = MockIpBlockService::new();
        service.expect_list().times(1).returning(move |_| {
            Box::pin(async move {
                Ok(vec![
                    dto("198.51.100.0/24", "spam", None),
                    dto(
                        "2001:db8::/32",
                        "scraping",
                        Some(now + chrono::Duration::days(1)),
                    ),
                    dto("192.0.2.1/32", "old", Some(now - chrono::Duration::days(1))),
                ])
            })
        });
        let out = execute_on(Command::List, &service, now)
            .await
            .expect("list");
        let lines: Vec<&str> = out.lines().collect();
        assert_eq!(lines.len(), 4, "{out}");
        assert!(lines[0].starts_with("CIDR "), "{out}");
        assert!(lines[1].starts_with("198.51.100.0/24  never "), "{out}");
        assert!(lines[2].contains("2026-10-13T00:00:00+00:00"), "{out}");
        assert!(
            lines[3].contains("2026-10-11T00:00:00+00:00 (expired)"),
            "{out}"
        );
        assert!(lines[3].ends_with("  old"), "{out}");

        let mut empty = MockIpBlockService::new();
        empty
            .expect_list()
            .returning(|_| Box::pin(async { Ok(vec![]) }));
        assert_eq!(
            execute_on(Command::List, &empty, now).await.expect("list"),
            "no IP blocks\n"
        );
    }
}
