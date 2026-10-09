use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::ServiceResult;
use crate::workspace_plugin::command::{
    AddWorkspacePluginCmd, DownloadPluginBundleQuery, ListWorkspacePluginsQuery,
    RemoveWorkspacePluginCmd, UpdateWorkspacePluginCmd, UploadPluginBundleCmd,
};
use crate::workspace_plugin::dto::{UploadedPluginBundleDto, WorkspacePluginDto};

/// A workspace's plugin registry. Members and guests read it; Owners and Moderators add,
/// update and remove entries. Every statement is verified against its publisher's key log
/// before it is stored, and each change is announced with `WorkspacePluginChanged`.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspacePluginService: Send + Sync {
    /// The workspace's entries, ordered by plugin id.
    ///
    /// # Errors
    /// `PermissionDenied` for a non-member.
    async fn list(
        &self,
        query: ListWorkspacePluginsQuery,
    ) -> ServiceResult<Vec<WorkspacePluginDto>>;

    /// Add an entry for the statement's plugin id.
    ///
    /// # Errors
    /// - `PermissionDenied` for a non-member, a Member or a Guest.
    /// - `Validation` for a statement that does not verify, a missing or broken publisher key
    ///   log, a bundle not uploaded here, or a bad bundle URL.
    /// - `Conflict` when the plugin id is already in the registry, or the registry is full.
    /// - `RateLimited` over the per-user limit.
    async fn add(&self, cmd: AddWorkspacePluginCmd) -> ServiceResult<WorkspacePluginDto>;

    /// Change an entry's statement, status or bundle URL.
    ///
    /// # Errors
    /// As for [`WorkspacePluginService::add`], plus `NotFound` for an unknown entry and
    /// `Validation` for a statement naming another plugin id or a call changing nothing.
    async fn update(&self, cmd: UpdateWorkspacePluginCmd) -> ServiceResult<WorkspacePluginDto>;

    /// Remove an entry. Its bundle goes with the next cleanup.
    ///
    /// # Errors
    /// `PermissionDenied` as for [`WorkspacePluginService::add`]; `NotFound` for an unknown
    /// entry; `RateLimited`.
    async fn remove(&self, cmd: RemoveWorkspacePluginCmd) -> ServiceResult<()>;
}

/// The web plugin bundles a workspace keeps for its registry, behind the bundle endpoints.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait PluginBundleService: Send + Sync {
    /// Check and store a bundle, then delete, best effort, the workspace's bundles no entry
    /// names that are over a day old. Storing a hash the workspace has is not an error.
    ///
    /// # Errors
    /// - `PermissionDenied` for a non-member, a Member or a Guest.
    /// - `Validation` naming why the bytes are not a bundle, or when they would pass the
    ///   workspace's storage quota.
    /// - `RateLimited` over the per-user limit.
    async fn upload(&self, cmd: UploadPluginBundleCmd) -> ServiceResult<UploadedPluginBundleDto>;

    /// The bytes of one of the workspace's bundles.
    ///
    /// # Errors
    /// `NotFound` for a non-member and for a hash the workspace does not have.
    async fn download(&self, query: DownloadPluginBundleQuery) -> ServiceResult<Vec<u8>>;
}
