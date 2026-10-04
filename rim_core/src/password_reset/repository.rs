use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::password_reset::model::{
    NewPasswordResetToken, PasswordResetToken, PasswordResetTokenId,
};
use crate::repository::error::RepoResult;
use crate::user::model::UserId;

#[cfg_attr(any(test, feature = "mocks"), automock)]
#[async_trait]
pub trait PasswordResetTokenRepository: Send + Sync {
    async fn create(&self, token: NewPasswordResetToken) -> RepoResult<PasswordResetToken>;
    async fn get_by_token_hash(&self, token_hash: &str) -> RepoResult<Option<PasswordResetToken>>;
    /// Atomically marks the token used if it is still unused and unexpired.
    ///
    /// Returns `true` if this call claimed the token, `false` if it was already used, expired
    /// or missing — so two concurrent redemptions of one token can't both succeed.
    async fn try_mark_used(&self, id: PasswordResetTokenId) -> RepoResult<bool>;
    /// Marks every still-unused token for `user_id` as used. Returns how many were marked.
    async fn invalidate_unused_for_user(&self, user_id: UserId) -> RepoResult<u64>;
}
