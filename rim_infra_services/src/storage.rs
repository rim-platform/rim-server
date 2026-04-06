use std::sync::Arc;

use async_trait::async_trait;

use rim_core::server::model::ServerId;
use rim_core::storage::{StorageBackend, StorageError, StoredFile};
use rim_services::storage::service::{QuotaUsage, StorageService};
use rim_services::{ServiceError, ServiceResult};

pub struct DefaultStorageService {
    backend: Arc<dyn StorageBackend>,
    max_workspace_storage_bytes: u64,
}

impl DefaultStorageService {
    pub fn new(backend: Arc<dyn StorageBackend>, max_workspace_storage_bytes: u64) -> Self {
        Self {
            backend,
            max_workspace_storage_bytes,
        }
    }

    /// Build a storage key for an attachment file.
    /// Pattern: `{workspace_id}/attachments/{attachment_id}/{filename}`
    pub fn attachment_key(workspace_id: &ServerId, attachment_id: &str, filename: &str) -> String {
        format!(
            "{}/attachments/{}/{}",
            workspace_id, attachment_id, filename
        )
    }

    /// Build a storage key for an avatar file.
    /// Pattern: `{workspace_id}/avatars/{user_id}/{filename}`
    pub fn avatar_key(workspace_id: &str, user_id: &str, filename: &str) -> String {
        format!("{}/avatars/{}/{}", workspace_id, user_id, filename)
    }

    pub fn backend(&self) -> &dyn StorageBackend {
        self.backend.as_ref()
    }
}

fn map_storage_error(err: StorageError) -> ServiceError {
    tracing::error!(storage_error = %err, "storage operation failed");
    match err {
        StorageError::NotFound(ref path) => {
            tracing::error!(path = %path, "storage not found");
            ServiceError::NotFound
        }
        StorageError::Io(e) => ServiceError::internal(e),
        StorageError::TooLarge { size, max } => ServiceError::Validation(format!(
            "file too large: {} bytes (max {} bytes)",
            size, max
        )),
        StorageError::QuotaExceeded { used, max } => ServiceError::Validation(format!(
            "storage quota exceeded: {} of {} bytes used",
            used, max
        )),
        StorageError::Unsupported(msg) => {
            ServiceError::internal(std::io::Error::new(std::io::ErrorKind::Unsupported, msg))
        }
    }
}

#[async_trait]
impl StorageService for DefaultStorageService {
    async fn upload_file(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> ServiceResult<StoredFile> {
        tracing::info!(key = %key, size = data.len(), content_type = %content_type, "uploading file");

        let stored = self
            .backend
            .put(key, data, content_type)
            .await
            .map_err(map_storage_error)?;

        tracing::info!(key = %stored.key, size = stored.size_bytes, "file uploaded successfully");
        Ok(stored)
    }

    async fn download_file(&self, key: &str) -> ServiceResult<Vec<u8>> {
        self.backend.get(key).await.map_err(map_storage_error)
    }

    async fn delete_file(&self, key: &str) -> ServiceResult<()> {
        tracing::info!(key = %key, "deleting file");
        self.backend.delete(key).await.map_err(map_storage_error)
    }

    async fn get_quota_usage(&self, _workspace_id: ServerId) -> ServiceResult<QuotaUsage> {
        // Quota tracking is not yet backed by a persistent store.
        // Return the configured limit with zero usage for now.
        Ok(QuotaUsage {
            used_bytes: 0,
            max_bytes: Some(self.max_workspace_storage_bytes),
            file_count: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_storage::LocalFileStorage;
    use tempfile::TempDir;

    #[tokio::test]
    async fn upload_and_download() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = DefaultStorageService::new(backend, 1_073_741_824);

        let stored = svc
            .upload_file("test/hello.txt", b"hello world", "text/plain")
            .await
            .unwrap();
        assert_eq!(stored.key, "test/hello.txt");
        assert_eq!(stored.size_bytes, 11);

        let data = svc.download_file("test/hello.txt").await.unwrap();
        assert_eq!(data, b"hello world");
    }

    #[tokio::test]
    async fn delete_file_works() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = DefaultStorageService::new(backend, 1_073_741_824);

        svc.upload_file("to_delete.txt", b"bye", "text/plain")
            .await
            .unwrap();
        svc.delete_file("to_delete.txt").await.unwrap();

        let err = svc.download_file("to_delete.txt").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn quota_usage_returns_defaults() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = DefaultStorageService::new(backend, 1_073_741_824);

        let usage = svc.get_quota_usage(ServerId::new()).await.unwrap();
        assert_eq!(usage.used_bytes, 0);
        assert_eq!(usage.max_bytes, Some(1_073_741_824));
        assert_eq!(usage.file_count, 0);
    }
}
