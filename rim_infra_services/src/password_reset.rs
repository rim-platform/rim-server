use std::sync::Arc;

use async_trait::async_trait;
use chrono::{Duration, Utc};
use sea_orm::DatabaseConnection;
use tracing::{info, instrument, warn};

use rim_auth::password::hash_password;
use rim_auth::session_token::{generate_session_token, hash_token};
use rim_core::auth::repository::SessionRepository;
use rim_core::password_reset::model::NewPasswordResetToken;
use rim_core::password_reset::repository::PasswordResetTokenRepository;
use rim_core::user::repository::UserRepository;
use rim_infra_db::begin_transaction;
use rim_services::command::rate_limit::RateLimitDecision;
use rim_services::password_reset::command::{RequestPasswordResetCmd, ResetPasswordCmd};
use rim_services::password_reset::service::PasswordResetService;
use rim_services::{ServiceError, ServiceResult};

use crate::email::{EmailSender, templates};
use crate::login_sources::KnownLoginSources;
use crate::registration_rate_limiter::RegistrationRateLimiter;
use crate::repos::AppRepos;
use crate::url_encode::percent_encode_path_segment;

/// Fixed "IP" slot for the per-username reset limiter, whose buckets are keyed by username alone.
const RESET_ACCOUNT_SCOPE: &str = "account";

#[derive(Clone)]
pub struct PasswordResetRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub password_reset_token_repo: Arc<dyn PasswordResetTokenRepository>,
    pub session_repo: Arc<dyn SessionRepository>,
}

pub struct DefaultPasswordResetService {
    repos: PasswordResetRepos,
    /// When set, `reset_password` runs its writes in one transaction.
    db: Option<DatabaseConnection>,
    ip_rate_limiter: RegistrationRateLimiter,
    account_rate_limiter: RegistrationRateLimiter,
    email_sender: Arc<dyn EmailSender>,
    /// e.g. `https://rim.example.com`. Used to build the `/reset-password/{token}` link. If
    /// unset, the email falls back to the bare token (dev-only — see `docs/email.md`).
    public_base_url: Option<String>,
    /// Login's store of recent successful sources; a completed reset forgets the account's.
    known_login_sources: Option<KnownLoginSources>,
}

impl DefaultPasswordResetService {
    pub fn new(
        repos: PasswordResetRepos,
        db: Option<DatabaseConnection>,
        email_sender: Arc<dyn EmailSender>,
        public_base_url: Option<String>,
    ) -> Self {
        Self {
            repos,
            db,
            ip_rate_limiter: RegistrationRateLimiter::password_reset_ip_default(),
            account_rate_limiter: RegistrationRateLimiter::password_reset_account_default(),
            email_sender,
            public_base_url,
            known_login_sources: None,
        }
    }

    /// Forget the account's recent login sources on each completed reset, so addresses an
    /// attacker signed in from with the old password lose their login-limit exemption.
    pub(crate) fn with_known_login_sources(mut self, sources: KnownLoginSources) -> Self {
        self.known_login_sources = Some(sources);
        self
    }

    /// Replace the request limiters (per-IP requests, per-username requests).
    #[cfg(test)]
    fn with_rate_limiters(
        mut self,
        ip_limiter: RegistrationRateLimiter,
        account_limiter: RegistrationRateLimiter,
    ) -> Self {
        self.ip_rate_limiter = ip_limiter;
        self.account_rate_limiter = account_limiter;
        self
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
        let RequestPasswordResetCmd {
            username,
            remote_ip,
        } = cmd;
        let now = Utc::now();

        // A caller over the per-IP limit is told so; it is the one making the requests.
        if let RateLimitDecision::Limited {
            retry_after_seconds,
        } = self
            .ip_rate_limiter
            .check_and_record(&remote_ip, "password_reset", now)
        {
            warn!(
                target = "audit",
                event = "password_reset_rate_limited",
                remote_ip = %remote_ip,
                retry_after_seconds = %retry_after_seconds,
                "password reset request rate limited by IP"
            );
            return Err(ServiceError::RateLimited {
                retry_after_seconds,
            });
        }

        // Over the per-username limit, succeed silently without a new token or email: the
        // account holder already has the earlier, still-valid links, and an error here would
        // only let a third party block their resets. The limit applies whether or not the
        // username exists, so it reveals nothing either.
        if let RateLimitDecision::Limited { .. } = self.account_rate_limiter.check_and_record(
            RESET_ACCOUNT_SCOPE,
            &username.to_lowercase(),
            now,
        ) {
            info!(
                target = "audit",
                event = "password_reset_suppressed",
                username = %username,
                "password reset request over the per-username limit; no email sent"
            );
            return Ok(());
        }

        let user = self.repos.user_repo.find_by_username(&username).await?;

        let Some(user) = user else {
            // Don't reveal whether user exists — silently succeed.
            info!(username = %username, "password reset requested for unknown user");
            return Ok(());
        };

        // A password no longer signs in to an account with a key, so no link is sent; the
        // answer is the same as for an unknown user.
        if user.principal_id.is_some() {
            info!(user_id = %user.id, "password reset requested for a key account; ignored");
            return Ok(());
        }

        let raw_token = Self::generate_reset_token();
        let token_hash = hash_token(&raw_token);
        let expires_at = now + Duration::hours(1);
        let link = self.build_reset_link(&raw_token);
        let message = templates::password_reset(&user.email, &link);

        // Token creation and the email send are spawned rather than awaited, so a known
        // username returns after the same single lookup as an unknown one. Awaiting the insert,
        // or the SMTP round-trip with its retry/backoff (up to several seconds, see
        // email/smtp.rs), would let a caller infer account existence from response timing.
        // Failures are logged, never surfaced to the caller.
        let token_repo = Arc::clone(&self.repos.password_reset_token_repo);
        let email_sender = Arc::clone(&self.email_sender);
        let user_id = user.id;
        tokio::spawn(async move {
            let created = token_repo
                .create(NewPasswordResetToken {
                    user_id,
                    token_hash,
                    expires_at,
                })
                .await;
            if let Err(e) = created {
                warn!(error = %e, user_id = %user_id, "failed to create password reset token");
                return;
            }
            info!(user_id = %user_id, "password reset token created");
            if let Err(e) = email_sender.send(message).await {
                warn!(error = %e, "failed to send password reset email");
            }
        });

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

        // A link sent before the account enrolled a key must not reset anything: the password
        // no longer signs in, and the reset would end every session.
        let user = self
            .repos
            .user_repo
            .get_by_id(record.user_id)
            .await?
            .ok_or(ServiceError::NotFound)?;
        if user.principal_id.is_some() {
            return Err(ServiceError::PermissionDenied);
        }

        // Validate new password.
        if cmd.new_password.len() < 8 {
            return Err(ServiceError::Validation(
                "password must be at least 8 characters".to_string(),
            ));
        }

        let password_hash = hash_password(&cmd.new_password).map_err(ServiceError::internal)?;

        // Claim the token, set the password, and cut off every other way back into the account
        // (outstanding reset links, existing sessions) in one transaction when a DB is available.
        let scope = match &self.db {
            Some(db) => Some(
                begin_transaction(db)
                    .await
                    .map_err(ServiceError::internal)?,
            ),
            None => None,
        };
        let repos = match &scope {
            Some(s) => AppRepos::with_handle(&s.handle()).for_password_reset(),
            None => self.repos.clone(),
        };

        // Atomic claim: a concurrent redemption of the same token gets `false` here and the
        // transaction rolls back on drop.
        if !repos
            .password_reset_token_repo
            .try_mark_used(record.id)
            .await?
        {
            return Err(ServiceError::PermissionDenied);
        }

        repos
            .user_repo
            .update_password(record.user_id, password_hash)
            .await?;

        let invalidated_tokens = repos
            .password_reset_token_repo
            .invalidate_unused_for_user(record.user_id)
            .await?;
        let revoked_sessions = repos
            .session_repo
            .revoke_all_for_user(record.user_id)
            .await?;

        if let Some(scope) = scope {
            scope.commit().await.map_err(ServiceError::internal)?;
        }

        if let Some(sources) = &self.known_login_sources {
            sources.forget(&user.username);
        }

        info!(
            target = "audit",
            event = "password_reset_completed",
            user_id = %record.user_id,
            revoked_sessions,
            invalidated_tokens,
            "password reset completed"
        );
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rim_core::auth::repository::MockSessionRepository;
    use rim_core::password_reset::model::{PasswordResetToken, PasswordResetTokenId};
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
            principal_id: None,
        }
    }

    fn build_service(
        user_repo: MockUserRepository,
        token_repo: MockPasswordResetTokenRepository,
        email_sender: crate::email::MockEmailSender,
        public_base_url: Option<String>,
    ) -> DefaultPasswordResetService {
        build_service_with_sessions(
            user_repo,
            token_repo,
            MockSessionRepository::new(),
            email_sender,
            public_base_url,
        )
    }

    fn build_service_with_sessions(
        user_repo: MockUserRepository,
        token_repo: MockPasswordResetTokenRepository,
        session_repo: MockSessionRepository,
        email_sender: crate::email::MockEmailSender,
        public_base_url: Option<String>,
    ) -> DefaultPasswordResetService {
        DefaultPasswordResetService::new(
            PasswordResetRepos {
                user_repo: Arc::new(user_repo),
                password_reset_token_repo: Arc::new(token_repo),
                session_repo: Arc::new(session_repo),
            },
            None,
            Arc::new(email_sender),
            public_base_url,
        )
    }

    fn reset_cmd(username: &str) -> RequestPasswordResetCmd {
        RequestPasswordResetCmd {
            username: username.to_string(),
            remote_ip: "1.2.3.4".to_string(),
        }
    }

    fn token_record(user_id: UserId) -> PasswordResetToken {
        PasswordResetToken {
            id: PasswordResetTokenId::new(),
            user_id,
            token_hash: hash_token("rim_rst_valid"),
            expires_at: Utc::now() + Duration::hours(1),
            used_at: None,
            created_at: Utc::now(),
        }
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

        let result = svc.request_reset(reset_cmd("alice")).await;
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

        let result = svc.request_reset(reset_cmd("nobody")).await;
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

        let result = svc.request_reset(reset_cmd("alice")).await;
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
                session_repo: Arc::new(MockSessionRepository::new()),
            },
            None,
            Arc::new(SlowEmailSender),
            None,
        );

        let start = std::time::Instant::now();
        let result = svc.request_reset(reset_cmd("alice")).await;
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

    #[tokio::test]
    async fn request_reset_over_ip_limit_is_rate_limited() {
        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_find_by_username()
            .times(1)
            .returning(|_| Ok(None));

        let svc = build_service(
            user_repo,
            MockPasswordResetTokenRepository::new(),
            crate::email::MockEmailSender::new(),
            None,
        )
        .with_rate_limiters(
            RegistrationRateLimiter::new(3600, 1),
            RegistrationRateLimiter::new(3600, 100),
        );

        assert!(svc.request_reset(reset_cmd("first")).await.is_ok());
        let err = svc.request_reset(reset_cmd("second")).await.unwrap_err();
        assert!(
            matches!(err, ServiceError::RateLimited { .. }),
            "expected RateLimited, got {err:?}"
        );
    }

    #[tokio::test]
    async fn request_reset_over_username_limit_succeeds_without_sending() {
        let user_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        // Only the first request reaches the lookup; the second is suppressed before it.
        user_repo
            .expect_find_by_username()
            .times(1)
            .returning(move |_| Ok(Some(make_user(user_id, "alice", "alice@example.com"))));

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().times(1).returning(|new| {
            Ok(PasswordResetToken {
                id: PasswordResetTokenId::new(),
                user_id: new.user_id,
                token_hash: new.token_hash,
                expires_at: new.expires_at,
                used_at: None,
                created_at: Utc::now(),
            })
        });

        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender.expect_send().times(1).returning(|_| Ok(()));

        let svc = build_service(user_repo, token_repo, email_sender, None).with_rate_limiters(
            RegistrationRateLimiter::new(3600, 100),
            RegistrationRateLimiter::new(3600, 1),
        );

        assert!(svc.request_reset(reset_cmd("alice")).await.is_ok());
        // Case-insensitive: "ALICE" shares alice's bucket.
        assert!(
            svc.request_reset(reset_cmd("ALICE")).await.is_ok(),
            "the per-username limit must not surface an error"
        );

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }

    #[tokio::test]
    async fn reset_password_revokes_sessions_and_other_tokens() {
        let user_id = UserId::new();
        let record = token_record(user_id);
        let record_id = record.id;

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(record.clone())));
        token_repo
            .expect_try_mark_used()
            .withf(move |id| *id == record_id)
            .times(1)
            .returning(|_| Ok(true));
        token_repo
            .expect_invalidate_unused_for_user()
            .withf(move |id| *id == user_id)
            .times(1)
            .returning(|_| Ok(2));

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_update_password()
            .withf(move |id, _| *id == user_id)
            .times(1)
            .returning(|_, _| Ok(()));
        user_repo
            .expect_get_by_id()
            .withf(move |id| *id == user_id)
            .times(1)
            .returning(move |id| Ok(Some(make_user(id, "alice", "alice@example.com"))));

        let mut session_repo = MockSessionRepository::new();
        session_repo
            .expect_revoke_all_for_user()
            .withf(move |id| *id == user_id)
            .times(1)
            .returning(|_| Ok(3));

        let known = KnownLoginSources::login_default();
        let now = Utc::now();
        known.record("6.6.6.6", "alice", now);
        known.record("1.1.1.1", "bob", now);
        let svc = build_service_with_sessions(
            user_repo,
            token_repo,
            session_repo,
            crate::email::MockEmailSender::new(),
            None,
        )
        .with_known_login_sources(known.clone());

        svc.reset_password(ResetPasswordCmd {
            token: "rim_rst_valid".to_string(),
            new_password: "new-password-123".to_string(),
        })
        .await
        .unwrap();

        assert!(
            !known.is_known("6.6.6.6", "alice", now),
            "a reset must forget the account's login sources"
        );
        assert!(known.is_known("1.1.1.1", "bob", now));
    }

    #[tokio::test]
    async fn reset_password_losing_the_token_claim_changes_nothing() {
        let user_id = UserId::new();
        let record = token_record(user_id);

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(record.clone())));
        // A concurrent redemption claimed the token first.
        token_repo.expect_try_mark_used().returning(|_| Ok(false));
        token_repo.expect_invalidate_unused_for_user().never();

        let mut user_repo = MockUserRepository::new();
        user_repo
            .expect_get_by_id()
            .returning(move |id| Ok(Some(make_user(id, "alice", "alice@example.com"))));
        user_repo.expect_update_password().never();

        let mut session_repo = MockSessionRepository::new();
        session_repo.expect_revoke_all_for_user().never();

        let svc = build_service_with_sessions(
            user_repo,
            token_repo,
            session_repo,
            crate::email::MockEmailSender::new(),
            None,
        );

        let err = svc
            .reset_password(ResetPasswordCmd {
                token: "rim_rst_valid".to_string(),
                new_password: "new-password-123".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }

    #[tokio::test]
    async fn request_reset_for_a_key_account_sends_nothing() {
        let user_id = UserId::new();
        let mut user_repo = MockUserRepository::new();
        user_repo.expect_find_by_username().returning(move |_| {
            Ok(Some(User {
                principal_id: Some("rimp1_key".to_string()),
                ..make_user(user_id, "alice", "alice@example.com")
            }))
        });
        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo.expect_create().never();
        let mut email_sender = crate::email::MockEmailSender::new();
        email_sender.expect_send().never();

        let svc = build_service(user_repo, token_repo, email_sender, None);
        assert!(svc.request_reset(reset_cmd("alice")).await.is_ok());
    }

    #[tokio::test]
    async fn reset_password_for_a_key_account_is_refused() {
        let user_id = UserId::new();
        let record = token_record(user_id);

        let mut token_repo = MockPasswordResetTokenRepository::new();
        token_repo
            .expect_get_by_token_hash()
            .returning(move |_| Ok(Some(record.clone())));
        token_repo.expect_try_mark_used().never();

        let mut user_repo = MockUserRepository::new();
        user_repo.expect_get_by_id().returning(move |id| {
            Ok(Some(User {
                principal_id: Some("rimp1_key".to_string()),
                ..make_user(id, "alice", "alice@example.com")
            }))
        });
        user_repo.expect_update_password().never();

        let mut session_repo = MockSessionRepository::new();
        session_repo.expect_revoke_all_for_user().never();

        let svc = build_service_with_sessions(
            user_repo,
            token_repo,
            session_repo,
            crate::email::MockEmailSender::new(),
            None,
        );
        let err = svc
            .reset_password(ResetPasswordCmd {
                token: "rim_rst_valid".to_string(),
                new_password: "new-password-123".to_string(),
            })
            .await
            .unwrap_err();
        assert!(matches!(err, ServiceError::PermissionDenied));
    }
}
