//! The server's default workspace: the workspace every account created here joins as a Member.

use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;
use rim_core::server::model::ServerId;

use crate::ServiceResult;

/// Which workspace new accounts join now.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait DefaultWorkspaceService: Send + Sync {
    /// The default workspace a signup joins at this moment: the configured one, or the one an
    /// owner claimed from the definition file; `None` when there is none yet.
    ///
    /// # Errors
    /// Storage errors reading the claimed workspace.
    async fn current(&self) -> ServiceResult<Option<ServerId>>;
}
