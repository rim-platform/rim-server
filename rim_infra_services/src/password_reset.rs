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

use crate::email::EmailSender;

#[derive(Clone)]
pub struct PasswordResetRepos {
    pub user_repo: Arc<dyn UserRepository>,
    pub password_reset_token_repo: Arc<dyn PasswordResetTokenRepository>,
}

pub struct DefaultPasswordResetService {
    repos: PasswordResetRepos,
    email_sender: Arc<dyn EmailSender>,
}

impl DefaultPasswordResetService {
    pub fn new(repos: PasswordResetRepos, email_sender: Arc<dyn EmailSender>) -> Self {
        Self {
            repos,
            email_sender,
        }
    }

    fn generate_reset_token() -> String {
        let t = generate_session_token();
        t.strip_prefix("rim_sess_")
            .map(|s| format!("rim_rst_{s}"))
            .unwrap_or_else(|| format!("rim_rst_{t}"))
    }
}

#[async_trait]
impl PasswordResetService for DefaultPasswordResetService {
    #[instrument(name = "password_reset.request", skip_all, fields(username = %cmd.username))]
    async fn request_reset(&self, cmd: RequestPasswordResetCmd) -> ServiceResult<()> {
        let user = self
            .repos
            .user_repo
            .find_by_username(&cmd.username)
            .await
            .map_err(rim_services::map_repo_error)?;

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
            .await
            .map_err(rim_services::map_repo_error)?;

        if let Err(e) = self
            .email_sender
            .send_password_reset(&cmd.username, &raw_token)
            .await
        {
            warn!(error = %e, "failed to send password reset email");
        }

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
            .await
            .map_err(rim_services::map_repo_error)?
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
            .await
            .map_err(rim_services::map_repo_error)?;

        self.repos
            .password_reset_token_repo
            .mark_used(record.id)
            .await
            .map_err(rim_services::map_repo_error)?;

        info!(user_id = %record.user_id, "password reset completed");
        Ok(())
    }
}
