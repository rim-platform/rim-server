//! CLI subcommand: `rim_server bootstrap-user`
//!
//! Issues a single-use bootstrap token and prints a signup URL to stdout.
//!
//! # Usage
//!
//! ```text
//! cargo run -p rim_server -- bootstrap-user --email <email> [--public-url <url>]
//! ```
//!
//! ## Outputs
//!
//! - **stdout**: one line — `https://<host>/join/<raw_token>`
//! - **stderr**: token id, expiry, and SHA-256 fingerprint (for audit correlation).
//!   The raw token is NOT printed to stderr.
//!
//! ## Exit codes
//!
//! - `0` on success.
//! - `1` on any error (config, DB, entropy failure).

use sea_orm::Database;

use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::model::NewBootstrapToken;
use rim_core::auth::repository::BootstrapTokenRepository;
use rim_infra_db::DbHandle;
use rim_infra_db::sea_orm_impl::bootstrap_token::SeaOrmBootstrapTokenRepository;

/// Parse the `bootstrap-user` subcommand arguments and run the command.
///
/// `args` must contain the remaining tokens after the `bootstrap-user` positional
/// argument has already been consumed by `main`.
///
/// Returns an exit code: `0` for success, `1` for any error.
///
/// # Examples
///
/// ```ignore
/// // Called from main after consuming the "bootstrap-user" token:
/// let exit_code = bootstrap_cmd::run(std::env::args().skip(2)).await;
/// ```
pub async fn run(args: impl Iterator<Item = String>) -> i32 {
    match run_inner(args).await {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("error: {e}");
            1
        }
    }
}

async fn run_inner(args: impl Iterator<Item = String>) -> Result<(), String> {
    let opts = parse_args(args)?;

    let config =
        crate::config::load_config().map_err(|e| format!("failed to load configuration: {e}"))?;

    // Validate the bootstrap token TTL from config (fails closed if out of range).
    let ttl_secs = config
        .bootstrap_token_ttl()
        .map_err(|e| format!("invalid RIM_BOOTSTRAP_TOKEN_TTL_SECONDS: {e}"))?;

    let db = Database::connect(&config.database_url)
        .await
        .map_err(|e| format!("failed to connect to database: {e}"))?;

    let handle: DbHandle = db.into();
    let repo = SeaOrmBootstrapTokenRepository::new(handle);

    let raw_token =
        generate_token_with_prefix("rim_boot_").map_err(|e| format!("entropy failure: {e}"))?;
    let token_hash = hash_token(&raw_token);
    let expires_at = chrono::Utc::now()
        + chrono::Duration::seconds(
            i64::try_from(ttl_secs).map_err(|_| "TTL overflow".to_string())?,
        );

    let token = repo
        .create(NewBootstrapToken {
            email: opts.email.clone(),
            token_hash: token_hash.clone(),
            expires_at,
        })
        .await
        .map_err(|e| format!("database error: {e}"))?;

    let base_url = opts
        .public_url
        .unwrap_or_else(|| format!("http://{}:{}", config.server_host, config.server_port));
    let signup_url = format!("{base_url}/join/{raw_token}");

    // stdout: the URL only — operator may pipe this directly.
    println!("{signup_url}");

    // stderr: audit metadata (NOT the raw token).
    // SHA-256 fingerprint for correlating this issuance with later consumption in audit logs.
    let fingerprint = token_fingerprint(&token_hash);
    eprintln!("bootstrap token issued");
    eprintln!("  token_id  : {}", token.id);
    eprintln!("  email     : {}", opts.email);
    eprintln!(
        "  expires_at: {}",
        token.expires_at.format("%Y-%m-%dT%H:%M:%SZ")
    );
    eprintln!("  sha256    : {fingerprint}");

    tracing::info!(
        target = "audit",
        event = "bootstrap_token_issued",
        token_id = %token.id,
        email = %opts.email,
        expires_at = %token.expires_at,
        sha256_prefix = %fingerprint,
        "bootstrap token issued via CLI"
    );

    Ok(())
}

/// Parsed options for the `bootstrap-user` subcommand.
#[derive(Debug)]
struct BootstrapUserOpts {
    email: String,
    public_url: Option<String>,
}

fn parse_args(mut args: impl Iterator<Item = String>) -> Result<BootstrapUserOpts, String> {
    let mut email: Option<String> = None;
    let mut public_url: Option<String> = None;

    while let Some(flag) = args.next() {
        match flag.as_str() {
            "--email" => {
                email = Some(args.next().ok_or("--email requires a value")?);
            }
            "--public-url" => {
                public_url = Some(args.next().ok_or("--public-url requires a value")?);
            }
            other => {
                return Err(format!("unknown argument: {other}"));
            }
        }
    }

    let email = email.ok_or("--email <email> is required")?;
    if email.is_empty() {
        return Err("--email must not be empty".to_string());
    }

    Ok(BootstrapUserOpts { email, public_url })
}

/// Generate a bootstrap token using the same primitive as session tokens but with
/// a `rim_boot_` prefix instead of `rim_sess_`.
fn generate_token_with_prefix(prefix: &str) -> Result<String, String> {
    let t = generate_session_token();
    let token = t
        .strip_prefix("rim_sess_")
        .map(|s| format!("{prefix}{s}"))
        .unwrap_or_else(|| format!("{prefix}{t}"));
    Ok(token)
}

/// Return the first 16 hex chars of the SHA-256 hash — enough for visual correlation
/// in audit logs without revealing the full hash (which is the DB credential).
fn token_fingerprint(hash: &str) -> String {
    hash.chars().take(16).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_args_email_required() {
        let args = vec![];
        let err = parse_args(args.into_iter()).unwrap_err();
        assert!(
            err.contains("--email"),
            "error should mention --email: {err}"
        );
    }

    #[test]
    fn parse_args_email_empty_rejected() {
        let args = vec!["--email".to_string(), "".to_string()];
        let err = parse_args(args.into_iter()).unwrap_err();
        assert!(
            err.contains("empty"),
            "error should mention empty email: {err}"
        );
    }

    #[test]
    fn parse_args_email_only() {
        let args = vec!["--email".to_string(), "ops@example.com".to_string()];
        let opts = parse_args(args.into_iter()).unwrap();
        assert_eq!(opts.email, "ops@example.com");
        assert!(opts.public_url.is_none());
    }

    #[test]
    fn parse_args_email_and_public_url() {
        let args = vec![
            "--email".to_string(),
            "ops@example.com".to_string(),
            "--public-url".to_string(),
            "https://rim.example.com".to_string(),
        ];
        let opts = parse_args(args.into_iter()).unwrap();
        assert_eq!(opts.email, "ops@example.com");
        assert_eq!(opts.public_url.as_deref(), Some("https://rim.example.com"));
    }

    #[test]
    fn parse_args_unknown_flag_rejected() {
        let args = vec![
            "--email".to_string(),
            "a@b.com".to_string(),
            "--unknown".to_string(),
        ];
        let err = parse_args(args.into_iter()).unwrap_err();
        assert!(
            err.contains("unknown"),
            "should mention unknown argument: {err}"
        );
    }

    #[test]
    fn generate_token_has_expected_prefix() {
        let t = generate_token_with_prefix("rim_boot_").unwrap();
        assert!(
            t.starts_with("rim_boot_"),
            "token should start with rim_boot_: {t}"
        );
    }

    #[test]
    fn token_fingerprint_returns_16_chars() {
        let hash = "abcdef1234567890abcdef";
        let fp = token_fingerprint(hash);
        assert_eq!(fp.len(), 16);
        assert_eq!(fp, "abcdef1234567890");
    }
}
