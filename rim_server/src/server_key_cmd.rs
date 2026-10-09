//! CLI subcommands for the server key and pinned peers.
//!
//! ```text
//! rim_server server-key show
//! rim_server server-key rotate
//! rim_server server-key recover --recovery-key-file <file> [--new-recovery-key]
//! rim_server peer reset <server-id>
//! ```
//!
//! A running server picks up a rotation or recovery within a minute. Exit code 0 on success,
//! 1 on any error, 2 on a usage error.

use std::path::PathBuf;
use std::sync::Arc;

use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::s2s::{SeaOrmPeerServerRepository, SeaOrmServerIdentityRepository};
use rim_infra_services::s2s::server_key::{self, LoadedKey};
use rim_infra_services::s2s::{DefaultPeerService, HttpDiscoveryFetcher};
use rim_s2s::key_hash;
use rim_services::s2s::service::PeerService;
use sea_orm::Database;

const USAGE: &str = "usage: rim_server server-key show | server-key rotate | \
     server-key recover --recovery-key-file <file> [--new-recovery-key] | peer reset <server-id>";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Show,
    Rotate,
    Recover {
        recovery_key_file: PathBuf,
        new_recovery_key: bool,
    },
    PeerReset {
        server_id: String,
    },
}

fn parse(group: &str, mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let action = args.next().ok_or(USAGE)?;
    let command = match (group, action.as_str()) {
        ("server-key", "show") => Command::Show,
        ("server-key", "rotate") => Command::Rotate,
        ("server-key", "recover") => {
            let mut file = None;
            let mut new_recovery_key = false;
            while let Some(flag) = args.next() {
                match flag.as_str() {
                    "--recovery-key-file" => {
                        file = Some(PathBuf::from(
                            args.next().ok_or("--recovery-key-file requires a value")?,
                        ));
                    }
                    "--new-recovery-key" => new_recovery_key = true,
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            return Ok(Command::Recover {
                recovery_key_file: file.ok_or("--recovery-key-file <file> is required")?,
                new_recovery_key,
            });
        }
        ("peer", "reset") => {
            let server_id = args.next().ok_or("peer reset requires a server id")?;
            if !rim_s2s::is_server_id(&server_id) {
                return Err(format!("{server_id} is not a server id"));
            }
            Command::PeerReset { server_id }
        }
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

fn print_key(loaded: &LoadedKey) {
    println!("server id    : {}", loaded.state.server_id);
    println!("key seq      : {}", loaded.state.seq);
    println!("server key   : {}", loaded.state.server_key);
    println!("key hash     : {}", key_hash(&loaded.state.server_key));
    println!("recovery key : {}", loaded.state.recovery_key);
}

/// Run `rim_server <group> …`, where `group` is `server-key` or `peer`. Returns the exit code.
pub async fn run(group: &str, args: impl Iterator<Item = String>) -> i32 {
    let command = match parse(group, args) {
        Ok(command) => command,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    match run_command(command).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn run_command(command: Command) -> Result<(), String> {
    let config =
        crate::config::load_config().map_err(|e| format!("failed to load configuration: {e}"))?;
    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;
    let handle: DbHandle = db.into();
    let files = config.key_files();
    let repo = SeaOrmServerIdentityRepository::new(handle.clone());
    match command {
        Command::Show => {
            let loaded = server_key::load(&repo, &files)
                .await
                .map_err(|e| format!("{e:#}"))?;
            print_key(&loaded);
            println!("key file     : {}", files.key_file.display());
            if files.recovery_file().exists() {
                println!(
                    "warning      : {} is still next to the server key; move it offline",
                    files.recovery_file().display()
                );
            }
        }
        Command::Rotate => {
            let loaded = server_key::rotate(&repo, &files)
                .await
                .map_err(|e| format!("{e:#}"))?;
            println!("rotated; a running server picks the new key up within a minute");
            print_key(&loaded);
        }
        Command::Recover {
            recovery_key_file,
            new_recovery_key,
        } => {
            let (loaded, written) =
                server_key::recover(&repo, &files, &recovery_key_file, new_recovery_key)
                    .await
                    .map_err(|e| format!("{e:#}"))?;
            println!("recovered; peers stop accepting the previous server key at once");
            print_key(&loaded);
            if let Some(path) = written {
                println!(
                    "new recovery key written to {}; move it offline",
                    path.display()
                );
            }
        }
        Command::PeerReset { server_id } => {
            let fetcher = HttpDiscoveryFetcher::new(config.s2s_allow_private_addresses())
                .map_err(|e| e.to_string())?;
            let peers = DefaultPeerService::new(
                Arc::new(SeaOrmPeerServerRepository::new(handle)),
                Arc::new(fetcher),
                config.s2s_allow_http(),
            );
            let existed = peers.reset(&server_id).await.map_err(|e| e.to_string())?;
            if existed {
                println!("forgot {server_id}; its next request pins it afresh");
            } else {
                println!("{server_id} was not pinned");
            }
        }
    }
    Ok(())
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

    #[test]
    fn parses_every_subcommand() {
        assert_eq!(parse("server-key", args(&["show"])), Ok(Command::Show));
        assert_eq!(parse("server-key", args(&["rotate"])), Ok(Command::Rotate));
        assert_eq!(
            parse(
                "server-key",
                args(&[
                    "recover",
                    "--recovery-key-file",
                    "/k.txt",
                    "--new-recovery-key"
                ])
            ),
            Ok(Command::Recover {
                recovery_key_file: PathBuf::from("/k.txt"),
                new_recovery_key: true,
            })
        );
        let id = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
        assert_eq!(
            parse("peer", args(&["reset", id])),
            Ok(Command::PeerReset {
                server_id: id.to_string()
            })
        );
    }

    #[test]
    fn refuses_bad_usage() {
        assert!(parse("server-key", args(&[])).is_err());
        assert!(parse("server-key", args(&["show", "extra"])).is_err());
        assert!(parse("server-key", args(&["recover"])).is_err());
        assert!(parse("server-key", args(&["recover", "--bogus"])).is_err());
        assert!(parse("peer", args(&["reset", "relay.example"])).is_err());
        assert!(parse("peer", args(&["forget", "x"])).is_err());
    }
}
