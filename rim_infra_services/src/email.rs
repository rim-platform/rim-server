use async_trait::async_trait;
use tracing::info;

#[async_trait]
pub trait EmailSender: Send + Sync {
    async fn send_password_reset(&self, username: &str, token: &str) -> anyhow::Result<()>;
}

pub struct LogEmailSender;

#[async_trait]
impl EmailSender for LogEmailSender {
    async fn send_password_reset(&self, username: &str, token: &str) -> anyhow::Result<()> {
        info!(
            username = %username,
            "PASSWORD RESET TOKEN (log-only, no email configured): {}",
            token,
        );
        Ok(())
    }
}
