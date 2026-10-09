use std::fmt;

use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,

    pub database_url: String,

    pub session_ttl_days: u32,

    pub enable_graphiql: bool,

    pub cors_allowed_origin: Option<String>,

    pub data_dir: String,

    pub storage_local_path: Option<String>,
    pub max_workspace_storage_bytes: Option<u64>,

    /// Bootstrap token TTL in seconds. Default: 900 (15 minutes). Hard cap: 3600 (1 hour).
    /// Controlled by `RIM_BOOTSTRAP_TOKEN_TTL_SECONDS`.
    pub bootstrap_token_ttl_seconds: Option<u64>,

    /// Set `RIM_OPEN_REGISTRATION=true` to enable open signup.
    pub open_registration: Option<bool>,

    /// When `true`, the server reads `X-Forwarded-For` / `Forwarded` headers
    /// to determine the real client IP for rate-limiting. Only enable this
    /// when a trusted reverse proxy (Nginx, Caddy, Traefik, …) is the sole
    /// entry point; enabling it without a proxy lets attackers spoof their IP
    /// and defeat the rate limiters (H1 hardening).
    ///
    /// Controlled by `RIM_TRUST_FORWARDED_HEADERS=true`.
    pub trust_forwarded_headers: Option<bool>,

    /// When `true`, all session and CSRF cookies are issued with `Secure=true`
    /// regardless of the incoming request scheme or `X-Forwarded-Proto` header.
    /// Use this for direct-HTTPS deployments where the reverse proxy does not
    /// forward the `X-Forwarded-Proto` header (H3 hardening).
    ///
    /// Controlled by `RIM_FORCE_SECURE_COOKIES=true`.
    pub force_secure_cookies: Option<bool>,

    /// Max alert-ingest webhook requests accepted per ingest token per minute.
    /// Default: 120. Controlled by `RIM_ALERT_RATE_LIMIT_PER_MIN`.
    pub alert_rate_limit_per_min: Option<u32>,

    /// Max accepted body size (bytes) for alert-ingest webhook requests.
    /// Default: 262144 (256 KiB). Controlled by `RIM_ALERT_MAX_BODY_BYTES`.
    pub alert_max_body_bytes: Option<usize>,

    /// Ingest-token TTL in seconds. If unset, tokens don't expire (revocation only).
    ///
    /// `workspace_ingest_token` has no `expires_at` column (see
    /// `m20260708_232445_create_workspace_ingest_token`) — this is a single, instance-wide
    /// TTL applied at authentication time as `now - token.created_at > ttl`, not a per-token
    /// persisted expiry. Enforced in `DefaultIngestTokenService::authenticate_token`.
    /// Controlled by `RIM_INGEST_TOKEN_TTL_SECONDS`.
    pub ingest_token_ttl_seconds: Option<u64>,

    // ── Track B3: transactional email (RIM_SMTP_*) ──────────────────────────
    //
    // SMTP is only enabled when both `smtp_host` and `smtp_from` are present (see
    // `SmtpConfig::from_parts` in `rim_infra_services::email`); otherwise every send falls
    // back to `LogEmailSender` (log-only, no delivery). `smtp_username`/`smtp_password` may be
    // absent (unauthenticated relay / localhost dev SMTP). See `docs/email.md`.
    pub smtp_host: Option<String>,
    /// Defaults to 587 (STARTTLS) when unset.
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// The `From:` address/mailbox, e.g. `"RIM <noreply@example.com>"`.
    pub smtp_from: Option<String>,
    /// `"starttls"` (default) | `"implicit"` | `"none"` (dev/localhost only).
    pub smtp_tls: Option<String>,
    /// e.g. `https://rim.example.com`. Required to build clickable reset-password/invite/
    /// incident links; without it, emails fall back to including the bare token/no link.
    pub public_base_url: Option<String>,

    /// Floor sleep duration (seconds) for the escalation timer driver task when no run is
    /// currently armed. Default: 5. The driver actually `sleep`s until the next armed
    /// deadline (a `sleep_until`-style wait, not fixed-interval polling) — this value is only a
    /// floor for the "nothing armed" idle case, per §11. Controlled by
    /// `RIM_ESCALATION_TIMER_RESOLUTION_SECS`.
    pub escalation_timer_resolution_secs: Option<u64>,

    /// How often (seconds) the notification digest job looks for due digests (Track 1.3).
    /// Default: 900 (15 minutes); values below 60 are raised to 60. Controlled by
    /// `RIM_NOTIFICATION_DIGEST_INTERVAL_SECS`.
    pub notification_digest_interval_secs: Option<u64>,

    /// Comma-separated `host[:port]` values a key login proof may name as its audience
    /// (`docs/specs/key-identity-v1.md`). Controlled by `RIM_KEY_LOGIN_AUDIENCES`; see
    /// [`RIMConfig::key_login_audiences`] for the defaults.
    pub key_login_audiences: Option<String>,
}

/// The registration mode resolved from environment at boot.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegistrationMode {
    /// Default: signup requires a valid token.
    InviteOnly,
    /// `RIM_OPEN_REGISTRATION=true`: signup accepts no token; rate-limited.
    Open,
}

impl fmt::Debug for RIMConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RIMConfig")
            .field("server_host", &self.server_host)
            .field("server_port", &self.server_port)
            .field("database_url", &"[REDACTED]")
            .field("session_ttl_days", &self.session_ttl_days)
            .field("enable_graphiql", &self.enable_graphiql)
            .field("cors_allowed_origin", &self.cors_allowed_origin)
            .field("data_dir", &self.data_dir)
            .field(
                "trust_forwarded_headers",
                &self.trust_forwarded_headers.unwrap_or(false),
            )
            .field(
                "force_secure_cookies",
                &self.force_secure_cookies.unwrap_or(false),
            )
            .field("alert_rate_limit_per_min", &self.alert_rate_limit_per_min())
            .field("alert_max_body_bytes", &self.alert_max_body_bytes())
            .field("smtp_host", &self.smtp_host)
            .field("smtp_port", &self.smtp_port)
            .field("smtp_username", &self.smtp_username)
            .field(
                "smtp_password",
                &self.smtp_password.as_ref().map(|_| "[REDACTED]"),
            )
            .field("smtp_from", &self.smtp_from)
            .field("smtp_tls", &self.smtp_tls)
            .field("public_base_url", &self.public_base_url)
            .field(
                "escalation_timer_resolution_secs",
                &self.escalation_timer_resolution_secs(),
            )
            .field(
                "notification_digest_interval_secs",
                &self.notification_digest_interval_secs(),
            )
            .field("key_login_audiences", &self.key_login_audiences())
            .finish()
    }
}

impl Default for RIMConfig {
    fn default() -> Self {
        Self {
            server_host: "0.0.0.0".to_string(),
            server_port: 8080,
            database_url: "".to_string(),
            session_ttl_days: 30,
            enable_graphiql: false,
            cors_allowed_origin: None,
            data_dir: "./data".to_string(),
            storage_local_path: None,
            max_workspace_storage_bytes: Some(1_073_741_824), // 1 GB default
            bootstrap_token_ttl_seconds: None,
            open_registration: None,
            trust_forwarded_headers: None,
            force_secure_cookies: None,
            alert_rate_limit_per_min: None,
            alert_max_body_bytes: None,
            ingest_token_ttl_seconds: None,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_from: None,
            smtp_tls: None,
            public_base_url: None,
            escalation_timer_resolution_secs: None,
            notification_digest_interval_secs: None,
            key_login_audiences: None,
        }
    }
}

impl RIMConfig {
    /// Resolve the bootstrap token TTL.
    ///
    /// Returns the configured value capped at 3600 seconds. If the configured
    /// value exceeds the cap, this returns an error so `load_config` fails closed.
    ///
    /// # Errors
    /// Returns an error string if the configured TTL exceeds the 3600-second cap.
    ///
    /// # Examples
    /// ```ignore
    /// let ttl = config.bootstrap_token_ttl()?;
    /// ```
    pub fn bootstrap_token_ttl(&self) -> Result<u64, String> {
        const DEFAULT_TTL: u64 = 900; // 15 minutes
        const MAX_TTL: u64 = 3600; // 1 hour

        let ttl = self.bootstrap_token_ttl_seconds.unwrap_or(DEFAULT_TTL);
        if ttl > MAX_TTL {
            return Err(format!(
                "RIM_BOOTSTRAP_TOKEN_TTL_SECONDS={ttl} exceeds the hard cap of {MAX_TTL}; \
                 reduce the value or omit it to use the default (900s)"
            ));
        }
        Ok(ttl)
    }

    /// Resolve the registration mode from the environment.
    ///
    /// # Examples
    /// ```ignore
    /// let mode = config.registration_mode();
    /// ```
    pub fn registration_mode(&self) -> RegistrationMode {
        if self.open_registration.unwrap_or(false) {
            RegistrationMode::Open
        } else {
            RegistrationMode::InviteOnly
        }
    }

    /// Whether the server should trust `X-Forwarded-For` / `Forwarded` headers
    /// for real-IP resolution.
    ///
    /// Returns `true` only when `RIM_TRUST_FORWARDED_HEADERS=true` is set.
    /// **Operator requirement:** only enable when a trusted reverse proxy is
    /// the sole entry point. Enabling without a proxy lets clients spoof their
    /// IP and defeat per-IP rate limits (H1 hardening).
    ///
    /// # Examples
    /// ```ignore
    /// if config.trust_forwarded_headers() {
    ///     // read X-Forwarded-For
    /// }
    /// ```
    pub fn trust_forwarded_headers(&self) -> bool {
        self.trust_forwarded_headers.unwrap_or(false)
    }

    /// Whether to unconditionally set the `Secure` cookie flag.
    ///
    /// Returns `true` when `RIM_FORCE_SECURE_COOKIES=true` is set.
    /// Use this for direct-HTTPS deployments (Tauri WebView, Caddy/Traefik
    /// without `X-Forwarded-Proto`) where the scheme cannot be inferred from
    /// headers (H3 hardening).
    ///
    /// # Examples
    /// ```ignore
    /// if config.force_secure_cookies() {
    ///     // always set Secure flag
    /// }
    /// ```
    pub fn force_secure_cookies(&self) -> bool {
        self.force_secure_cookies.unwrap_or(false)
    }

    /// Max alert-ingest webhook requests accepted per ingest token per minute.
    /// Defaults to 120 if unset.
    ///
    /// # Examples
    /// ```ignore
    /// let per_min = config.alert_rate_limit_per_min();
    /// ```
    pub fn alert_rate_limit_per_min(&self) -> u32 {
        const DEFAULT: u32 = 120;
        self.alert_rate_limit_per_min.unwrap_or(DEFAULT)
    }

    /// Max accepted body size (bytes) for alert-ingest webhook requests.
    /// Defaults to 262144 (256 KiB) if unset.
    ///
    /// # Examples
    /// ```ignore
    /// let max_bytes = config.alert_max_body_bytes();
    /// ```
    pub fn alert_max_body_bytes(&self) -> usize {
        const DEFAULT: usize = 262_144;
        self.alert_max_body_bytes.unwrap_or(DEFAULT)
    }

    /// Ingest-token TTL, if the operator has opted into time-based expiry.
    /// `None` means tokens never expire (revocation only) — the default.
    ///
    /// # Examples
    /// ```ignore
    /// let ttl = config.ingest_token_ttl_seconds();
    /// ```
    pub fn ingest_token_ttl_seconds(&self) -> Option<u64> {
        self.ingest_token_ttl_seconds
    }

    /// Resolve the SMTP configuration from `RIM_SMTP_*`. Returns `None` (fall back to
    /// `LogEmailSender`) unless both `smtp_host` and `smtp_from` are present — see
    /// `rim_infra_services::email::SmtpConfig::from_parts` and `docs/email.md`.
    ///
    /// # Examples
    /// ```ignore
    /// let smtp = config.smtp_config();
    /// ```
    pub fn smtp_config(&self) -> Option<rim_infra_services::email::SmtpConfig> {
        rim_infra_services::email::SmtpConfig::from_parts(
            self.smtp_host.clone(),
            self.smtp_port,
            self.smtp_username.clone(),
            self.smtp_password.clone(),
            self.smtp_from.clone(),
            self.smtp_tls.clone(),
        )
    }

    /// e.g. `https://rim.example.com`. Required to build clickable password-reset/invite/
    /// incident links in transactional emails; without it, those emails fall back to a bare
    /// token or no link (see `docs/email.md`).
    ///
    /// # Examples
    /// ```ignore
    /// let base_url = config.public_base_url();
    /// ```
    pub fn public_base_url(&self) -> Option<String> {
        self.public_base_url.clone()
    }

    /// Floor sleep duration (seconds) for the escalation timer driver's idle case (nothing
    /// currently armed). Defaults to 5 if unset.
    ///
    /// # Examples
    /// ```ignore
    /// let floor_secs = config.escalation_timer_resolution_secs();
    /// ```
    pub fn escalation_timer_resolution_secs(&self) -> u64 {
        const DEFAULT: u64 = 5;
        self.escalation_timer_resolution_secs.unwrap_or(DEFAULT)
    }

    /// Interval (seconds) of the notification digest job. Defaults to 900 (15 minutes); never
    /// less than 60 so a misconfiguration cannot hammer the database.
    ///
    /// # Examples
    /// ```ignore
    /// let every = std::time::Duration::from_secs(config.notification_digest_interval_secs());
    /// ```
    pub fn notification_digest_interval_secs(&self) -> u64 {
        const DEFAULT: u64 = 900;
        const MIN: u64 = 60;
        self.notification_digest_interval_secs
            .unwrap_or(DEFAULT)
            .max(MIN)
    }
}

impl RIMConfig {
    /// The audiences a key login proof may name: this server's `host[:port]` as clients reach
    /// it. A proof naming another server is refused, so a server that receives a proof cannot
    /// replay it elsewhere.
    ///
    /// `RIM_KEY_LOGIN_AUDIENCES` when set; otherwise the host of `RIM_PUBLIC_BASE_URL`;
    /// otherwise `localhost:PORT` and `127.0.0.1:PORT` for local development. Lowercased.
    ///
    /// # Examples
    /// ```ignore
    /// let audiences = config.key_login_audiences();
    /// ```
    pub fn key_login_audiences(&self) -> Vec<String> {
        let configured: Vec<String> = self
            .key_login_audiences
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        if !configured.is_empty() {
            return configured;
        }
        if let Some(authority) = self.public_base_url.as_deref().and_then(url_authority) {
            return vec![authority];
        }
        vec![
            format!("localhost:{}", self.server_port),
            format!("127.0.0.1:{}", self.server_port),
        ]
    }
}

/// The `host[:port]` of an absolute URL such as `https://rim.example.com/app`, lowercased.
fn url_authority(url: &str) -> Option<String> {
    let (_, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    (!authority.is_empty()).then(|| authority.to_ascii_lowercase())
}

impl RIMConfig {
    #[allow(clippy::missing_errors_doc)]
    pub fn load() -> Result<Self, ConfigError> {
        let c = Config::builder()
            .set_default("server_host", "0.0.0.0")?
            .set_default("server_port", 8080)?
            .set_default("session_ttl_days", 30)?
            .set_default("enable_graphiql", false)?
            .set_default("data_dir", "./data")?
            .set_default("max_workspace_storage_bytes", 1_073_741_824_i64)?
            .set_default("alert_rate_limit_per_min", 120_i64)?
            .set_default("alert_max_body_bytes", 262_144_i64)?
            .set_default("escalation_timer_resolution_secs", 5_i64)?
            .set_default("notification_digest_interval_secs", 900_i64)?
            .add_source(Environment::with_prefix("RIM"))
            .add_source(Environment::default());
        c.build()?.try_deserialize()
    }
}

pub fn load_config() -> Result<RIMConfig, ConfigError> {
    dotenv().ok();
    RIMConfig::load()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_rate_limit_per_min_defaults_to_120_when_unset() {
        let config = RIMConfig {
            alert_rate_limit_per_min: None,
            ..Default::default()
        };
        assert_eq!(config.alert_rate_limit_per_min(), 120);
    }

    #[test]
    fn alert_rate_limit_per_min_returns_configured_value() {
        let config = RIMConfig {
            alert_rate_limit_per_min: Some(240),
            ..Default::default()
        };
        assert_eq!(config.alert_rate_limit_per_min(), 240);
    }

    #[test]
    fn alert_max_body_bytes_defaults_to_256kib_when_unset() {
        let config = RIMConfig {
            alert_max_body_bytes: None,
            ..Default::default()
        };
        assert_eq!(config.alert_max_body_bytes(), 262_144);
    }

    #[test]
    fn alert_max_body_bytes_returns_configured_value() {
        let config = RIMConfig {
            alert_max_body_bytes: Some(65_536),
            ..Default::default()
        };
        assert_eq!(config.alert_max_body_bytes(), 65_536);
    }

    #[test]
    fn ingest_token_ttl_seconds_defaults_to_none() {
        let config = RIMConfig::default();
        assert_eq!(config.ingest_token_ttl_seconds(), None);
    }

    #[test]
    fn ingest_token_ttl_seconds_returns_configured_value() {
        let config = RIMConfig {
            ingest_token_ttl_seconds: Some(86_400),
            ..Default::default()
        };
        assert_eq!(config.ingest_token_ttl_seconds(), Some(86_400));
    }

    #[test]
    fn smtp_config_none_when_unconfigured() {
        let config = RIMConfig::default();
        assert!(config.smtp_config().is_none());
    }

    #[test]
    fn smtp_config_none_when_only_host_present() {
        let config = RIMConfig {
            smtp_host: Some("smtp.example.com".to_string()),
            ..Default::default()
        };
        assert!(config.smtp_config().is_none());
    }

    #[test]
    fn smtp_config_some_when_host_and_from_present() {
        let config = RIMConfig {
            smtp_host: Some("smtp.example.com".to_string()),
            smtp_from: Some("RIM <noreply@example.com>".to_string()),
            ..Default::default()
        };
        let smtp = config.smtp_config().unwrap();
        assert_eq!(smtp.host, "smtp.example.com");
        assert_eq!(smtp.port, 587);
    }

    #[test]
    fn debug_output_does_not_contain_smtp_password() {
        let config = RIMConfig {
            smtp_password: Some("super-secret-password".to_string()),
            ..Default::default()
        };
        let debug_output = format!("{config:?}");
        assert!(!debug_output.contains("super-secret-password"));
        assert!(debug_output.contains("[REDACTED]"));
    }

    #[test]
    fn key_login_audiences_parses_the_configured_list() {
        let config = RIMConfig {
            key_login_audiences: Some(" Rim.Example.com , relay.rim.chat:8443,, ".to_string()),
            public_base_url: Some("https://other.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.key_login_audiences(),
            vec!["rim.example.com", "relay.rim.chat:8443"]
        );
    }

    #[test]
    fn key_login_audiences_falls_back_to_public_base_url_host() {
        let config = RIMConfig {
            public_base_url: Some("https://user@RIM.example.com:8443/app?x=1".to_string()),
            ..Default::default()
        };
        assert_eq!(config.key_login_audiences(), vec!["rim.example.com:8443"]);
    }

    #[test]
    fn key_login_audiences_defaults_to_localhost_on_the_server_port() {
        let config = RIMConfig {
            server_port: 9000,
            key_login_audiences: Some(" ".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.key_login_audiences(),
            vec!["localhost:9000", "127.0.0.1:9000"]
        );
    }

    #[test]
    fn public_base_url_defaults_to_none() {
        let config = RIMConfig::default();
        assert_eq!(config.public_base_url(), None);
    }

    #[test]
    fn public_base_url_returns_configured_value() {
        let config = RIMConfig {
            public_base_url: Some("https://rim.example.com".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.public_base_url(),
            Some("https://rim.example.com".to_string())
        );
    }

    #[test]
    fn notification_digest_interval_defaults_to_fifteen_minutes() {
        let config = RIMConfig::default();
        assert_eq!(config.notification_digest_interval_secs(), 900);
    }

    #[test]
    fn notification_digest_interval_is_configurable_with_a_floor() {
        let config = RIMConfig {
            notification_digest_interval_secs: Some(300),
            ..Default::default()
        };
        assert_eq!(config.notification_digest_interval_secs(), 300);
        let config = RIMConfig {
            notification_digest_interval_secs: Some(5),
            ..Default::default()
        };
        assert_eq!(config.notification_digest_interval_secs(), 60);
    }
}
