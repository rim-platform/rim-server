use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::password_reset::command::{RequestPasswordResetCmd, ResetPasswordCmd};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PasswordResetService: Send + Sync {
    async fn request_reset(&self, cmd: RequestPasswordResetCmd) -> ServiceResult<()>;

    async fn reset_password(&self, cmd: ResetPasswordCmd) -> ServiceResult<()>;
}
