//! CLI subcommands for the franking keys.
//!
//! ```text
//! rim_server franking-key show
//! rim_server franking-key rotate
//! ```
//!
//! A running server picks up a rotation within a minute, and at once for a report that names the
//! new key. Exit code 0 on success, 1 on any error, 2 on a usage error.

use chrono::Utc;

use rim_infra_services::franking_keys::{self, FrankingKeys, RETIRED_KEY_WINDOW};

const USAGE: &str = "usage: rim_server franking-key show | franking-key rotate";

/// A parsed subcommand.
#[derive(Debug, PartialEq, Eq)]
enum Command {
    Show,
    Rotate,
}

fn parse(mut args: impl Iterator<Item = String>) -> Result<Command, String> {
    let command = match args.next().as_deref() {
        Some("show") => Command::Show,
        Some("rotate") => Command::Rotate,
        _ => return Err(USAGE.to_string()),
    };
    match args.next() {
        Some(extra) => Err(format!("unknown argument: {extra}")),
        None => Ok(command),
    }
}

fn print_keys(keys: &FrankingKeys) {
    let now = Utc::now();
    let current = keys.current().id;
    let mut next_created = keys.keys().iter().skip(1).map(|k| k.created_at);
    for key in keys.keys() {
        let replaced = next_created.next();
        let state = match replaced {
            None if key.id == current => "signs new receipts".to_string(),
            Some(at) if keys.verifying(key.id, now).is_some() => format!(
                "verifies until {}",
                (at + RETIRED_KEY_WINDOW).format("%Y-%m-%dT%H:%M:%SZ")
            ),
            _ => "no longer verifies; the next rotation removes it".to_string(),
        };
        println!(
            "key {:<4} created {}  {state}",
            key.id,
            key.created_at.format("%Y-%m-%dT%H:%M:%SZ")
        );
    }
}

/// Run `rim_server franking-key …`. Returns the exit code.
pub fn run(args: impl Iterator<Item = String>) -> i32 {
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
    let path = config.franking_key_file();
    let result = match command {
        Command::Show => franking_keys::read_key_file(&path),
        Command::Rotate => franking_keys::rotate(&path, Utc::now()),
    };
    match result {
        Ok(keys) => {
            if command == Command::Rotate {
                println!(
                    "rotated; a running server signs with key {} within a minute",
                    keys.current().id
                );
            }
            println!("key file: {}", path.display());
            print_keys(&keys);
            0
        }
        Err(e) => {
            eprintln!("error: {e:#}");
            1
        }
    }
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
    fn parses_the_subcommands() {
        assert_eq!(parse(args(&["show"])), Ok(Command::Show));
        assert_eq!(parse(args(&["rotate"])), Ok(Command::Rotate));
        assert!(parse(args(&[])).is_err());
        assert!(parse(args(&["rotate", "now"])).is_err());
        assert!(parse(args(&["delete"])).is_err());
    }
}
