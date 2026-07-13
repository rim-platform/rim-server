use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use tracing::{info, instrument, warn};

use rim_auth::password::hash_password;
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::password_reset::model::NewPasswordResetToken;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::user::repository::UserRepository;
use rim_services::password_reset::command::{RequestPasswordResetCmd, ResetPasswordCmd};
use rim_services::password_reset::service::PasswordResetService;
use rim_services::{ServiceError, ServiceResult};

use crate::email::{EmailSender, templates};
use crate::url_encode::percent_encode_path_segment;

#[derive(Clone)]
pub struct PasswordResetRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub password_reset_token_repo: Arc<dyn PasswordResetTokenRepository>,
}

pub struct DefaultPasswordResetService {
    repos: PasswordResetRepos,
    email_sender: Arc<dyn EmailSender>,
    /// e.g. `https://rim.example.com`. Used to build the `/reset-password/{token}` link. If
    /// unset, the email falls back to the bare token (dev-only — see `docs/email.md`).
    public_base_url: Option<String>,
}

impl DefaultPasswordResetService {
    pub fn new(
        repos: PasswordResetRepos,
        email_sender: Arc<dyn EmailSender>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            email_sender,
            public_base_url,
        }
    }

    fn generate_reset_token() -> String {
        let t = generate_session_token();
        t.strip_prefix("rim_sess_")
            .map(|s| format!("rim_rst_{s}"))
            .unwrap_or_else(|| format!("rim_rst_{t}"))
    }

    /// Build the reset link as a path segment: `{public_base_url}/reset-password/{token}` — a
    /// path segment, never a `?token=` query string (query strings would both fail to route and
    /// leak the token via `Referer`; see `docs/email.md`). The token is percent-encoded
    /// defensively even though `rim_rst_`-prefixed base64url tokens are already path-safe. Falls
    /// back to the bare token (with a warning) if `public_base_url` is unset.
    fn build_reset_link(&self, raw_token: &str) -> String {
        let encoded = percent_encode_path_segment(raw_token);
        match &self.public_base_url {
            Some(base) => format!("{}/reset-password/{}", base.trim_end_matches('/'), encoded),
            None => {
                warn!(
                    "RIM_PUBLIC_BASE_URL is not configured; password reset email will contain \
                     the bare token instead of a clickable link"
                );
                raw_token.to_string()
            }
        }
    }
}

#[async_trait]
impl PasswordResetService for DefaultPasswordResetService {
    #[instrument(name = "password_reset.request", skip_all, fields(username = %cmd.username))]
    async fn request_reset(&self, cmd: RequestPasswordResetCmd) -> ServiceResult<()> {
        let user = self.repos.user_repo.find_by_username(&cmd.username).await?;

        let Some(user) = user else {
            // Don't reveal whether user exists — silently succeed.
            info!(username = %cmd.username, "password reset requested for unknown user");
            return Ok(());
        };

        let raw_token = Self::generate_reset_token();
        let token_hash = hash_token(&raw_token);
        let expires_at = Utc::now() + Duration::hours(1);

        self.repos
            .password_reset_token_repo
            .create(NewPasswordResetToken {
                user_id: user.id,
                token_hash,
                expires_at,
            })
            .await?;

        let link = self.build_reset_link(&raw_token);
        let message = templates::password_reset(&user.email, &link);

        // Spawned rather than awaited: awaiting the SMTP round-trip (plus retry/backoff on
        // transient failure — up to several seconds, see email/smtp.rs) here would make the
        // known-user response measurably slower than the unknown-user early return above,
        // letting a caller infer account existence from response timing. Fire-and-forget keeps
        // this response path's cost near-constant regardless of whether `username` exists.
        // Send failures remain non-fatal — logged, never surfaced to the caller.
        let email_sender = Arc::clone(&self.email_sender);
        tokio::spawn(async move {
            if let Err(e) = email_sender.send(message).await {
                warn!(error = %e, "failed to send password reset email");
            }
        });

        info!(user_id = %user.id, "password reset token created");
        Ok(())
    }

    #[instrument(name = "password_reset.reset", skip_all)]
    async fn reset_password(&self, cmd: ResetPasswordCmd) -> ServiceResult<()> {
        let token_hash = hash_token(&cmd.token);
        let record = self
            .repos
            .password_reset_token_repo
            .get_by_token_hash(&token_hash)
            .await?
            .ok_or(ServiceError::NotFound)?;

        if record.used_at.is_some() {
            return Err(ServiceError::PermissionDenied);
        }

        if record.expires_at < Utc::now() {
            return Err(ServiceError::PermissionDenied);
        }

        // Validate new password.
        if cmd.new_password.len() < 8 {
            return Err(ServiceError::Validation(
                "password must be at least 8 characters".to_string(),
            ));
        }

        let password_hash = hash_password(&cmd.new_password).map_err(ServiceError::internal)?;

        self.repos
            .user_repo
            .update_password(record.user_id, password_hash)
            .await?;

        self.repos
            .password_reset_token_repo
            .mark_used(record.id)
            .await?;

        info!(user_id = %record.user_id, "password reset completed");
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::password_reset::repository::MockPasswordResetTokenRepository;
    use rim_core::user::model::{User, UserId};
    use rim_core::user::repository::MockUserRepository;

    fn make_user(id: UserId, username: &str, email: &str) -> User {
        let now = Utc::now();
        User {
            id,
            username: username.to_string(),
            password_hash: "hash".to_string(),
            created_at: now,
            updated_at: now,
            last_login_at: None,
            display_name: None,
            avatar_url: None,
            status_text: None,
            bio: None,
            email: email.to_string(),
            timezone: None,
        }
    }

    fn build_service(
        user_repo: MockUserRepository,
        token_repo: MockPasswordResetTokenRepository,
        email_sender: crate::email::MockEmailSender,
        public_base_url: Option<String>,
    ) -> DefaultPasswordResetService {
        DefaultPasswordResetService::new(
            PasswordResetRepos {
                user_repo: Arc::new(user_repo),
                password_reset_token_repo: Arc::new(token_repo),
            },
            Arc::new(email_sender),
            public_base_url,
        )
    }

    #[tokio::test]
    async fn request_reset_sends_email_to_known_user_with_link() {
        let user_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(make_user(user_id, "alice", "alice@example.com"))));

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().returning(|new| {
            Ok(rim_core::password_reset::model::PasswordResetToken {
                id: rim_core::password_reset::model::PasswordResetTokenId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                expires_at: new.expires_at,
                used_at: None,
                created_at: Utc::now(),
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .withf(|msg| {
                msg.to == "alice@example.com"
                    && msg
                        .body_text
                        .contains("https://rim.example.com/reset-password/")
            })
            .times(1)
            .returning(|_| Ok(()));

        let svc = build_service(
            user_repo,
            token_repo,
            email_sender,
            Some("https://rim.example.com".to_string()),
        );

        let result = svc
            .request_reset(RequestPasswordResetCmd {
                username: "alice".to_string(),
            })
            .await;
        assert!(result.is_ok());

        // The send is spawned, not awaited inline (timing side-channel fix) — give the spawned
        // task a chance to run before the mock's expectations are checked on drop.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn request_reset_unknown_user_returns_ok_without_sending() {
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(|_| Ok(None));

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().never();

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender.expect_send().never();

        let svc = build_service(user_repo, token_repo, email_sender, None);

        let result = svc
            .request_reset(RequestPasswordResetCmd {
                username: "nobody".to_string(),
            })
            .await;
        assert!(
            result.is_ok(),
            "must not reveal whether the user exists via an error"
        );
    }

    #[tokio::test]
    async fn request_reset_send_failure_is_non_fatal() {
        let user_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(make_user(user_id, "alice", "alice@example.com"))));

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().returning(|new| {
            Ok(rim_core::password_reset::model::PasswordResetToken {
                id: rim_core::password_reset::model::PasswordResetTokenId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                expires_at: new.expires_at,
                used_at: None,
                created_at: Utc::now(),
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender
            .expect_send()
            .returning(|_| Err(anyhow::anyhow!("smtp down")));

        let svc = build_service(user_repo, token_repo, email_sender, None);

        let result = svc
            .request_reset(RequestPasswordResetCmd {
                username: "alice".to_string(),
            })
            .await;
        assert!(
            result.is_ok(),
            "email send failure must not fail the request"
        );

        // Let the spawned send run to completion (and log its failure) before the mock's
        // expectations are checked on drop.
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    /// A hand-written (non-mockall) `EmailSender` that genuinely awaits before resolving,
    /// simulating a slow SMTP round-trip. Used instead of `MockEmailSender` for the timing test
    /// below because `MockEmailSender`'s `.returning()` closures run synchronously (no real
    /// `.await` point), so they can't inject an actual delay to observe.
    struct SlowEmailSender;

    #[async_trait]
    impl EmailSender for SlowEmailSender {
        async fn send(&self, _message: crate::email::EmailMessage) -> anyhow::Result<()> {
            tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            Ok(())
        }
    }

    /// Regression guard for the timing side-channel fix: `request_reset` must return without
    /// waiting on the email send. A slow/hanging `EmailSender` (e.g. a stalled SMTP connection)
    /// must not measurably slow down the known-user response relative to the unknown-user path,
    /// or a caller could infer account existence purely from response latency.
    #[tokio::test]
    async fn request_reset_does_not_await_the_email_send() {
        let user_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .returning(move |_| Ok(Some(make_user(user_id, "alice", "alice@example.com"))));

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().returning(|new| {
            Ok(rim_core::password_reset::model::PasswordResetToken {
                id: rim_core::password_reset::model::PasswordResetTokenId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                expires_at: new.expires_at,
                used_at: None,
                created_at: Utc::now(),
            })
        });

        let svc = DefaultPasswordResetService::new(
            PasswordResetRepos {
                user_repo: Arc::new(user_repo),
                password_reset_token_repo: Arc::new(token_repo),
            },
            Arc::new(SlowEmailSender),
            None,
        );

        let start = std::time::Instant::now();
        let result = svc
            .request_reset(RequestPasswordResetCmd {
                username: "alice".to_string(),
            })
            .await;
        let elapsed = start.elapsed();

        assert!(result.is_ok());
        assert!(
            elapsed < std::time::Duration::from_secs(1),
            "request_reset must return well before the (5s-simulated) email send completes, \
             got {elapsed:?}"
        );
    }

    #[test]
    fn build_reset_link_uses_public_base_url_when_set() {
        let svc = build_service(
            MockUserRepository::new(),
            MockPasswordResetTokenRepository::new(),
            crate::email::MockEmailSender::new(),
            Some("https://rim.example.com".to_string()),
        );
        assert_eq!(
            svc.build_reset_link("rim_rst_abc123"),
            "https://rim.example.com/reset-password/rim_rst_abc123"
        );
    }

    #[test]
    fn build_reset_link_falls_back_to_bare_token_when_base_url_unset() {
        let svc = build_service(
            MockUserRepository::new(),
            MockPasswordResetTokenRepository::new(),
            crate::email::MockEmailSender::new(),
            None,
        );
        assert_eq!(svc.build_reset_link("rim_rst_abc123"), "rim_rst_abc123");
    }
}
