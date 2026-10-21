use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::server::model::ServerId;
use rim_core::storage::StoredFile;
use rim_core::user::model::UserId;

use crate::ServiceResult;

#[derive(Debug, Clone)]
pub struct QuotaUsage {
    pub used_bytes: u64,
    pub max_bytes: Option<u64>,
    pub file_count: u64,
}

#[async_trait]
#[cfg_attr(any(test, feature = "mocks"), automock)]
pub trait StorageService: Send + Sync {
    async fn upload_file(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> ServiceResult<StoredFile>;

    async fn delete_file(&self, key: &str) -> ServiceResult<()>;

    /// The bytes stored under `key`; `NotFound` when there are none.
    async fn download_file(&self, key: &str) -> ServiceResult<Vec<u8>>;

    async fn get_quota_usage(&self, workspace_id: ServerId) -> ServiceResult<QuotaUsage>;

    /// The keys of the files stored directly in the directory `dir`, sorted; empty when it
    /// holds none.
    async fn list_files(&self, dir: &str) -> ServiceResult<Vec<String>>;

    /// Delete every file in `user_id`'s avatar directory except the one stored under `keep`,
    /// and return the keys deleted. A file already gone is skipped.
    ///
    /// # Errors
    ///
    /// The storage error of the listing or of the first file that could not be deleted; the
    /// files before it are deleted.
    async fn delete_avatar_files(
        &self,
        user_id: UserId,
        keep: Option<String>,
    ) -> ServiceResult<Vec<String>>;
}
