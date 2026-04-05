use async_trait::async_trait;
#[cfg(any(test, feature = "mocks"))]
use mockall::automock;

use rim_core::server::model::ServerId;
use rim_core::storage::StoredFile;

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

    async fn download_file(&self, key: &str) -> ServiceResult<Vec<u8>>;

    async fn delete_file(&self, key: &str) -> ServiceResult<()>;

    async fn get_quota_usage(&self, workspace_id: ServerId) -> ServiceResult<QuotaUsage>;
}
