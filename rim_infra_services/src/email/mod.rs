//! Transactional email delivery (Track B3). A generic `send(EmailMessage)` transport, kept
//! trivially mockable via [`EmailSender`], with two implementations: [`log::LogEmailSender`]
//! (dev/unconfigured fallback — logs, never delivers) and [`smtp::SmtpEmailSender`] (real SMTP
//! via `lettre`, rustls-only, with bounded retry on transient failures — see `smtp.rs`).
//!
//! Per-flow content lives in [`templates`] as small pure functions returning [`EmailMessage`],
//! so composing a message never touches I/O and is trivially unit-testable.
//!
//! See `docs/email.md` for the operator-facing config matrix and wiring overview.

pub mod log;
pub mod smtp;
pub mod templates;

use async_trait::async_trait;
#[cfg(test)]
use mockall::automock;

pub use log::LogEmailSender;
pub use smtp::{SmtpConfig, SmtpEmailSender, SmtpTlsMode};

/// One outbound email, transport-agnostic.
#[derive(Clone, PartialEq, Eq)]
pub struct EmailMessage {
    /// Recipient address.
    pub to: String,
    pub subject: String,
    /// Required plain-text body (every message must have one, even if `body_html` is set).
    pub body_text: String,
    /// Optional HTML alternative, sent as a multipart `text/plain` + `text/html` message when
    /// present.
    pub body_html: Option<String>,
}

/// Hand-written to redact `body_text`/`body_html`: these bodies carry raw, unhashed reset/invite
/// tokens embedded in a link (see `password_reset.rs`/`auth.rs`'s `build_reset_link`/
/// `build_invite_link`), so a derived `Debug` printed in a log line or error message would leak a
/// live, usable credential. `to` and `subject` are not secret and are kept visible for
/// debuggability.
impl std::fmt::Debug for EmailMessage {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("EmailMessage")
            .field("to", &self.to)
            .field("subject", &self.subject)
            .field("body_text", &"[REDACTED]")
            .field("body_html", &self.body_html.as_ref().map(|_| "[REDACTED]"))
            .finish()
    }
}

/// Generic email transport. Implementations retry transient failures internally (see
/// [`smtp::SmtpEmailSender`]) — callers should treat `Err` as a final, non-retryable failure and
/// decide for themselves whether to swallow it (every current caller does, since email delivery
/// is best-effort everywhere it's wired — see `docs/email.md`).
#[cfg_attr(test, automock)]
#[async_trait]
pub trait EmailSender: Send + Sync {
    /// Send one message. Returns `Err` only after retries are exhausted (transient errors) or
    /// immediately on a permanent error (e.g. invalid recipient, auth failure).
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression guard: `body_text`/`body_html` can carry a raw, usable reset/invite token
    /// embedded in a link — `Debug` must never print them.
    #[test]
    fn debug_redacts_bodies_but_keeps_to_and_subject() {
        let msg = EmailMessage {
            to: "user@example.com".to_string(),
            subject: "Reset your RIM password".to_string(),
            body_text:
                "Reset your password: https://rim.example.com/reset-password/rim_rst_secrettoken"
                    .to_string(),
            body_html: Some(
                "<a href=\"https://rim.example.com/reset-password/rim_rst_secrettoken\">Reset</a>"
                    .to_string(),
            ),
        };
        let debug_str = format!("{msg:?}");
        assert!(debug_str.contains("user@example.com"));
        assert!(debug_str.contains("Reset your RIM password"));
        assert!(!debug_str.contains("rim_rst_secrettoken"));
        assert!(debug_str.contains("[REDACTED]"));
    }

    #[test]
    fn debug_shows_none_when_body_html_absent() {
        let msg = EmailMessage {
            to: "user@example.com".to_string(),
            subject: "hi".to_string(),
            body_text: "plain".to_string(),
            body_html: None,
        };
        let debug_str = format!("{msg:?}");
        assert!(debug_str.contains("body_html: None"));
    }
}
