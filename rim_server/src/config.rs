use std::fmt;

use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;
use serde::Deserialize;

#[derive(Clone, Deserialize)]
#[allow(dead_code)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,

    pub database_url: String,

    pub bootstrap_secret: Option<String>,

    pub session_ttl_days: u32,

    pub enable_graphiql: bool,

    pub cors_allowed_origin: Option<String>,

    pub data_dir: String,

    pub storage_backend: Option<String>,
    pub storage_local_path: Option<String>,
    pub storage_s3_bucket: Option<String>,
    pub storage_s3_region: Option<String>,
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
            .field(
                "bootstrap_secret",
                &self.bootstrap_secret.as_ref().map(|_| "[REDACTED]"),
            )
            .field("session_ttl_days", &self.session_ttl_days)
            .field("enable_graphiql", &self.enable_graphiql)
            .field("cors_allowed_origin", &self.cors_allowed_origin)
            .field("data_dir", &self.data_dir)
            .field("storage_backend", &self.storage_backend)
            .field(
                "trust_forwarded_headers",
                &self.trust_forwarded_headers.unwrap_or(false),
            )
            .field(
                "force_secure_cookies",
                &self.force_secure_cookies.unwrap_or(false),
            )
            .finish()
    }
}

impl Default for RIMConfig {
    fn default() -> Self {
        Self {
            server_host: "0.0.0.0".to_string(),
            server_port: 8080,
            database_url: "".to_string(),
            bootstrap_secret: None,
            session_ttl_days: 30,
            enable_graphiql: false,
            cors_allowed_origin: None,
            data_dir: "./data".to_string(),
            storage_backend: Some("local".to_string()),
            storage_local_path: None,
            storage_s3_bucket: None,
            storage_s3_region: None,
            max_workspace_storage_bytes: Some(1_073_741_824), // 1 GB default
            bootstrap_token_ttl_seconds: None,
            open_registration: None,
            trust_forwarded_headers: None,
            force_secure_cookies: None,
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
            .set_default("storage_backend", "local")?
            .set_default("max_workspace_storage_bytes", 1_073_741_824_i64)?
            .add_source(Environment::with_prefix("RIM"))
            .add_source(Environment::default());
        c.build()?.try_deserialize()
    }
}

pub fn load_config() -> Result<RIMConfig, ConfigError> {
    dotenv().ok();
    RIMConfig::load()
}
