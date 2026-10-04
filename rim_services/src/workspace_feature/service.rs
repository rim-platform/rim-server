use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::server::model::ServerId;
use rim_core::workspace_feature::model::WorkspaceFeature;

use crate::ServiceResult;
use crate::workspace_feature::command::{ListWorkspaceFeaturesQuery, SetWorkspaceFeatureCmd};
use crate::workspace_feature::dto::WorkspaceFeatureStateDto;

/// Per-workspace feature flags (Track 0.4). See
/// `docs/specs/track-0-4-optional-incident-module.md`.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspaceFeatureService: Send + Sync {
    /// Any member. Returns one entry per known feature ([`WorkspaceFeature::ALL`]), with
    /// defaults filled in for features that have no persisted row.
    ///
    /// # Errors
    /// [`crate::ServiceError::NotFound`] when the actor is not a member of the server.
    async fn list(
        &self,
        query: ListWorkspaceFeaturesQuery,
    ) -> ServiceResult<Vec<WorkspaceFeatureStateDto>>;

    /// Owner only. Upserts the flag and returns the new state.
    ///
    /// # Errors
    /// [`crate::ServiceError::NotFound`] for a non-member,
    /// [`crate::ServiceError::PermissionDenied`] for a member who is not the owner.
    async fn set(&self, cmd: SetWorkspaceFeatureCmd) -> ServiceResult<WorkspaceFeatureStateDto>;

    /// Internal gate: no actor, callers are already authorized. Applies the feature default
    /// when no row exists.
    async fn is_enabled(
        &self,
        server_id: ServerId,
        feature: WorkspaceFeature,
    ) -> ServiceResult<bool>;

    /// Internal gate: the features currently enabled for `server_id`, resolved with defaults,
    /// in one repository call. Used by command listing to filter many commands at once.
    async fn enabled_features(&self, server_id: ServerId) -> ServiceResult<Vec<WorkspaceFeature>>;
}

#[cfg(any(test, feature = "mocks"))]
impl MockWorkspaceFeatureService {
    /// A mock with every feature enabled for every workspace, for tests of the command
    /// service that are not about the feature gate.
    pub fn all_enabled() -> Self {
        let mut mock = Self::new();
        mock.expect_is_enabled()
            .returning(|_, _| Box::pin(async { Ok(true) }));
        mock.expect_enabled_features()
            .returning(|_| Box::pin(async { Ok(WorkspaceFeature::ALL.to_vec()) }));
        mock
    }

    /// A mock with every feature disabled for every workspace.
    pub fn all_disabled() -> Self {
        let mut mock = Self::new();
        mock.expect_is_enabled()
            .returning(|_, _| Box::pin(async { Ok(false) }));
        mock.expect_enabled_features()
            .returning(|_| Box::pin(async { Ok(Vec::new()) }));
        mock
    }
}
