use async_trait::async_trait;
use chrono::Duration;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use crate::repository::error::RepoResult;
use crate::server::model::ServerId;
use crate::workspace_plugin::model::{
    NewPluginBundle, PluginBundle, WorkspacePlugin, WorkspacePluginId, WorkspacePluginWrite,
};

/// Workspace plugin registries and the bundles uploaded to them.
///
/// Adds and updates call [`WorkspacePluginRepository::lock_server`] first, inside one
/// transaction, so the entry cap holds when two adds race.
#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait WorkspacePluginRepository: Send + Sync {
    /// Lock the server's row until the surrounding transaction ends (`SELECT ... FOR UPDATE`).
    /// Returns `false` when the server does not exist.
    async fn lock_server(&self, server_id: ServerId) -> RepoResult<bool>;

    /// The server's entries, ordered by plugin id.
    async fn list(&self, server_id: ServerId) -> RepoResult<Vec<WorkspacePlugin>>;

    async fn get(
        &self,
        server_id: ServerId,
        plugin_id: &str,
    ) -> RepoResult<Option<WorkspacePlugin>>;

    /// Insert an entry unless the server already has `max_entries`; `None` when it has.
    ///
    /// # Errors
    /// `Conflict` when the server already has an entry for this plugin id.
    async fn insert(
        &self,
        entry: WorkspacePluginWrite,
        max_entries: u64,
    ) -> RepoResult<Option<WorkspacePlugin>>;

    /// Replace every written column of an entry; `None` when it does not exist.
    async fn update(
        &self,
        id: WorkspacePluginId,
        entry: WorkspacePluginWrite,
    ) -> RepoResult<Option<WorkspacePlugin>>;

    /// Delete an entry. Returns `false` when there was none.
    async fn delete(&self, server_id: ServerId, plugin_id: &str) -> RepoResult<bool>;

    async fn get_bundle(
        &self,
        server_id: ServerId,
        sha256: &str,
    ) -> RepoResult<Option<PluginBundle>>;

    /// Record an uploaded bundle; `None` when the server already has one with this hash.
    async fn insert_bundle(&self, bundle: NewPluginBundle) -> RepoResult<Option<PluginBundle>>;

    /// Total size in bytes of the server's bundles.
    async fn bundle_bytes(&self, server_id: ServerId) -> RepoResult<u64>;

    /// Delete the server's bundles that no entry names and that were uploaded more than `age`
    /// ago. Returns their storage keys.
    async fn delete_unnamed_bundles(
        &self,
        server_id: ServerId,
        age: Duration,
    ) -> RepoResult<Vec<String>>;
}
