use std::fmt;

use config::{Config, ConfigError, Environment};
use dotenvy::dotenv;
use rim_services::auth::dto::RegistrationMode;
use serde::Deserialize;

/// The release version of this build: the workspace version every crate carries.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");

/// What `rim-server --version` prints.
pub fn version_line() -> String {
    format!("rim-server {VERSION}")
}

#[derive(Clone, Deserialize)]
pub struct RIMConfig {
    pub server_host: String,
    pub server_port: u16,

    pub database_url: String,
    /// Largest number of Postgres connections the server's pool opens
    /// (`RIM_DATABASE_MAX_CONNECTIONS`); unset keeps the driver's default of 10.
    pub database_max_connections: Option<u32>,

    pub session_ttl_days: u32,

    pub enable_graphiql: bool,

    pub cors_allowed_origin: Option<String>,

    pub data_dir: String,

    pub storage_local_path: Option<String>,
    pub max_workspace_storage_bytes: Option<u64>,

    /// Set `RIM_OPEN_REGISTRATION=true` to enable open signup.
    pub open_registration: Option<bool>,

    /// When `true`, the server reads `X-Forwarded-For` / `Forwarded` headers
    /// to determine the real client IP for rate-limiting. Only enable this
    /// when a trusted reverse proxy (Nginx, Caddy, Traefik, …) is the sole
    /// entry point; enabling it without a proxy lets attackers spoof their IP
    /// and defeat the rate limiters.
    ///
    /// Controlled by `RIM_TRUST_FORWARDED_HEADERS=true`.
    pub trust_forwarded_headers: Option<bool>,

    /// When `true`, all session and CSRF cookies are issued with `Secure=true`
    /// regardless of the incoming request scheme or `X-Forwarded-Proto` header.
    /// Use this for direct-HTTPS deployments where the reverse proxy does not
    /// forward the `X-Forwarded-Proto` header.
    ///
    /// Controlled by `RIM_FORCE_SECURE_COOKIES=true`.
    pub force_secure_cookies: Option<bool>,

    // ── Transactional email (RIM_SMTP_*) ────────────────────────────────────
    //
    // SMTP is only enabled when both `smtp_host` and `smtp_from` are present (see
    // `SmtpConfig::from_parts` in `rim_infra_services::email`); otherwise every send falls
    // back to `LogEmailSender` (log-only, no delivery). `smtp_username`/`smtp_password` may be
    // absent (unauthenticated relay / localhost dev SMTP).
    pub smtp_host: Option<String>,
    /// Defaults to 587 (STARTTLS) when unset.
    pub smtp_port: Option<u16>,
    pub smtp_username: Option<String>,
    pub smtp_password: Option<String>,
    /// The `From:` address/mailbox, e.g. `"RIM <noreply@example.com>"`.
    pub smtp_from: Option<String>,
    /// `"starttls"` (default) | `"implicit"` | `"none"` (dev/localhost only).
    pub smtp_tls: Option<String>,
    /// e.g. `https://rim.example.com`. Required to build clickable reset-password/invite
    /// links; without it, emails fall back to including the bare token/no link.
    pub public_base_url: Option<String>,

    /// How often (seconds) the notification digest job looks for due digests.
    /// Default: 900 (15 minutes); values below 60 are raised to 60. Controlled by
    /// `RIM_NOTIFICATION_DIGEST_INTERVAL_SECS`.
    pub notification_digest_interval_secs: Option<u64>,

    /// Comma-separated `host[:port]` values a key login proof may name as its audience.
    /// Controlled by `RIM_KEY_LOGIN_AUDIENCES`; see
    /// [`RIMConfig::key_login_audiences`] for the defaults.
    pub key_login_audiences: Option<String>,

    /// `relay`, `host` or `both` (default). Controlled by `RIM_ROLE`.
    pub role: Option<String>,

    /// Path of the server key file. Default `<RIM_DATA_DIR>/server-key.json`. Controlled by
    /// `RIM_SERVER_KEY_FILE`.
    pub server_key_file: Option<String>,

    /// Path of the franking key file. Default `<RIM_DATA_DIR>/franking-key.json`. Controlled by
    /// `RIM_FRANKING_KEY_FILE`.
    pub franking_key_file: Option<String>,

    /// Comma-separated server ids allowed on host endpoints; empty allows any relay.
    /// Controlled by `RIM_HOST_ALLOWED_RELAYS`.
    pub host_allowed_relays: Option<String>,

    /// Comma-separated server ids refused on every server-to-server endpoint. Controlled by
    /// `RIM_S2S_DENIED_PEERS`.
    pub s2s_denied_peers: Option<String>,

    /// Server-to-server requests accepted per peer per minute, per node. Default 600.
    /// Controlled by `RIM_S2S_PEER_RATE_PER_MIN`.
    pub s2s_peer_rate_per_min: Option<u32>,

    /// Largest server-to-server request body. Default 8 MiB. Controlled by
    /// `RIM_S2S_MAX_BODY_BYTES`.
    pub s2s_max_body_bytes: Option<usize>,

    /// Allow peers on loopback, private, link-local and unique-local addresses (development
    /// and tests only). Controlled by `RIM_S2S_ALLOW_PRIVATE_ADDRESSES`.
    pub s2s_allow_private_addresses: Option<bool>,

    /// Allow server-to-server traffic over plain `http` (development and tests only).
    /// Controlled by `RIM_S2S_ALLOW_HTTP`.
    pub s2s_allow_http: Option<bool>,

    /// Forwarded requests accepted per device per minute. Default 120. Controlled by
    /// `RIM_S2S_DEVICE_RATE_PER_MIN`.
    pub s2s_device_rate_per_min: Option<u32>,

    /// Active delegations one relay may hold on this host. Default 10,000. Controlled by
    /// `RIM_S2S_SUBSCRIPTIONS_PER_PEER`.
    pub s2s_subscriptions_per_peer: Option<u32>,

    /// Bytes queued per local user before the oldest items are dropped. Default 256 MiB.
    /// Controlled by `RIM_RELAY_QUEUE_BYTES_PER_USER`.
    pub relay_queue_bytes_per_user: Option<i64>,

    /// Days queued items and DM requests are kept. Default 30. Controlled by
    /// `RIM_RELAY_RETENTION_DAYS`.
    pub relay_retention_days: Option<u32>,

    /// Leading zero bits a key-only signup's proof-of-work needs; set, it turns on `pow`
    /// registration. 1 to 40. Controlled by `RIM_REGISTRATION_POW_BITS`.
    pub registration_pow_bits: Option<u32>,

    /// Largest profile card, in bytes. Default and most 131072. Controlled by
    /// `RIM_RELAY_CARD_MAX_BYTES`.
    pub relay_card_max_bytes: Option<usize>,

    /// Unclaimed one-time key packages per device and cipher suite. Default 100. Controlled by
    /// `RIM_RELAY_KEY_PACKAGES_PER_DEVICE`.
    pub relay_key_packages_per_device: Option<i64>,

    /// Devices that are not revoked, per user. Default 32. Controlled by
    /// `RIM_RELAY_DEVICES_PER_USER`.
    pub relay_devices_per_user: Option<u32>,

    /// Requests a user sends through `/api/remote/…` per day. Default 50000. Controlled by
    /// `RIM_RELAY_FORWARDS_PER_USER_PER_DAY`.
    pub relay_forwards_per_user_per_day: Option<u32>,

    /// DM requests one sender principal files here per day, and invites a local user sends to
    /// people on other servers per day. Default 20. Controlled by
    /// `RIM_RELAY_DM_REQUESTS_PER_DAY`.
    pub relay_dm_requests_per_day: Option<u32>,

    /// `host:port` of the Prometheus listener serving `GET /metrics`; unset turns metrics off.
    /// Controlled by `RIM_METRICS_ADDR`.
    pub metrics_addr: Option<String>,

    /// Days command audit log rows are kept before the daily purge exports and deletes them;
    /// unset keeps them forever. 1 to 3650. Controlled by `RIM_AUDIT_RETENTION_DAYS`.
    pub audit_retention_days: Option<u32>,

    /// Comma-separated optional features switched off on this instance, e.g.
    /// `attachments,custom_emoji`; unset or empty keeps every feature on. Controlled by
    /// `RIM_FEATURES_DISABLED`; see [`RIMConfig::features_disabled`].
    pub features_disabled: Option<String>,

    /// Requests one client address sends to `/api` routes per minute, with a burst of the same
    /// size; unset or 0 sets no limit. Controlled by `RIM_RATE_REQUESTS_PER_MIN_PER_IP`.
    pub rate_requests_per_min_per_ip: Option<u32>,
    /// Mutation fields one session runs per minute, counted per GraphQL request; callers
    /// without a session count per address. Unset or 0 sets no limit. Controlled by
    /// `RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION`.
    pub rate_mutations_per_min_per_session: Option<u32>,
    /// Messages one user sends per minute; unset or 0 sets no limit. Controlled by
    /// `RIM_RATE_MESSAGES_PER_MIN_PER_USER`.
    pub rate_messages_per_min_per_user: Option<u32>,
    /// Messages one user may send at once before the per-minute rate applies; unset uses the
    /// per-minute number. Controlled by `RIM_RATE_MESSAGES_BURST`.
    pub rate_messages_burst: Option<u32>,
    /// WebSocket connections one user holds open at once; unset or 0 sets no limit.
    /// Controlled by `RIM_RATE_WS_CONNECTIONS_PER_USER`.
    pub rate_ws_connections_per_user: Option<u32>,
    /// Subscriptions one WebSocket connection holds at once; unset or 0 sets no limit.
    /// Controlled by `RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION`.
    pub rate_subscriptions_per_connection: Option<u32>,
    /// Most GraphQL requests in one batch. Default 20. Controlled by `RIM_API_MAX_BATCH`.
    pub api_max_batch: Option<usize>,
    /// A short text shown to people on this instance, at most 500 characters. Controlled by
    /// `RIM_INSTANCE_NOTICE`; see [`RIMConfig::instance_notice`].
    pub instance_notice: Option<String>,

    /// The terms of service every signup accepts, an absolute http(s) URL. Controlled by
    /// `RIM_TERMS_URL`; see [`RIMConfig::terms_url`].
    pub terms_url: Option<String>,

    // ── Instance limits (RIM_LIMIT_*) ───────────────────────────────────────
    //
    // Each caps one thing across the instance; unset or 0 sets no cap. See
    // [`RIMConfig::instance_limits`].
    /// Workspaces one user owns. Controlled by `RIM_LIMIT_WORKSPACES_PER_USER`.
    pub limit_workspaces_per_user: Option<u32>,
    /// Members of one workspace; the direct space has no cap. Controlled by
    /// `RIM_LIMIT_MEMBERS_PER_WORKSPACE`.
    pub limit_members_per_workspace: Option<u32>,
    /// Channels of one workspace, public and private. Controlled by
    /// `RIM_LIMIT_CHANNELS_PER_WORKSPACE`.
    pub limit_channels_per_workspace: Option<u32>,
    /// Devices on one account; the stricter of this and `RIM_RELAY_DEVICES_PER_USER` applies.
    /// Controlled by `RIM_LIMIT_DEVICES_PER_USER`.
    pub limit_devices_per_user: Option<u32>,
    /// DM requests one user sends to strangers per day; the stricter of this and
    /// `RIM_RELAY_DM_REQUESTS_PER_DAY` applies. Controlled by `RIM_LIMIT_DM_REQUESTS_PER_DAY`.
    pub limit_dm_requests_per_day: Option<u32>,
    /// Macros of one workspace. Controlled by `RIM_LIMIT_MACROS_PER_WORKSPACE`.
    pub limit_macros_per_workspace: Option<u32>,
    /// Pipelines of one workspace. Controlled by `RIM_LIMIT_PIPELINES_PER_WORKSPACE`.
    pub limit_pipelines_per_workspace: Option<u32>,
    /// Plugins of one workspace's registry. Controlled by `RIM_LIMIT_PLUGINS_PER_WORKSPACE`.
    pub limit_plugins_per_workspace: Option<u32>,
    /// Invites one user creates per day. Controlled by `RIM_LIMIT_INVITES_PER_USER_PER_DAY`.
    pub limit_invites_per_user_per_day: Option<u32>,
    /// Uses one invite may have. Controlled by `RIM_LIMIT_INVITE_MAX_USES`.
    pub limit_invite_max_uses: Option<u32>,
    /// Bytes of one message's text, at most 65536. Controlled by `RIM_LIMIT_MESSAGE_BYTES`.
    pub limit_message_bytes: Option<u32>,

    // ── Probation of new accounts ───────────────────────────────────────────
    /// Hours after its creation during which an account cannot create a workspace or send a
    /// DM request to a stranger; unset or 0 turns probation off. Controlled by
    /// `RIM_PROBATION_HOURS`.
    pub probation_hours: Option<u32>,
    /// Messages an account in probation sends per minute, with a burst of the same size,
    /// instead of the normal message rate; unset or 0 keeps the normal rate. Controlled by
    /// `RIM_RATE_PROBATION_MESSAGES_PER_MIN`.
    pub rate_probation_messages_per_min: Option<u32>,
    /// Days between an account's deletion request and its purge, 0 to 30; default 14.
    /// Controlled by `RIM_ACCOUNT_DELETION_GRACE_DAYS`.
    pub account_deletion_grace_days: Option<u32>,
    /// Comma-separated usernames and display names signups and profile edits refuse, with
    /// each followed by `-`, `_`, `.` or a space. Controlled by `RIM_RESERVED_NAMES`; see
    /// [`RIMConfig::reserved_names`].
    pub reserved_names: Option<String>,
}

/// Which parts of the server-to-server protocol this server runs (`RIM_ROLE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerRole {
    /// The server of its users' apps, with a direct space for their DMs.
    Relay,
    /// The authority for its workspaces.
    Host,
    /// Both: a single server is its users' relay and hosts its workspaces.
    Both,
}

impl ServerRole {
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "relay" => Some(Self::Relay),
            "host" => Some(Self::Host),
            "both" => Some(Self::Both),
            _ => None,
        }
    }

    /// The roles the descriptor lists, in its order.
    pub fn roles(self) -> Vec<rim_s2s::Role> {
        match self {
            Self::Relay => vec![rim_s2s::Role::Relay],
            Self::Host => vec![rim_s2s::Role::Host],
            Self::Both => vec![rim_s2s::Role::Host, rim_s2s::Role::Relay],
        }
    }

    pub fn runs(self, role: rim_s2s::Role) -> bool {
        self.roles().contains(&role)
    }
}

/// Longest `RIM_INSTANCE_NOTICE`, in characters.
pub const MAX_INSTANCE_NOTICE_CHARS: usize = 500;

/// Default of `RIM_S2S_PEER_RATE_PER_MIN`.
const DEFAULT_S2S_PEER_RATE_PER_MIN: u32 = 600;

/// Default of `RIM_S2S_MAX_BODY_BYTES`: 8 MiB.
pub const DEFAULT_S2S_MAX_BODY_BYTES: usize = 8 * 1024 * 1024;

impl fmt::Debug for RIMConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("RIMConfig")
            .field("server_host", &self.server_host)
            .field("server_port", &self.server_port)
            .field("database_url", &"[REDACTED]")
            .field("database_max_connections", &self.database_max_connections)
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
                "notification_digest_interval_secs",
                &self.notification_digest_interval_secs(),
            )
            .field("key_login_audiences", &self.key_login_audiences())
            .field("role", &self.role)
            .field("server_key_file", &self.server_key_file)
            .field("franking_key_file", &self.franking_key_file)
            .field("host_allowed_relays", &self.host_allowed_relays)
            .field("s2s_denied_peers", &self.s2s_denied_peers)
            .field("s2s_peer_rate_per_min", &self.s2s_peer_rate_per_min())
            .field("s2s_max_body_bytes", &self.s2s_max_body_bytes())
            .field(
                "s2s_allow_private_addresses",
                &self.s2s_allow_private_addresses(),
            )
            .field("s2s_allow_http", &self.s2s_allow_http())
            .field("registration_pow_bits", &self.registration_pow_bits)
            .field("quotas", &self.quotas())
            .field(
                "s2s_subscriptions_per_peer",
                &self.s2s_subscriptions_per_peer(),
            )
            .field("metrics_addr", &self.metrics_addr)
            .field("audit_retention_days", &self.audit_retention_days)
            .field("features_disabled", &self.features_disabled)
            .field("api_limits", &self.api_limits())
            .field("instance_notice", &self.instance_notice)
            .field("terms_url", &self.terms_url)
            .field("instance_limits", &self.instance_limits())
            .field(
                "account_deletion_grace_days",
                &self.account_deletion_grace_days,
            )
            .field("reserved_names", &self.reserved_names)
            .finish()
    }
}

impl Default for RIMConfig {
    fn default() -> Self {
        Self {
            server_host: "0.0.0.0".to_string(),
            server_port: 8080,
            database_url: "".to_string(),
            database_max_connections: None,
            session_ttl_days: 30,
            enable_graphiql: false,
            cors_allowed_origin: None,
            data_dir: "./data".to_string(),
            storage_local_path: None,
            max_workspace_storage_bytes: Some(1_073_741_824), // 1 GB default
            open_registration: None,
            trust_forwarded_headers: None,
            force_secure_cookies: None,
            smtp_host: None,
            smtp_port: None,
            smtp_username: None,
            smtp_password: None,
            smtp_from: None,
            smtp_tls: None,
            public_base_url: None,
            notification_digest_interval_secs: None,
            key_login_audiences: None,
            role: None,
            server_key_file: None,
            franking_key_file: None,
            host_allowed_relays: None,
            s2s_denied_peers: None,
            s2s_peer_rate_per_min: None,
            s2s_max_body_bytes: None,
            s2s_allow_private_addresses: None,
            s2s_allow_http: None,
            s2s_device_rate_per_min: None,
            s2s_subscriptions_per_peer: None,
            relay_queue_bytes_per_user: None,
            relay_retention_days: None,
            registration_pow_bits: None,
            relay_card_max_bytes: None,
            relay_key_packages_per_device: None,
            relay_devices_per_user: None,
            relay_forwards_per_user_per_day: None,
            relay_dm_requests_per_day: None,
            metrics_addr: None,
            audit_retention_days: None,
            features_disabled: None,
            rate_requests_per_min_per_ip: None,
            rate_mutations_per_min_per_session: None,
            rate_messages_per_min_per_user: None,
            rate_messages_burst: None,
            rate_ws_connections_per_user: None,
            rate_subscriptions_per_connection: None,
            api_max_batch: None,
            instance_notice: None,
            terms_url: None,
            limit_workspaces_per_user: None,
            limit_members_per_workspace: None,
            limit_channels_per_workspace: None,
            limit_devices_per_user: None,
            limit_dm_requests_per_day: None,
            limit_macros_per_workspace: None,
            limit_pipelines_per_workspace: None,
            limit_plugins_per_workspace: None,
            limit_invites_per_user_per_day: None,
            limit_invite_max_uses: None,
            limit_message_bytes: None,
            probation_hours: None,
            rate_probation_messages_per_min: None,
            account_deletion_grace_days: None,
            reserved_names: None,
        }
    }
}

impl RIMConfig {
    /// Connect the server's pool to `DATABASE_URL`, capped at `RIM_DATABASE_MAX_CONNECTIONS` when
    /// it is set.
    ///
    /// # Errors
    /// The driver's error when Postgres cannot be reached.
    pub async fn connect_database(&self) -> Result<sea_orm::DatabaseConnection, sea_orm::DbErr> {
        let mut options = sea_orm::ConnectOptions::new(self.database_url.clone());
        if let Some(max) = self.database_max_connections {
            options.max_connections(max.max(1));
        }
        sea_orm::Database::connect(options).await
    }

    /// Resolve the registration mode from the environment: `pow` when
    /// `RIM_REGISTRATION_POW_BITS` is set (it wins over `RIM_OPEN_REGISTRATION`), else open or
    /// invite-only.
    ///
    /// # Errors
    /// A message naming `RIM_REGISTRATION_POW_BITS` when it is outside 1 to 40.
    ///
    /// # Examples
    /// ```ignore
    /// let mode = config.registration_mode()?;
    /// ```
    pub fn registration_mode(&self) -> Result<RegistrationMode, String> {
        if let Some(bits) = self.registration_pow_bits {
            if !(1..=rim_s2s::pow::MAX_POW_BITS).contains(&bits) {
                return Err(format!(
                    "RIM_REGISTRATION_POW_BITS={bits} is outside 1 to {}",
                    rim_s2s::pow::MAX_POW_BITS
                ));
            }
            return Ok(RegistrationMode::Pow { bits });
        }
        Ok(if self.open_registration.unwrap_or(false) {
            RegistrationMode::Open
        } else {
            RegistrationMode::InviteOnly
        })
    }

    /// What each local user may hold and send, from `RIM_RELAY_*`,
    /// `RIM_S2S_DEVICE_RATE_PER_MIN` and the message rate, each kept within what the protocol
    /// allows; devices and DM requests also within their `RIM_LIMIT_*` cap.
    pub fn quotas(&self) -> rim_infra_services::quota::Quotas {
        use rim_infra_services::quota::{MAX_CARD_BYTES, Quotas};
        let defaults = Quotas::default();
        Quotas {
            card_max_bytes: self
                .relay_card_max_bytes
                .unwrap_or(defaults.card_max_bytes)
                .clamp(1024, MAX_CARD_BYTES),
            key_packages_per_device: self
                .relay_key_packages_per_device
                .unwrap_or(defaults.key_packages_per_device)
                .clamp(1, 1000),
            devices_per_user: stricter(
                self.relay_devices_per_user
                    .map_or(defaults.devices_per_user, |n| n as usize)
                    .clamp(1, 64),
                cap(self.limit_devices_per_user).map(|n| n as usize),
            ),
            forwards_per_device_per_min: self.s2s_device_rate_per_min(),
            forwards_per_user_per_day: self
                .relay_forwards_per_user_per_day
                .unwrap_or(defaults.forwards_per_user_per_day)
                .max(1),
            dm_requests_per_day: stricter(
                self.relay_dm_requests_per_day
                    .unwrap_or(defaults.dm_requests_per_day)
                    .max(1),
                cap(self.limit_dm_requests_per_day),
            ),
            messages: self.api_limits().messages,
            probation_messages: self.api_limits().probation_messages,
        }
    }

    /// Whether the server should trust `X-Forwarded-For` / `Forwarded` headers
    /// for real-IP resolution.
    ///
    /// Returns `true` only when `RIM_TRUST_FORWARDED_HEADERS=true` is set.
    /// **Operator requirement:** only enable when a trusted reverse proxy is
    /// the sole entry point. Enabling without a proxy lets clients spoof their
    /// IP and defeat per-IP rate limits.
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
    /// headers.
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

    /// Resolve the SMTP configuration from `RIM_SMTP_*`. Returns `None` (fall back to
    /// `LogEmailSender`) unless both `smtp_host` and `smtp_from` are present — see
    /// `rim_infra_services::email::SmtpConfig::from_parts`.
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

    /// e.g. `https://rim.example.com`. Required to build clickable password-reset/invite
    /// links in transactional emails; without it, those emails fall back to a bare token or no
    /// link.
    ///
    /// # Examples
    /// ```ignore
    /// let base_url = config.public_base_url();
    /// ```
    pub fn public_base_url(&self) -> Option<String> {
        self.public_base_url.clone()
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
    /// otherwise `localhost:PORT` and `127.0.0.1:PORT`, but only for a server bound to a
    /// loopback address. Lowercased.
    ///
    /// # Errors
    /// A message naming both variables when neither is set and `RIM_SERVER_HOST` accepts
    /// connections from other machines.
    pub fn key_login_audiences(&self) -> Result<Vec<String>, String> {
        let configured: Vec<String> = self
            .key_login_audiences
            .as_deref()
            .unwrap_or_default()
            .split(',')
            .map(|a| a.trim().to_ascii_lowercase())
            .filter(|a| !a.is_empty())
            .collect();
        if !configured.is_empty() {
            return Ok(configured);
        }
        if let Some(authority) = self.public_base_url.as_deref().and_then(url_authority) {
            return Ok(vec![authority]);
        }
        if !is_loopback_host(&self.server_host) {
            return Err(format!(
                "RIM_SERVER_HOST={} accepts connections from other machines: set \
                 RIM_KEY_LOGIN_AUDIENCES or RIM_PUBLIC_BASE_URL to the host[:port] clients reach \
                 this server at (the localhost default is only for loopback binds)",
                self.server_host
            ));
        }
        Ok(vec![
            format!("localhost:{}", self.server_port),
            format!("127.0.0.1:{}", self.server_port),
        ])
    }
}

/// Whether a bind host accepts connections from this machine only: `localhost` or a loopback
/// address.
fn is_loopback_host(host: &str) -> bool {
    let host = host.trim();
    let host = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl RIMConfig {
    /// `RIM_ROLE`, `both` when unset.
    ///
    /// # Errors
    /// A message naming the value when it is not `relay`, `host` or `both`.
    pub fn server_role(&self) -> Result<ServerRole, String> {
        match self
            .role
            .as_deref()
            .map(str::trim)
            .filter(|r| !r.is_empty())
        {
            None => Ok(ServerRole::Both),
            Some(role) => ServerRole::parse(role)
                .ok_or_else(|| format!("RIM_ROLE={role} is not relay, host or both")),
        }
    }

    /// The server key file: `RIM_SERVER_KEY_FILE`, or `<RIM_DATA_DIR>/server-key.json`.
    pub fn key_files(&self) -> rim_infra_services::s2s::server_key::KeyFiles {
        rim_infra_services::s2s::server_key::KeyFiles::new(
            &self.data_dir,
            self.server_key_file
                .as_deref()
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(std::path::PathBuf::from),
        )
    }

    /// The franking key file: `RIM_FRANKING_KEY_FILE`, or `<RIM_DATA_DIR>/franking-key.json`.
    pub fn franking_key_file(&self) -> std::path::PathBuf {
        self.franking_key_file
            .as_deref()
            .map(str::trim)
            .filter(|p| !p.is_empty())
            .map(std::path::PathBuf::from)
            .unwrap_or_else(|| std::path::Path::new(&self.data_dir).join("franking-key.json"))
    }

    pub fn s2s_allow_http(&self) -> bool {
        self.s2s_allow_http.unwrap_or(false)
    }

    pub fn s2s_allow_private_addresses(&self) -> bool {
        self.s2s_allow_private_addresses.unwrap_or(false)
    }

    pub fn s2s_peer_rate_per_min(&self) -> u32 {
        self.s2s_peer_rate_per_min
            .unwrap_or(DEFAULT_S2S_PEER_RATE_PER_MIN)
    }

    pub fn s2s_device_rate_per_min(&self) -> u32 {
        self.s2s_device_rate_per_min.unwrap_or(120).max(1)
    }

    pub fn s2s_subscriptions_per_peer(&self) -> u32 {
        self.s2s_subscriptions_per_peer.unwrap_or(10_000).max(1)
    }

    pub fn relay_queue_bytes_per_user(&self) -> i64 {
        self.relay_queue_bytes_per_user
            .unwrap_or(256 * 1024 * 1024)
            .max(1024 * 1024)
    }

    pub fn relay_retention_days(&self) -> u32 {
        self.relay_retention_days.unwrap_or(30).clamp(1, 365)
    }

    pub fn s2s_max_body_bytes(&self) -> usize {
        self.s2s_max_body_bytes
            .unwrap_or(DEFAULT_S2S_MAX_BODY_BYTES)
    }

    /// The origin of `RIM_PUBLIC_BASE_URL` when server-to-server traffic is on: the URL is
    /// `https`, or `http` with `RIM_S2S_ALLOW_HTTP`. `None` turns the traffic off.
    pub fn s2s_origin(&self) -> Option<String> {
        let url = url::Url::parse(self.public_base_url.as_deref()?.trim()).ok()?;
        match url.scheme() {
            "https" => {}
            "http" if self.s2s_allow_http() => {}
            _ => return None,
        }
        url.host()?;
        Some(url.origin().ascii_serialization())
    }

    /// The origin clients reach this server at: that of `RIM_PUBLIC_BASE_URL` (any scheme), or
    /// `http://localhost:PORT` when unset.
    pub fn public_origin(&self) -> String {
        self.public_base_url
            .as_deref()
            .and_then(|u| url::Url::parse(u.trim()).ok())
            .filter(|u| u.host().is_some())
            .map(|u| u.origin().ascii_serialization())
            .unwrap_or_else(|| format!("http://localhost:{}", self.server_port))
    }

    /// `RIM_METRICS_ADDR` as a socket address; `None` when unset or empty.
    ///
    /// # Errors
    /// A message naming the variable when the value is not an `ip:port` address.
    pub fn metrics_addr(&self) -> Result<Option<std::net::SocketAddr>, String> {
        match self
            .metrics_addr
            .as_deref()
            .map(str::trim)
            .filter(|a| !a.is_empty())
        {
            None => Ok(None),
            Some(addr) => addr
                .parse()
                .map(Some)
                .map_err(|_| format!("RIM_METRICS_ADDR={addr} is not an ip:port address")),
        }
    }

    /// `RIM_AUDIT_RETENTION_DAYS`; `None` keeps audit rows forever.
    ///
    /// # Errors
    /// A message naming the variable when the value is outside 1 to 3650.
    pub fn audit_retention_days(&self) -> Result<Option<u32>, String> {
        use rim_infra_services::audit_retention::{MAX_RETENTION_DAYS, MIN_RETENTION_DAYS};
        match self.audit_retention_days {
            None => Ok(None),
            Some(days) if (MIN_RETENTION_DAYS..=MAX_RETENTION_DAYS).contains(&days) => {
                Ok(Some(days))
            }
            Some(days) => Err(format!(
                "RIM_AUDIT_RETENTION_DAYS={days} is outside {MIN_RETENTION_DAYS} to \
                 {MAX_RETENTION_DAYS}; unset it to keep audit rows forever"
            )),
        }
    }

    /// `RIM_ACCOUNT_DELETION_GRACE_DAYS`: the days between an account's deletion request and
    /// its purge; 14 when unset.
    ///
    /// # Errors
    /// A message naming the variable when the value is above 30.
    pub fn account_deletion_grace_days(&self) -> Result<u32, String> {
        use rim_services::account_deletion::dto::{DEFAULT_GRACE_DAYS, MAX_GRACE_DAYS};
        match self.account_deletion_grace_days {
            None => Ok(DEFAULT_GRACE_DAYS),
            Some(days) if days <= MAX_GRACE_DAYS => Ok(days),
            Some(days) => Err(format!(
                "RIM_ACCOUNT_DELETION_GRACE_DAYS={days} is outside 0 to {MAX_GRACE_DAYS}"
            )),
        }
    }

    /// `RIM_S2S_DENIED_PEERS` as server ids.
    ///
    /// # Errors
    /// A message naming the first entry that is not a server id.
    pub fn s2s_denied_peers(&self) -> Result<Vec<String>, String> {
        server_id_list("RIM_S2S_DENIED_PEERS", self.s2s_denied_peers.as_deref())
    }

    /// `RIM_HOST_ALLOWED_RELAYS` as server ids; empty allows any relay.
    ///
    /// # Errors
    /// A message naming the first entry that is not a server id.
    pub fn host_allowed_relays(&self) -> Result<Vec<String>, String> {
        server_id_list(
            "RIM_HOST_ALLOWED_RELAYS",
            self.host_allowed_relays.as_deref(),
        )
    }
}

impl RIMConfig {
    /// `RIM_FEATURES_DISABLED` as features; empty when unset.
    ///
    /// # Errors
    /// A message naming the variable and the first name that is not a feature, so a typo
    /// stops the server instead of leaving a feature on.
    pub fn features_disabled(
        &self,
    ) -> Result<Vec<rim_core::server_feature::model::ServerFeature>, String> {
        rim_core::server_feature::model::ServerFeature::parse_list(
            self.features_disabled.as_deref().unwrap_or_default(),
        )
        .map_err(|e| format!("RIM_FEATURES_DISABLED: {e}"))
    }

    /// The features the rest of the configuration cannot support, each with the reason: email
    /// without SMTP.
    pub fn features_unavailable(
        &self,
    ) -> Vec<(rim_core::server_feature::model::ServerFeature, String)> {
        let mut unavailable = Vec::new();
        if self.smtp_config().is_none() {
            unavailable.push((
                rim_core::server_feature::model::ServerFeature::Email,
                "SMTP is not configured: set RIM_SMTP_HOST and RIM_SMTP_FROM".to_string(),
            ));
        }
        unavailable
    }

    /// The instance's feature gate: the disabled features, then the unavailable ones.
    ///
    /// # Errors
    /// The message of [`RIMConfig::features_disabled`].
    pub fn feature_gate(
        &self,
    ) -> Result<rim_infra_services::feature_gate::InstanceFeatureGate, String> {
        Ok(rim_infra_services::feature_gate::InstanceFeatureGate::new(
            &self.features_disabled()?,
            &self.features_unavailable(),
        ))
    }
}

impl RIMConfig {
    /// `RIM_INSTANCE_NOTICE`, trimmed; `None` when unset or blank.
    ///
    /// # Errors
    /// A message naming the variable when the notice is longer than
    /// [`MAX_INSTANCE_NOTICE_CHARS`] characters.
    pub fn instance_notice(&self) -> Result<Option<String>, String> {
        let Some(notice) = non_blank(self.instance_notice.as_deref()) else {
            return Ok(None);
        };
        let chars = notice.chars().count();
        if chars > MAX_INSTANCE_NOTICE_CHARS {
            return Err(format!(
                "RIM_INSTANCE_NOTICE is {chars} characters long; the most is \
                 {MAX_INSTANCE_NOTICE_CHARS}"
            ));
        }
        Ok(Some(notice.to_string()))
    }

    /// `RIM_TERMS_URL`, trimmed; `None` when unset or blank.
    ///
    /// # Errors
    /// A message naming the variable when the value is not an absolute `http` or `https` URL
    /// with a host.
    pub fn terms_url(&self) -> Result<Option<String>, String> {
        let Some(terms) = non_blank(self.terms_url.as_deref()) else {
            return Ok(None);
        };
        match url::Url::parse(terms) {
            Ok(url) if matches!(url.scheme(), "http" | "https") && url.host().is_some() => {
                Ok(Some(terms.to_string()))
            }
            _ => Err(format!(
                "RIM_TERMS_URL={terms} is not an absolute http or https URL"
            )),
        }
    }

    /// The instance notice and terms URL the GraphQL schema serves and signups follow.
    ///
    /// # Errors
    /// The messages of [`RIMConfig::instance_notice`] and [`RIMConfig::terms_url`].
    pub fn instance_policy(&self) -> Result<rim_api::types::InstancePolicy, String> {
        Ok(rim_api::types::InstancePolicy {
            notice: self.instance_notice()?,
            terms_url: self.terms_url()?,
        })
    }
}

impl RIMConfig {
    /// `RIM_RESERVED_NAMES`, compared case-insensitively after NFKC normalisation; nothing is
    /// reserved when it is unset or blank.
    ///
    /// # Errors
    /// A message naming the variable when an entry is empty, as in `rim,,admin`.
    pub fn reserved_names(
        &self,
    ) -> Result<rim_infra_services::reserved_names::ReservedNames, String> {
        rim_infra_services::reserved_names::ReservedNames::parse(
            self.reserved_names.as_deref().unwrap_or_default(),
        )
        .map_err(|e| format!("RIM_RESERVED_NAMES: {e}"))
    }
}

/// `value` trimmed, or `None` when it is unset or blank.
fn non_blank(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|v| !v.is_empty())
}

/// A comma-separated list of server ids; refuses anything else so a typo does not silently
/// allow or deny the wrong server.
fn server_id_list(name: &str, value: Option<&str>) -> Result<Vec<String>, String> {
    value
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|id| !id.is_empty())
        .map(|id| {
            if rim_s2s::is_server_id(id) {
                Ok(id.to_string())
            } else {
                Err(format!("{name}: {id} is not a server id"))
            }
        })
        .collect()
}

/// The `host[:port]` of an absolute URL such as `https://rim.example.com/app`, lowercased.
fn url_authority(url: &str) -> Option<String> {
    let (_, rest) = url.trim().split_once("://")?;
    let authority = rest.split(['/', '?', '#']).next()?;
    let authority = authority.rsplit('@').next()?;
    (!authority.is_empty()).then(|| authority.to_ascii_lowercase())
}

/// Most GraphQL requests in one batch unless `RIM_API_MAX_BATCH` says otherwise.
pub const DEFAULT_API_MAX_BATCH: usize = 20;

/// The API's request limits; a `None` limit is off.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApiLimits {
    /// Requests per client address to `/api` routes.
    pub requests_per_ip: Option<rim_infra_services::token_bucket::BucketRate>,
    /// Mutation fields per session, or per address without one.
    pub mutations_per_session: Option<rim_infra_services::token_bucket::BucketRate>,
    /// Messages per user.
    pub messages: Option<rim_infra_services::token_bucket::BucketRate>,
    /// Messages per account in probation, instead of `messages`.
    pub probation_messages: Option<rim_infra_services::token_bucket::BucketRate>,
    /// Open WebSocket connections per user.
    pub ws_connections_per_user: Option<u32>,
    /// Active subscriptions per WebSocket connection.
    pub subscriptions_per_connection: Option<u32>,
    /// GraphQL requests per batch.
    pub max_batch: usize,
}

impl RIMConfig {
    /// The API limits from `RIM_RATE_*` and `RIM_API_MAX_BATCH`: each rate off when unset or
    /// 0, the message burst defaulting to the per-minute rate, the batch cap to
    /// [`DEFAULT_API_MAX_BATCH`] and at least 1.
    pub fn api_limits(&self) -> ApiLimits {
        use rim_infra_services::token_bucket::BucketRate;
        let per_minute = |n: Option<u32>| n.and_then(BucketRate::per_minute);
        let cap = |n: Option<u32>| n.filter(|n| *n > 0);
        ApiLimits {
            requests_per_ip: per_minute(self.rate_requests_per_min_per_ip),
            mutations_per_session: per_minute(self.rate_mutations_per_min_per_session),
            messages: self
                .rate_messages_per_min_per_user
                .and_then(|rate| BucketRate::new(rate, self.rate_messages_burst.unwrap_or(rate))),
            probation_messages: per_minute(self.rate_probation_messages_per_min),
            ws_connections_per_user: cap(self.rate_ws_connections_per_user),
            subscriptions_per_connection: cap(self.rate_subscriptions_per_connection),
            max_batch: self.api_max_batch.unwrap_or(DEFAULT_API_MAX_BATCH).max(1),
        }
    }
}

/// Largest `RIM_LIMIT_MESSAGE_BYTES`: the largest message content a client may send.
pub const MAX_LIMIT_MESSAGE_BYTES: u32 = rim_services::e2ee::MAX_CONTENT_BYTES as u32;

/// A `RIM_LIMIT_*` value as a cap: `None` when unset or 0.
fn cap(n: Option<u32>) -> Option<u32> {
    n.filter(|n| *n > 0)
}

/// `quota` lowered to `limit` when that is smaller.
fn stricter<T: Ord>(quota: T, limit: Option<T>) -> T {
    match limit {
        Some(limit) => quota.min(limit),
        None => quota,
    }
}

impl RIMConfig {
    /// The instance limits from `RIM_LIMIT_*`, each off when unset or 0. Devices and DM
    /// requests report the stricter of their cap and relay quota, which is what applies.
    ///
    /// # Errors
    /// A message naming `RIM_LIMIT_MESSAGE_BYTES` when it is above
    /// [`MAX_LIMIT_MESSAGE_BYTES`].
    pub fn instance_limits(&self) -> Result<rim_services::limits::InstanceLimits, String> {
        let message_bytes = cap(self.limit_message_bytes);
        if let Some(bytes) = message_bytes
            && bytes > MAX_LIMIT_MESSAGE_BYTES
        {
            return Err(format!(
                "RIM_LIMIT_MESSAGE_BYTES={bytes} is above {MAX_LIMIT_MESSAGE_BYTES}, the largest \
                 message"
            ));
        }
        let quotas = self.quotas();
        Ok(rim_services::limits::InstanceLimits {
            workspaces_per_user: cap(self.limit_workspaces_per_user),
            members_per_workspace: cap(self.limit_members_per_workspace),
            channels_per_workspace: cap(self.limit_channels_per_workspace),
            devices_per_user: u32::try_from(quotas.devices_per_user).ok(),
            dm_requests_per_day: Some(quotas.dm_requests_per_day),
            macros_per_workspace: cap(self.limit_macros_per_workspace),
            pipelines_per_workspace: cap(self.limit_pipelines_per_workspace),
            plugins_per_workspace: cap(self.limit_plugins_per_workspace).map(|n| {
                n.min(
                    u32::try_from(rim_core::workspace_plugin::model::MAX_WORKSPACE_PLUGINS)
                        .unwrap_or(u32::MAX),
                )
            }),
            invites_per_user_per_day: cap(self.limit_invites_per_user_per_day),
            invite_max_uses: cap(self.limit_invite_max_uses),
            message_bytes,
            probation_hours: cap(self.probation_hours),
        })
    }
}

/// Variables the configuration reads without the `RIM_` prefix. Every other setting needs the
/// prefix, so a bare `ROLE` or `SERVER_PORT` left in the environment by something else changes
/// nothing. `RUST_LOG` is read unprefixed by the logger, not here.
const UNPREFIXED_VARIABLES: &[&str] = &["DATABASE_URL"];

impl RIMConfig {
    #[allow(clippy::missing_errors_doc)]
    pub fn load() -> Result<Self, ConfigError> {
        Self::load_from(std::env::vars().collect())
    }

    /// Builds the configuration from the given environment variables over the defaults.
    fn load_from(env: config::Map<String, String>) -> Result<Self, ConfigError> {
        let unprefixed: config::Map<String, String> = env
            .iter()
            .filter(|(name, _)| UNPREFIXED_VARIABLES.contains(&name.as_str()))
            .map(|(name, value)| (name.clone(), value.clone()))
            .collect();
        let c = Config::builder()
            .set_default("server_host", "0.0.0.0")?
            .set_default("server_port", 8080)?
            .set_default("session_ttl_days", 30)?
            .set_default("enable_graphiql", false)?
            .set_default("data_dir", "./data")?
            .set_default("max_workspace_storage_bytes", 1_073_741_824_i64)?
            .set_default("notification_digest_interval_secs", 900_i64)?
            .set_default(
                "s2s_peer_rate_per_min",
                i64::from(DEFAULT_S2S_PEER_RATE_PER_MIN),
            )?
            .set_default("s2s_max_body_bytes", DEFAULT_S2S_MAX_BODY_BYTES as i64)?
            .add_source(Environment::with_prefix("RIM").source(Some(env)))
            .add_source(Environment::default().source(Some(unprefixed)));
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

    /// The `version` under `[workspace.package]` in the root manifest.
    fn workspace_version() -> String {
        let manifest = include_str!("../../Cargo.toml");
        let section = manifest
            .split("[workspace.package]")
            .nth(1)
            .expect("[workspace.package] in the root manifest");
        section
            .lines()
            .find_map(|line| line.trim().strip_prefix("version = "))
            .expect("a workspace version")
            .trim_matches('"')
            .to_string()
    }

    #[test]
    fn version_is_the_workspace_version() {
        assert_eq!(VERSION, workspace_version());
        assert_eq!(
            version_line(),
            format!("rim-server {}", workspace_version())
        );
    }

    fn env(pairs: &[(&str, &str)]) -> config::Map<String, String> {
        pairs
            .iter()
            .map(|(name, value)| ((*name).to_string(), (*value).to_string()))
            .collect()
    }

    #[test]
    fn only_database_url_is_read_without_the_prefix() {
        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("ROLE", "relay"),
            ("DATA_DIR", "/elsewhere"),
            ("SERVER_PORT", "1"),
            ("ENABLE_GRAPHIQL", "true"),
        ]))
        .expect("loads");
        assert_eq!(config.database_url, "postgres://localhost/rim");
        assert_eq!(config.role, None);
        assert_eq!(config.data_dir, "./data");
        assert_eq!(config.server_port, 8080);
        assert!(!config.enable_graphiql);

        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("RIM_ROLE", "relay"),
            ("RIM_DATA_DIR", "/elsewhere"),
            ("RIM_SERVER_PORT", "9000"),
            ("RIM_ENABLE_GRAPHIQL", "true"),
        ]))
        .expect("loads");
        assert_eq!(config.role.as_deref(), Some("relay"));
        assert_eq!(config.data_dir, "/elsewhere");
        assert_eq!(config.server_port, 9000);
        assert!(config.enable_graphiql);
    }

    #[test]
    fn database_url_is_required() {
        assert!(RIMConfig::load_from(env(&[("RIM_SERVER_PORT", "9000")])).is_err());
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
            Ok(vec![
                "rim.example.com".to_string(),
                "relay.rim.chat:8443".to_string()
            ])
        );
    }

    #[test]
    fn key_login_audiences_falls_back_to_public_base_url_host() {
        let config = RIMConfig {
            public_base_url: Some("https://user@RIM.example.com:8443/app?x=1".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.key_login_audiences(),
            Ok(vec!["rim.example.com:8443".to_string()])
        );
    }

    #[test]
    fn key_login_audiences_defaults_to_localhost_on_the_server_port() {
        for host in ["127.0.0.1", "localhost", "::1", "[::1]", "127.0.0.2"] {
            let config = RIMConfig {
                server_host: host.to_string(),
                server_port: 9000,
                key_login_audiences: Some(" ".to_string()),
                ..Default::default()
            };
            assert_eq!(
                config.key_login_audiences(),
                Ok(vec![
                    "localhost:9000".to_string(),
                    "127.0.0.1:9000".to_string()
                ]),
                "{host}"
            );
        }
    }

    #[test]
    fn finding_15_a_public_bind_without_an_audience_refuses_to_start() {
        for host in ["0.0.0.0", "::", "192.0.2.10", "rim.example.com"] {
            let config = RIMConfig {
                server_host: host.to_string(),
                ..Default::default()
            };
            let err = config.key_login_audiences().unwrap_err();
            assert!(
                err.contains("RIM_KEY_LOGIN_AUDIENCES") && err.contains("RIM_PUBLIC_BASE_URL"),
                "{host}: {err}"
            );
            let configured = RIMConfig {
                key_login_audiences: Some("rim.example.com".to_string()),
                ..config.clone()
            };
            assert!(configured.key_login_audiences().is_ok(), "{host}");
            let public = RIMConfig {
                public_base_url: Some("https://rim.example.com".to_string()),
                ..config
            };
            assert!(public.key_login_audiences().is_ok(), "{host}");
        }
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
    fn role_defaults_to_both_and_refuses_unknown_values() {
        assert_eq!(RIMConfig::default().server_role(), Ok(ServerRole::Both));
        for (value, role) in [
            ("relay", ServerRole::Relay),
            (" Host ", ServerRole::Host),
            ("both", ServerRole::Both),
        ] {
            let config = RIMConfig {
                role: Some(value.to_string()),
                ..Default::default()
            };
            assert_eq!(config.server_role(), Ok(role));
        }
        let config = RIMConfig {
            role: Some("hub".to_string()),
            ..Default::default()
        };
        assert!(config.server_role().is_err());
        assert_eq!(
            ServerRole::Both.roles(),
            vec![rim_s2s::Role::Host, rim_s2s::Role::Relay]
        );
    }

    #[test]
    fn s2s_is_on_only_for_https_or_allowed_http() {
        assert_eq!(RIMConfig::default().s2s_origin(), None);
        let https = RIMConfig {
            public_base_url: Some("https://RIM.example.com/app".to_string()),
            ..Default::default()
        };
        assert_eq!(
            https.s2s_origin().as_deref(),
            Some("https://rim.example.com")
        );
        let http = RIMConfig {
            public_base_url: Some("http://127.0.0.1:8098".to_string()),
            ..Default::default()
        };
        assert_eq!(http.s2s_origin(), None);
        assert_eq!(http.public_origin(), "http://127.0.0.1:8098");
        let allowed = RIMConfig {
            s2s_allow_http: Some(true),
            ..http
        };
        assert_eq!(
            allowed.s2s_origin().as_deref(),
            Some("http://127.0.0.1:8098")
        );
        assert_eq!(
            RIMConfig {
                server_port: 9000,
                ..Default::default()
            }
            .public_origin(),
            "http://localhost:9000"
        );
    }

    #[test]
    fn franking_key_file_defaults_to_the_data_dir() {
        let config = RIMConfig {
            data_dir: "/var/lib/rim".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.franking_key_file(),
            std::path::PathBuf::from("/var/lib/rim/franking-key.json")
        );
        let config = RIMConfig {
            franking_key_file: Some(" /secrets/franking.json ".to_string()),
            ..config
        };
        assert_eq!(
            config.franking_key_file(),
            std::path::PathBuf::from("/secrets/franking.json")
        );
    }

    #[test]
    fn peer_lists_accept_only_server_ids() {
        let id = "rims1_OFPDNXTV1splCcKcgz1vWYj6Mg-lbUollHQwXTkYEBw";
        let config = RIMConfig {
            s2s_denied_peers: Some(format!(" {id} ,, ")),
            host_allowed_relays: Some("relay.example".to_string()),
            ..Default::default()
        };
        assert_eq!(config.s2s_denied_peers(), Ok(vec![id.to_string()]));
        assert!(config.host_allowed_relays().is_err());
        assert_eq!(RIMConfig::default().host_allowed_relays(), Ok(vec![]));
    }

    #[test]
    fn key_file_defaults_to_the_data_dir() {
        let config = RIMConfig {
            data_dir: "/var/lib/rim".to_string(),
            ..Default::default()
        };
        assert_eq!(
            config.key_files().key_file,
            std::path::PathBuf::from("/var/lib/rim/server-key.json")
        );
        let config = RIMConfig {
            server_key_file: Some("/secrets/key.json".to_string()),
            ..config
        };
        assert_eq!(
            config.key_files().key_file,
            std::path::PathBuf::from("/secrets/key.json")
        );
    }

    #[test]
    fn pow_bits_turn_on_pow_registration_over_open() {
        assert_eq!(
            RIMConfig::default().registration_mode(),
            Ok(RegistrationMode::InviteOnly)
        );
        let config = RIMConfig {
            open_registration: Some(true),
            ..Default::default()
        };
        assert_eq!(config.registration_mode(), Ok(RegistrationMode::Open));
        let config = RIMConfig {
            registration_pow_bits: Some(24),
            ..config
        };
        assert_eq!(
            config.registration_mode(),
            Ok(RegistrationMode::Pow { bits: 24 })
        );
        for bits in [0, 41] {
            let config = RIMConfig {
                registration_pow_bits: Some(bits),
                ..Default::default()
            };
            let err = config.registration_mode().unwrap_err();
            assert!(err.contains("RIM_REGISTRATION_POW_BITS"), "{err}");
        }
    }

    #[test]
    fn quotas_have_defaults_and_stay_within_the_protocol() {
        assert_eq!(
            RIMConfig::default().quotas(),
            rim_infra_services::quota::Quotas::default()
        );
        let config = RIMConfig {
            relay_card_max_bytes: Some(1 << 30),
            relay_key_packages_per_device: Some(0),
            relay_devices_per_user: Some(500),
            relay_forwards_per_user_per_day: Some(10),
            relay_dm_requests_per_day: Some(5),
            s2s_device_rate_per_min: Some(60),
            ..Default::default()
        };
        let quotas = config.quotas();
        assert_eq!(quotas.card_max_bytes, 131_072);
        assert_eq!(quotas.key_packages_per_device, 1);
        assert_eq!(quotas.devices_per_user, 64);
        assert_eq!(quotas.forwards_per_device_per_min, 60);
        assert_eq!(quotas.forwards_per_user_per_day, 10);
        assert_eq!(quotas.dm_requests_per_day, 5);
    }

    #[test]
    fn metrics_addr_is_off_when_unset_and_refuses_a_non_address() {
        assert_eq!(RIMConfig::default().metrics_addr(), Ok(None));
        let config = |value: &str| RIMConfig {
            metrics_addr: Some(value.to_string()),
            ..Default::default()
        };
        assert_eq!(config("  ").metrics_addr(), Ok(None));
        assert_eq!(
            config(" 127.0.0.1:9090 ").metrics_addr(),
            Ok(Some(std::net::SocketAddr::from(([127, 0, 0, 1], 9090))))
        );
        assert_eq!(
            config("[::1]:9090").metrics_addr(),
            Ok(Some("[::1]:9090".parse().unwrap()))
        );
        for bad in ["localhost:9090", "127.0.0.1", "9090"] {
            let err = config(bad).metrics_addr().unwrap_err();
            assert!(err.contains("RIM_METRICS_ADDR"), "{bad}: {err}");
        }
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

    #[test]
    fn relay_queue_cap_defaults_to_256_mib_with_a_1_mib_floor() {
        let mib = 1024 * 1024;
        assert_eq!(RIMConfig::default().relay_queue_bytes_per_user(), 256 * mib);
        let cap = |bytes| RIMConfig {
            relay_queue_bytes_per_user: Some(bytes),
            ..Default::default()
        };
        assert_eq!(cap(8 * mib).relay_queue_bytes_per_user(), 8 * mib);
        assert_eq!(cap(1).relay_queue_bytes_per_user(), mib);
    }

    #[test]
    fn audit_retention_is_off_when_unset_and_refuses_values_outside_the_range() {
        assert_eq!(RIMConfig::default().audit_retention_days(), Ok(None));
        for days in [1, 90, 3650] {
            let config = RIMConfig {
                audit_retention_days: Some(days),
                ..Default::default()
            };
            assert_eq!(config.audit_retention_days(), Ok(Some(days)));
        }
        for days in [0, 3651] {
            let config = RIMConfig {
                audit_retention_days: Some(days),
                ..Default::default()
            };
            let err = config.audit_retention_days().unwrap_err();
            assert!(err.contains("RIM_AUDIT_RETENTION_DAYS"), "{err}");
        }
    }

    #[test]
    fn the_account_deletion_grace_is_14_days_unless_set_between_0_and_30() {
        assert_eq!(RIMConfig::default().account_deletion_grace_days(), Ok(14));
        for days in [0, 7, 30] {
            let config = RIMConfig {
                account_deletion_grace_days: Some(days),
                ..Default::default()
            };
            assert_eq!(config.account_deletion_grace_days(), Ok(days));
        }
        let config = RIMConfig {
            account_deletion_grace_days: Some(31),
            ..Default::default()
        };
        let err = config.account_deletion_grace_days().unwrap_err();
        assert!(err.contains("RIM_ACCOUNT_DELETION_GRACE_DAYS"), "{err}");
    }

    #[test]
    fn audit_retention_days_is_read_from_its_variable_and_a_non_number_is_refused() {
        let parse = |value: &str| {
            Config::builder()
                .set_override("server_host", "127.0.0.1")
                .and_then(|b| b.set_override("server_port", 8090))
                .and_then(|b| b.set_override("database_url", "postgres://localhost/rim"))
                .and_then(|b| b.set_override("session_ttl_days", 30))
                .and_then(|b| b.set_override("enable_graphiql", false))
                .and_then(|b| b.set_override("data_dir", "./data"))
                .and_then(|b| b.set_override("audit_retention_days", value))
                .and_then(|b| b.build())
                .and_then(|c| c.try_deserialize::<RIMConfig>())
        };
        let config = parse("90").expect("a number parses");
        assert_eq!(config.audit_retention_days(), Ok(Some(90)));
        assert!(parse("ninety").is_err());
        assert!(parse("-1").is_err());
    }

    #[test]
    fn features_disabled_is_empty_when_unset_or_blank() {
        assert_eq!(RIMConfig::default().features_disabled(), Ok(vec![]));
        let config = RIMConfig {
            features_disabled: Some(" , ".to_string()),
            ..Default::default()
        };
        assert_eq!(config.features_disabled(), Ok(vec![]));
    }

    #[test]
    fn features_disabled_ignores_spaces_around_names() {
        use rim_core::server_feature::model::ServerFeature;
        let config = RIMConfig {
            features_disabled: Some(" attachments , custom_emoji ".to_string()),
            ..Default::default()
        };
        assert_eq!(
            config.features_disabled(),
            Ok(vec![ServerFeature::Attachments, ServerFeature::CustomEmoji])
        );
    }

    #[test]
    fn an_unknown_feature_refuses_to_start_naming_it() {
        let config = RIMConfig {
            features_disabled: Some("attachments,incidents".to_string()),
            ..Default::default()
        };
        let err = config.features_disabled().unwrap_err();
        assert!(err.contains("RIM_FEATURES_DISABLED"), "{err}");
        assert!(err.contains("incidents"), "{err}");
        assert!(config.feature_gate().is_err());
    }

    #[test]
    fn email_is_unavailable_without_smtp_and_disabled_wins() {
        use rim_core::server_feature::model::{FeatureState, ServerFeature};
        use rim_services::feature::service::FeatureGate;

        let gate = RIMConfig::default().feature_gate().expect("gate");
        assert!(matches!(
            gate.state(ServerFeature::Email),
            FeatureState::Unavailable { reason } if reason.contains("RIM_SMTP_HOST")
        ));
        for feature in ServerFeature::ALL {
            if feature != ServerFeature::Email {
                assert_eq!(gate.state(feature), FeatureState::Enabled, "{feature}");
            }
        }

        let smtp = RIMConfig {
            smtp_host: Some("smtp.example.com".to_string()),
            smtp_from: Some("RIM <noreply@example.com>".to_string()),
            ..Default::default()
        };
        assert!(smtp.features_unavailable().is_empty());
        let gate = smtp.feature_gate().expect("gate");
        assert_eq!(gate.state(ServerFeature::Email), FeatureState::Enabled);

        let disabled = RIMConfig {
            features_disabled: Some("email".to_string()),
            ..Default::default()
        };
        let gate = disabled.feature_gate().expect("gate");
        assert_eq!(gate.state(ServerFeature::Email), FeatureState::Disabled);
    }

    #[test]
    fn api_limits_are_off_by_default_except_the_batch_cap() {
        let limits = RIMConfig::default().api_limits();
        assert_eq!(
            limits,
            ApiLimits {
                requests_per_ip: None,
                mutations_per_session: None,
                messages: None,
                probation_messages: None,
                ws_connections_per_user: None,
                subscriptions_per_connection: None,
                max_batch: DEFAULT_API_MAX_BATCH,
            }
        );
        assert_eq!(RIMConfig::default().quotas().messages, None);
    }

    #[test]
    fn api_limits_are_read_from_their_variables() {
        use rim_infra_services::token_bucket::BucketRate;
        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("RIM_RATE_REQUESTS_PER_MIN_PER_IP", "300"),
            ("RIM_RATE_MUTATIONS_PER_MIN_PER_SESSION", "0"),
            ("RIM_RATE_MESSAGES_PER_MIN_PER_USER", "30"),
            ("RIM_RATE_WS_CONNECTIONS_PER_USER", "5"),
            ("RIM_RATE_SUBSCRIPTIONS_PER_CONNECTION", "50"),
            ("RIM_API_MAX_BATCH", "0"),
        ]))
        .expect("loads");
        let limits = config.api_limits();
        assert_eq!(limits.requests_per_ip, BucketRate::new(300, 300));
        assert_eq!(limits.mutations_per_session, None, "0 is off");
        assert_eq!(
            limits.messages,
            BucketRate::new(30, 30),
            "the burst defaults to the rate"
        );
        assert_eq!(limits.ws_connections_per_user, Some(5));
        assert_eq!(limits.subscriptions_per_connection, Some(50));
        assert_eq!(limits.max_batch, 1, "the batch cap is at least 1");
        assert_eq!(config.quotas().messages, limits.messages);

        let config = RIMConfig {
            rate_messages_per_min_per_user: Some(30),
            rate_messages_burst: Some(5),
            ..Default::default()
        };
        assert_eq!(config.api_limits().messages, BucketRate::new(30, 5));
    }

    #[test]
    fn features_disabled_is_read_from_its_variable() {
        let config = Config::builder()
            .set_override("server_host", "127.0.0.1")
            .and_then(|b| b.set_override("server_port", 8090))
            .and_then(|b| b.set_override("database_url", "postgres://localhost/rim"))
            .and_then(|b| b.set_override("session_ttl_days", 30))
            .and_then(|b| b.set_override("enable_graphiql", false))
            .and_then(|b| b.set_override("data_dir", "./data"))
            .and_then(|b| b.set_override("features_disabled", "custom_emoji"))
            .and_then(|b| b.build())
            .and_then(|c| c.try_deserialize::<RIMConfig>())
            .expect("parses");
        assert_eq!(
            config.features_disabled(),
            Ok(vec![
                rim_core::server_feature::model::ServerFeature::CustomEmoji
            ])
        );
    }

    #[test]
    fn instance_notice_is_trimmed_blank_is_none_and_over_500_characters_is_refused() {
        let notice = |value: &str| {
            RIMConfig {
                instance_notice: Some(value.to_string()),
                ..Default::default()
            }
            .instance_notice()
        };
        assert_eq!(RIMConfig::default().instance_notice(), Ok(None));
        assert_eq!(notice("  "), Ok(None));
        assert_eq!(
            notice(" Run by the Example club. "),
            Ok(Some("Run by the Example club.".to_string()))
        );
        // Characters, not bytes: 500 two-byte characters fit.
        let longest = "é".repeat(MAX_INSTANCE_NOTICE_CHARS);
        assert_eq!(notice(&longest), Ok(Some(longest.clone())));
        let err = notice(&format!("{longest}x")).unwrap_err();
        assert!(err.contains("RIM_INSTANCE_NOTICE"), "{err}");
        assert!(err.contains("501"), "{err}");
    }

    #[test]
    fn terms_url_takes_only_an_absolute_http_or_https_url() {
        let terms = |value: &str| {
            RIMConfig {
                terms_url: Some(value.to_string()),
                ..Default::default()
            }
            .terms_url()
        };
        assert_eq!(RIMConfig::default().terms_url(), Ok(None));
        assert_eq!(terms(" "), Ok(None));
        for url in [
            "https://rim.example.org/terms",
            "http://localhost:8080/terms.html",
        ] {
            assert_eq!(terms(&format!(" {url} ")), Ok(Some(url.to_string())));
        }
        for bad in [
            "/terms",
            "rim.example.org/terms",
            "ftp://rim.example.org/terms",
            "mailto:legal@example.org",
            "https://",
        ] {
            let err = terms(bad).unwrap_err();
            assert!(err.contains("RIM_TERMS_URL"), "{bad}: {err}");
        }
        let config = RIMConfig {
            terms_url: Some("/terms".to_string()),
            ..Default::default()
        };
        assert!(config.instance_policy().is_err());
    }

    #[test]
    fn instance_notice_and_terms_url_are_read_from_their_variables() {
        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            (
                "RIM_INSTANCE_NOTICE",
                "Run by the Example club, rules at /rules",
            ),
            ("RIM_TERMS_URL", "https://rim.example.org/terms"),
        ]))
        .expect("loads");
        assert_eq!(
            config.instance_policy(),
            Ok(rim_api::types::InstancePolicy {
                notice: Some("Run by the Example club, rules at /rules".to_string()),
                terms_url: Some("https://rim.example.org/terms".to_string()),
            })
        );
    }

    #[test]
    fn instance_limits_are_off_when_unset_or_zero_but_devices_and_dm_requests_keep_their_quota() {
        use rim_services::limits::InstanceLimits;
        let relay_only = InstanceLimits {
            devices_per_user: Some(32),
            dm_requests_per_day: Some(20),
            ..Default::default()
        };
        assert_eq!(RIMConfig::default().instance_limits(), Ok(relay_only));
        let zero = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("RIM_LIMIT_WORKSPACES_PER_USER", "0"),
            ("RIM_LIMIT_MEMBERS_PER_WORKSPACE", "0"),
            ("RIM_LIMIT_CHANNELS_PER_WORKSPACE", "0"),
            ("RIM_LIMIT_DEVICES_PER_USER", "0"),
            ("RIM_LIMIT_DM_REQUESTS_PER_DAY", "0"),
            ("RIM_LIMIT_MACROS_PER_WORKSPACE", "0"),
            ("RIM_LIMIT_PIPELINES_PER_WORKSPACE", "0"),
            ("RIM_LIMIT_PLUGINS_PER_WORKSPACE", "0"),
            ("RIM_LIMIT_INVITES_PER_USER_PER_DAY", "0"),
            ("RIM_LIMIT_INVITE_MAX_USES", "0"),
            ("RIM_LIMIT_MESSAGE_BYTES", "0"),
        ]))
        .expect("loads");
        assert_eq!(zero.instance_limits(), Ok(relay_only));
        assert_eq!(zero.quotas(), RIMConfig::default().quotas());
    }

    #[test]
    fn instance_limits_are_read_from_their_variables() {
        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("RIM_LIMIT_WORKSPACES_PER_USER", "5"),
            ("RIM_LIMIT_MEMBERS_PER_WORKSPACE", "50"),
            ("RIM_LIMIT_CHANNELS_PER_WORKSPACE", "30"),
            ("RIM_LIMIT_DEVICES_PER_USER", "5"),
            ("RIM_LIMIT_DM_REQUESTS_PER_DAY", "20"),
            ("RIM_LIMIT_MACROS_PER_WORKSPACE", "50"),
            ("RIM_LIMIT_PIPELINES_PER_WORKSPACE", "50"),
            ("RIM_LIMIT_PLUGINS_PER_WORKSPACE", "50"),
            ("RIM_LIMIT_INVITES_PER_USER_PER_DAY", "5"),
            ("RIM_LIMIT_INVITE_MAX_USES", "5"),
            ("RIM_LIMIT_MESSAGE_BYTES", "8192"),
        ]))
        .expect("loads");
        assert_eq!(
            config.instance_limits(),
            Ok(rim_services::limits::InstanceLimits {
                workspaces_per_user: Some(5),
                members_per_workspace: Some(50),
                channels_per_workspace: Some(30),
                devices_per_user: Some(5),
                dm_requests_per_day: Some(20),
                macros_per_workspace: Some(50),
                pipelines_per_workspace: Some(50),
                plugins_per_workspace: Some(50),
                invites_per_user_per_day: Some(5),
                invite_max_uses: Some(5),
                message_bytes: Some(8192),
                probation_hours: None,
            })
        );
    }

    #[test]
    fn a_negative_or_non_numeric_limit_refuses_to_load() {
        for value in ["-1", "five", "1.5"] {
            for name in [
                "RIM_LIMIT_WORKSPACES_PER_USER",
                "RIM_LIMIT_DEVICES_PER_USER",
                "RIM_LIMIT_MESSAGE_BYTES",
            ] {
                let loaded = RIMConfig::load_from(env(&[
                    ("DATABASE_URL", "postgres://localhost/rim"),
                    (name, value),
                ]));
                assert!(loaded.is_err(), "{name}={value}");
            }
        }
    }

    #[test]
    fn message_bytes_above_the_largest_message_refuses_to_start() {
        let bytes = |n| RIMConfig {
            limit_message_bytes: Some(n),
            ..Default::default()
        };
        assert_eq!(
            bytes(65_536).instance_limits().map(|l| l.message_bytes),
            Ok(Some(65_536))
        );
        let err = bytes(65_537).instance_limits().unwrap_err();
        assert!(err.contains("RIM_LIMIT_MESSAGE_BYTES"), "{err}");
    }

    #[test]
    fn the_stricter_of_a_limit_and_its_relay_quota_applies_and_is_reported() {
        for (limit, quota, effective) in [(Some(5), 32, 5), (Some(50), 10, 10), (None, 10, 10)] {
            let config = RIMConfig {
                limit_devices_per_user: limit,
                relay_devices_per_user: Some(quota),
                limit_dm_requests_per_day: limit,
                relay_dm_requests_per_day: Some(quota),
                ..Default::default()
            };
            let quotas = config.quotas();
            assert_eq!(
                quotas.devices_per_user, effective as usize,
                "{limit:?} {quota}"
            );
            assert_eq!(quotas.dm_requests_per_day, effective, "{limit:?} {quota}");
            let limits = config.instance_limits().expect("limits");
            assert_eq!(limits.devices_per_user, Some(effective));
            assert_eq!(limits.dm_requests_per_day, Some(effective));
        }
    }

    #[test]
    fn plugins_per_workspace_stays_within_the_built_in_registry_cap() {
        let config = RIMConfig {
            limit_plugins_per_workspace: Some(500),
            ..Default::default()
        };
        assert_eq!(
            config.instance_limits().map(|l| l.plugins_per_workspace),
            Ok(Some(100))
        );
    }

    #[test]
    fn probation_is_off_when_unset_or_zero_and_read_with_its_message_rate() {
        use rim_infra_services::token_bucket::BucketRate;
        assert_eq!(
            RIMConfig::default()
                .instance_limits()
                .map(|l| l.probation_hours),
            Ok(None)
        );
        assert_eq!(RIMConfig::default().quotas().probation_messages, None);
        let load = |hours: &str, rate: &str| {
            RIMConfig::load_from(env(&[
                ("DATABASE_URL", "postgres://localhost/rim"),
                ("RIM_PROBATION_HOURS", hours),
                ("RIM_RATE_PROBATION_MESSAGES_PER_MIN", rate),
            ]))
        };
        let off = load("0", "0").expect("loads");
        assert_eq!(off.instance_limits().map(|l| l.probation_hours), Ok(None));
        assert_eq!(off.quotas().probation_messages, None);
        let on = load("24", "5").expect("loads");
        assert_eq!(
            on.instance_limits().map(|l| l.probation_hours),
            Ok(Some(24))
        );
        assert_eq!(on.quotas().probation_messages, BucketRate::new(5, 5));
        for bad in ["-1", "a day"] {
            assert!(load(bad, "5").is_err(), "RIM_PROBATION_HOURS={bad}");
            assert!(
                load("24", bad).is_err(),
                "RIM_RATE_PROBATION_MESSAGES_PER_MIN={bad}"
            );
        }
    }

    #[test]
    fn reserved_names_are_read_from_their_variable_and_an_empty_entry_is_refused() {
        assert_eq!(
            RIMConfig::default()
                .reserved_names()
                .map(|r| r.names().len()),
            Ok(0)
        );
        let config = RIMConfig::load_from(env(&[
            ("DATABASE_URL", "postgres://localhost/rim"),
            ("RIM_RESERVED_NAMES", "rim, Admin ,moderator"),
        ]))
        .expect("loads");
        assert_eq!(
            config.reserved_names().expect("names").names(),
            ["rim", "admin", "moderator"]
        );
        let config = RIMConfig {
            reserved_names: Some("rim,,admin".to_string()),
            ..Default::default()
        };
        let err = config.reserved_names().unwrap_err();
        assert!(err.contains("RIM_RESERVED_NAMES"), "{err}");
        assert!(err.contains("entry 2 is empty"), "{err}");
    }
}
