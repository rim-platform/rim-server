use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::user::model::UserId;
use crate::workspace_feature::model::{WorkspaceFeature, WorkspaceFeatureState};

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceFeatureRepository: Send + Sync {
    /// Every persisted row for the server. Features without a row are absent (the caller
    /// applies [`WorkspaceFeature::default_enabled`]); rows with an unknown feature string are
    /// skipped.
    async fn list_for_server(&self, server_id: ServerId) -> RepoResult<Vec<WorkspaceFeatureState>>;

    /// Upsert the row for `(server_id, feature)` and return it.
    async fn set(
        &self,
        server_id: ServerId,
        feature: WorkspaceFeature,
        enabled: bool,
        updated_by: Option<UserId>,
    ) -> RepoResult<WorkspaceFeatureState>;

    /// The persisted value, or [`WorkspaceFeature::default_enabled`] when no row exists.
    async fn is_enabled(&self, server_id: ServerId, feature: WorkspaceFeature) -> RepoResult<bool>;
}

#[cfg(any(test, feature = "mocks"))]
impl MockWorkspaceFeatureRepository {
    /// A mock that reports every feature enabled, for tests of gated services that are not
    /// about the gate itself.
    pub fn all_enabled() -> Self {
        let mut mock = Self::new();
        mock.expect_is_enabled()
            .returning(|_, _| Box::pin(async { Ok(true) }));
        mock
    }

    /// A mock that reports every feature disabled.
    pub fn all_disabled() -> Self {
        let mut mock = Self::new();
        mock.expect_is_enabled()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        mock
    }
}
