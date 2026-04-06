use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::password_reset::model::{
    NewPasswordResetToken, PasswordResetToken, PasswordResetTokenId,
};
use crate::repository::error::RepoResult;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait PasswordResetTokenRepository: Send + Sync {
    async fn create(&self, token: NewPasswordResetToken) -> RepoResult<PasswordResetToken>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PasswordResetToken>>;
    async fn mark_used(&self, id: PasswordResetTokenId) -> RepoResult<()>;
}
