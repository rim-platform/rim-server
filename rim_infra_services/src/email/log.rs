use async_trait::async_trait;
use tracing::info;

use super::{EmailMessage, EmailSender};

/// Dev/unconfigured fallback sender: logs that an email *would* be sent, never delivers.
///
/// Used whenever `RIM_SMTP_*` is not configured (see `docs/email.md`). Deliberately logs only
/// `to`/`subject` at `info` level — never the body — so this never leaks a password-reset or
/// invite token into production logs, unlike the pre-Track-B3 stub it replaces.
pub struct LogEmailSender;

#[async_trait]
impl EmailSender for LogEmailSender {
    /// Always succeeds; logs `to` and `subject` at `info`.
    ///
    /// # Examples
    /// ```ignore
    /// use rim_infra_services::email::{EmailMessage, EmailSender, LogEmailSender};
    /// let sender = LogEmailSender;
    /// sender.send(EmailMessage {
    ///     to: "user@example.com".to_string(),
    ///     subject: "hello".to_string(),
    ///     body_text: "secret token here".to_string(),
    ///     body_html: None,
    /// }).await.unwrap();
    /// ```
    async fn send(&self, message: EmailMessage) -> anyhow::Result<()> {
        info!(
            to = %message.to,
            subject = %message.subject,
            "email would be sent (log-only, no SMTP configured)"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn send_always_succeeds() {
        let sender = LogEmailSender;
        let result = sender
            .send(EmailMessage {
                to: "user@example.com".to_string(),
                subject: "test".to_string(),
                body_text: "body".to_string(),
                body_html: None,
            })
            .await;
        assert!(result.is_ok());
    }
}
