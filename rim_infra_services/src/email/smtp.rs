use std::future::Future;
use std::str::FromStr;
use std::time::Duration;

use async_trait::async_trait;
use lettre::message::{Mailbox, Message, MultiPart};
use lettre::transport::smtp::AsyncSmtpTransport;
use lettre::transport::smtp::authentication::Credentials;
use lettre::{AsyncTransport, Tokio1Executor};
use tracing::{error, warn};

use super::{EmailMessage, EmailSender};

/// How the SMTP connection negotiates TLS.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SmtpTlsMode {
    /// Connect in plaintext, then upgrade via `STARTTLS` (the default; standard for port 587).
    Starttls,
    /// Connect already wrapped in TLS from the first byte (standard for port 465).
    Implicit,
    /// No TLS at all. Only appropriate for a trusted local relay (dev, mailpit, localhost
    /// container) — never use against a real mail server.
    None,
}

impl SmtpTlsMode {
    /// Parse the `RIM_SMTP_TLS` value (case-insensitive). Unrecognized values fall back to
    /// [`Self::Starttls`] (the safe default) rather than erroring, since a typo here should not
    /// take the whole server down at boot.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::email::SmtpTlsMode;
    /// assert_eq!(SmtpTlsMode::parse(Some("implicit")), SmtpTlsMode::Implicit);
    /// assert_eq!(SmtpTlsMode::parse(Some("NONE")), SmtpTlsMode::None);
    /// assert_eq!(SmtpTlsMode::parse(None), SmtpTlsMode::Starttls);
    /// assert_eq!(SmtpTlsMode::parse(Some("bogus")), SmtpTlsMode::Starttls);
    /// ```
    pub fn parse(raw: Option<&str>) -> Self {
        match raw.map(str::to_ascii_lowercase).as_deref() {
            Some("implicit") => Self::Implicit,
            Some("none") => Self::None,
            _ => Self::Starttls,
        }
    }
}

impl FromStr for SmtpTlsMode {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::parse(Some(s)))
    }
}

/// Resolved SMTP configuration — a plain data struct with no dependency on `rim_server`'s
/// config type (layering: `rim_server` depends on `rim_infra_services`, never the reverse). The
/// composition root (`rim_server/src/app.rs`) builds this from `RIM_SMTP_*` env vars.
#[derive(Clone)]
pub struct SmtpConfig {
    pub host: String,
    pub port: u16,
    pub username: Option<String>,
    pub password: Option<String>,
    /// The `From:` address/mailbox, e.g. `"RIM <noreply@example.com>"` or a bare address.
    pub from: String,
    pub tls: SmtpTlsMode,
}

/// Hand-written to redact `username` and `password`: this struct is logged/inspected in
/// debugging contexts (e.g. `{:?}` in error messages, ad-hoc `dbg!()`), and the derived `Debug`
/// would otherwise print the plaintext SMTP credentials.
impl std::fmt::Debug for SmtpConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SmtpConfig")
            .field("host", &self.host)
            .field("port", &self.port)
            .field("username", &self.username.as_ref().map(|_| "[REDACTED]"))
            .field("password", &self.password.as_ref().map(|_| "[REDACTED]"))
            .field("from", &self.from)
            .field("tls", &self.tls)
            .finish()
    }
}

impl SmtpConfig {
    /// Build a config from individual `RIM_SMTP_*` values, applying the "host+from required"
    /// rule: returns `None` (fall back to [`super::LogEmailSender`]) unless
    /// both `host` and `from` are present. `username`/`password` may be absent (unauthenticated
    /// relay / localhost dev SMTP); `port` defaults to 587; `tls` defaults to
    /// [`SmtpTlsMode::Starttls`] on any unrecognized/absent value.
    ///
    /// # Examples
    /// ```
    /// use rim_infra_services::email::SmtpConfig;
    /// assert!(SmtpConfig::from_parts(None, None, None, None, None, None).is_none());
    /// assert!(
    ///     SmtpConfig::from_parts(
    ///         Some("smtp.example.com".to_string()),
    ///         None,
    ///         None,
    ///         None,
    ///         Some("RIM <noreply@example.com>".to_string()),
    ///         None,
    ///     )
    ///     .is_some()
    /// );
    /// ```
    pub fn from_parts(
        host: Option<String>,
        port: Option<u16>,
        username: Option<String>,
        password: Option<String>,
        from: Option<String>,
        tls: Option<String>,
    ) -> Option<Self> {
        let host = host?;
        let from = from?;
        Some(Self {
            host,
            port: port.unwrap_or(587),
            username,
            password,
            from,
            tls: SmtpTlsMode::parse(tls.as_deref()),
        })
    }
}

/// Max send attempts (1 initial + up to 2 retries).
const MAX_ATTEMPTS: u32 = 3;
/// Base backoff delay; doubles each retry, capped at [`MAX_BACKOFF`].
const BASE_BACKOFF: Duration = Duration::from_millis(500);
const MAX_BACKOFF: Duration = Duration::from_secs(5);

/// Bounded exponential-backoff retry loop, generic over the operation so the retry *policy*
/// (attempt counting, backoff, transient-vs-permanent short-circuit) is unit-testable without a
/// live or fake SMTP transport — `lettre::transport::smtp::Error` has no public constructor, so
/// a test can't synthesize one; a plain closure returning `(E, is_permanent)` on failure can.
///
/// Retries up to [`MAX_ATTEMPTS`] total attempts, only while `is_permanent` is `false`. Sleeps
/// `BASE_BACKOFF * 2^(attempt-1)` (capped at [`MAX_BACKOFF`]) between attempts.
///
/// # Examples
/// ```ignore
/// let result = send_with_retry(|| async { transport.send(email.clone()).await }).await;
/// ```
async fn send_with_retry<F, Fut, T, E>(mut op: F) -> Result<T, E>
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Result<T, (E, bool)>>,
    E: std::fmt::Display,
{
    let mut attempt: u32 = 1;
    loop {
        match op().await {
            Ok(value) => return Ok(value),
            Err((err, is_permanent)) if !is_permanent && attempt < MAX_ATTEMPTS => {
                let backoff = BASE_BACKOFF
                    .saturating_mul(1 << (attempt - 1))
                    .min(MAX_BACKOFF);
                warn!(
                    attempt,
                    max_attempts = MAX_ATTEMPTS,
                    backoff_ms = backoff.as_millis() as u64,
                    error = %err,
                    "transient send failure, retrying"
                );
                tokio::time::sleep(backoff).await;
                attempt += 1;
            }
            Err((err, _)) => {
                error!(attempt, error = %err, "send failed (final)");
                return Err(err);
            }
        }
    }
}

/// Real SMTP sender, `lettre` + rustls (pure-rustls stack — no native-tls/OpenSSL feature is
/// enabled, matching the rest of this workspace). Retries transient failures via
/// [`send_with_retry`]; permanent failures (invalid recipient, auth failure) return immediately
/// without retrying — see [`Self::is_permanent`] for the classification.
pub struct SmtpEmailSender {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Mailbox,
}

impl SmtpEmailSender {
    /// Build the underlying `lettre` transport from `config` and parse `config.from` as a
    /// [`Mailbox`].
    ///
    /// # Errors
    /// Returns an error if `config.from` is not a valid mailbox address, or if the transport
    /// fails to build (e.g. malformed host).
    ///
    /// # Examples
    /// ```ignore
    /// use rim_infra_services::email::{SmtpConfig, SmtpEmailSender, SmtpTlsMode};
    /// let config = SmtpConfig {
    ///     host: "smtp.example.com".to_string(),
    ///     port: 587,
    ///     username: Some("user".to_string()),
    ///     password: Some("pass".to_string()),
    ///     from: "RIM <noreply@example.com>".to_string(),
    ///     tls: SmtpTlsMode::Starttls,
    /// };
    /// let sender = SmtpEmailSender::new(config)?;
    /// ```
    pub fn new(config: SmtpConfig) -> anyhow::Result<Self> {
        let from: Mailbox = config.from.parse()?;

        let mut builder = match config.tls {
            SmtpTlsMode::Starttls => {
                AsyncSmtpTransport::<Tokio1Executor>::starttls_relay(&config.host)?
            }
            SmtpTlsMode::Implicit => AsyncSmtpTransport::<Tokio1Executor>::relay(&config.host)?,
            SmtpTlsMode::None => {
                AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(&config.host)
            }
        }
        .port(config.port);

        if let (Some(username), Some(password)) = (config.username, config.password) {
            builder = builder.credentials(Credentials::new(username, password));
        }

        Ok(Self {
            transport: builder.build(),
            from,
        })
    }

    fn build_message(&self, message: &EmailMessage) -> anyhow::Result<Message> {
        let to: Mailbox = message.to.parse()?;

        let builder = Message::builder()
            .from(self.from.clone())
            .to(to)
            .subject(&message.subject);

        let email = match &message.body_html {
            Some(html) => builder.multipart(MultiPart::alternative_plain_html(
                message.body_text.clone(),
                html.clone(),
            ))?,
            None => builder.body(message.body_text.clone())?,
        };

        Ok(email)
    }

    /// Whether a `lettre` SMTP error is permanent. Permanent errors (5xx SMTP replies —
    /// invalid recipient, auth failure) are never retried. Everything else (4xx transient
    /// replies, connection/network/TLS errors) is retried, since those are the class of failure
    /// that can plausibly clear up on its own within a few seconds.
    fn is_permanent(err: &lettre::transport::smtp::Error) -> bool {
        err.is_permanent()
    }
}

#[async_trait]
impl EmailSender for SmtpEmailSender {
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()> {
        let email = self.build_message(&message)?;

        send_with_retry(|| async {
            self.transport
                .send(email.clone())
                .await
                .map(|_| ())
                .map_err(|e| {
                    let is_permanent = Self::is_permanent(&e);
                    (e, is_permanent)
                })
        })
        .await?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    // ── SmtpTlsMode::parse ───────────────────────────────────────────────────

    #[test]
    fn tls_mode_parse_recognizes_all_variants_case_insensitively() {
        assert_eq!(SmtpTlsMode::parse(Some("starttls")), SmtpTlsMode::Starttls);
        assert_eq!(SmtpTlsMode::parse(Some("STARTTLS")), SmtpTlsMode::Starttls);
        assert_eq!(SmtpTlsMode::parse(Some("implicit")), SmtpTlsMode::Implicit);
        assert_eq!(SmtpTlsMode::parse(Some("IMPLICIT")), SmtpTlsMode::Implicit);
        assert_eq!(SmtpTlsMode::parse(Some("none")), SmtpTlsMode::None);
        assert_eq!(SmtpTlsMode::parse(Some("NONE")), SmtpTlsMode::None);
    }

    #[test]
    fn tls_mode_parse_defaults_to_starttls_on_unset_or_unknown() {
        assert_eq!(SmtpTlsMode::parse(None), SmtpTlsMode::Starttls);
        assert_eq!(SmtpTlsMode::parse(Some("bogus")), SmtpTlsMode::Starttls);
    }

    // ── SmtpConfig::Debug redaction ──────────────────────────────────────────

    #[test]
    fn debug_redacts_username_and_password() {
        let config = SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 587,
            username: Some("supersecretuser".to_string()),
            password: Some("supersecretpass".to_string()),
            from: "RIM <noreply@example.com>".to_string(),
            tls: SmtpTlsMode::Starttls,
        };
        let debug_str = format!("{config:?}");
        assert!(!debug_str.contains("supersecretuser"));
        assert!(!debug_str.contains("supersecretpass"));
        assert!(debug_str.contains("[REDACTED]"));
        assert!(
            debug_str.contains("smtp.example.com"),
            "non-secret fields must still print"
        );
    }

    #[test]
    fn debug_shows_none_for_absent_credentials() {
        let config = SmtpConfig {
            host: "smtp.example.com".to_string(),
            port: 587,
            username: None,
            password: None,
            from: "RIM <noreply@example.com>".to_string(),
            tls: SmtpTlsMode::Starttls,
        };
        let debug_str = format!("{config:?}");
        assert!(!debug_str.contains("[REDACTED]"));
    }

    // ── SmtpConfig::from_parts ───────────────────────────────────────────────

    #[test]
    fn from_parts_none_when_host_absent() {
        assert!(
            SmtpConfig::from_parts(
                None,
                None,
                None,
                None,
                Some("noreply@example.com".to_string()),
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn from_parts_none_when_from_absent() {
        assert!(
            SmtpConfig::from_parts(
                Some("smtp.example.com".to_string()),
                None,
                None,
                None,
                None,
                None,
            )
            .is_none()
        );
    }

    #[test]
    fn from_parts_some_when_only_host_and_from_present() {
        let config = SmtpConfig::from_parts(
            Some("smtp.example.com".to_string()),
            None,
            None,
            None,
            Some("noreply@example.com".to_string()),
            None,
        )
        .unwrap();
        assert_eq!(config.host, "smtp.example.com");
        assert_eq!(config.port, 587, "port defaults to 587");
        assert_eq!(
            config.tls,
            SmtpTlsMode::Starttls,
            "tls defaults to starttls"
        );
        assert!(config.username.is_none());
        assert!(config.password.is_none());
    }

    #[test]
    fn from_parts_preserves_explicit_values() {
        let config = SmtpConfig::from_parts(
            Some("smtp.example.com".to_string()),
            Some(465),
            Some("user".to_string()),
            Some("pass".to_string()),
            Some("RIM <noreply@example.com>".to_string()),
            Some("implicit".to_string()),
        )
        .unwrap();
        assert_eq!(config.port, 465);
        assert_eq!(config.username.as_deref(), Some("user"));
        assert_eq!(config.password.as_deref(), Some("pass"));
        assert_eq!(config.tls, SmtpTlsMode::Implicit);
    }

    // ── message building ─────────────────────────────────────────────────────

    fn text_only_sender() -> SmtpEmailSender {
        SmtpEmailSender::new(SmtpConfig {
            host: "localhost".to_string(),
            port: 2525,
            username: None,
            password: None,
            from: "RIM <noreply@example.com>".to_string(),
            tls: SmtpTlsMode::None,
        })
        .unwrap()
    }

    // These three tests are `#[tokio::test]` (not plain `#[test]`), even though
    // `build_message` itself is synchronous: `AsyncSmtpTransport`'s connection pool spawns a
    // background cleanup task via `Tokio1Executor` in its `Drop` impl, which panics
    // ("no reactor running") if `sender` is dropped outside a Tokio runtime context.

    #[tokio::test]
    async fn build_message_text_only() {
        let sender = text_only_sender();
        let msg = EmailMessage {
            to: "user@example.com".to_string(),
            subject: "hello".to_string(),
            body_text: "plain body".to_string(),
            body_html: None,
        };
        let built = sender.build_message(&msg).unwrap();
        let formatted = String::from_utf8_lossy(&built.formatted()).into_owned();
        assert!(formatted.contains("hello"), "subject must be present");
        assert!(
            formatted.contains("plain body"),
            "text body must be present"
        );
    }

    #[tokio::test]
    async fn build_message_multipart_when_html_present() {
        let sender = text_only_sender();
        let msg = EmailMessage {
            to: "user@example.com".to_string(),
            subject: "hello".to_string(),
            body_text: "plain body".to_string(),
            body_html: Some("<p>html body</p>".to_string()),
        };
        let built = sender.build_message(&msg);
        assert!(built.is_ok());
    }

    #[tokio::test]
    async fn build_message_rejects_invalid_recipient() {
        let sender = text_only_sender();
        let msg = EmailMessage {
            to: "not-an-email".to_string(),
            subject: "hello".to_string(),
            body_text: "plain body".to_string(),
            body_html: None,
        };
        assert!(sender.build_message(&msg).is_err());
    }

    // ── retry policy (send_with_retry) ────────────────────────────────────────

    #[tokio::test]
    async fn retries_transient_failures_until_success() {
        let calls = AtomicU32::new(0);
        let result: Result<(), &str> = send_with_retry(|| async {
            let n = calls.fetch_add(1, Ordering::SeqCst) + 1;
            if n < 3 {
                Err(("transient", false))
            } else {
                Ok(())
            }
        })
        .await;

        assert!(result.is_ok());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            3,
            "should retry twice then succeed"
        );
    }

    #[tokio::test]
    async fn does_not_retry_permanent_failures() {
        let calls = AtomicU32::new(0);
        let result: Result<(), &str> = send_with_retry(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(("permanent", true))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            1,
            "must not retry a permanent error"
        );
    }

    #[tokio::test]
    async fn gives_up_after_max_attempts_on_persistent_transient_failure() {
        let calls = AtomicU32::new(0);
        let result: Result<(), &str> = send_with_retry(|| async {
            calls.fetch_add(1, Ordering::SeqCst);
            Err(("always transient", false))
        })
        .await;

        assert!(result.is_err());
        assert_eq!(
            calls.load(Ordering::SeqCst),
            MAX_ATTEMPTS,
            "must stop after MAX_ATTEMPTS even though every failure is transient"
        );
    }
}
