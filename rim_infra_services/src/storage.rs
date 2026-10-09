use std::sync::Arc;

use async_trait::async_trait;

use rim_core::message_attachment::repository::MessageAttachmentRepository;
use rim_core::server::model::ServerId;
use rim_core::storage::{StorageBackend, StorageError, StoredFile};
use rim_core::workspace_plugin::repository::WorkspacePluginRepository;
use rim_services::message_attachment::{MAX_ATTACHMENT_BYTES, UPLOAD_TICKET_TTL_MINUTES};
use rim_services::storage::service::{QuotaUsage, StorageService};
use rim_services::{ServiceError, ServiceResult};

pub struct DefaultStorageService {
    backend: Arc<dyn StorageBackend>,
    /// Sources of per-workspace usage: attachments and plugin bundles are the per-workspace
    /// uploads large enough to need a quota.
    attachment_repo: Arc<dyn MessageAttachmentRepository>,
    workspace_plugin_repo: Arc<dyn WorkspacePluginRepository>,
    max_workspace_storage_bytes: u64,
}

impl DefaultStorageService {
    pub fn new(
        backend: Arc<dyn StorageBackend>,
        attachment_repo: Arc<dyn MessageAttachmentRepository>,
        workspace_plugin_repo: Arc<dyn WorkspacePluginRepository>,
        max_workspace_storage_bytes: u64,
    ) -> Self {
        Self {
            backend,
            attachment_repo,
            workspace_plugin_repo,
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

    async fn delete_file(&self, key: &str) -> ServiceResult<()> {
        tracing::info!(key = %key, "deleting file");
        self.backend.delete(key).await.map_err(map_storage_error)
    }

    async fn download_file(&self, key: &str) -> ServiceResult<Vec<u8>> {
        self.backend.get(key).await.map_err(map_storage_error)
    }

    async fn get_quota_usage(&self, workspace_id: ServerId) -> ServiceResult<QuotaUsage> {
        let pending_since =
            chrono::Utc::now() - chrono::Duration::minutes(UPLOAD_TICKET_TTL_MINUTES);
        let usage = self
            .attachment_repo
            .usage_for_server(
                workspace_id,
                pending_since,
                i64::try_from(MAX_ATTACHMENT_BYTES).unwrap_or(i64::MAX),
            )
            .await?;
        let bundle_bytes = self
            .workspace_plugin_repo
            .bundle_bytes(workspace_id)
            .await?;
        Ok(QuotaUsage {
            used_bytes: usage.total_bytes.saturating_add(bundle_bytes),
            max_bytes: Some(self.max_workspace_storage_bytes),
            file_count: usage.count,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::file_storage::LocalFileStorage;
    use rim_core::message_attachment::model::AttachmentUsage;
    use rim_core::message_attachment::repository::MockMessageAttachmentRepository;
    use rim_core::workspace_plugin::repository::MockWorkspacePluginRepository;
    use tempfile::TempDir;

    fn service_with_bundles(
        backend: Arc<dyn StorageBackend>,
        usage: AttachmentUsage,
        bundle_bytes: u64,
    ) -> DefaultStorageService {
        let mut repo = MockMessageAttachmentRepository::new();
        repo.expect_usage_for_server()
            .returning(move |_, _, _| Box::pin(async move { Ok(usage) }));
        let mut plugins = MockWorkspacePluginRepository::new();
        plugins
            .expect_bundle_bytes()
            .returning(move |_| Box::pin(async move { Ok(bundle_bytes) }));
        DefaultStorageService::new(backend, Arc::new(repo), Arc::new(plugins), 1_073_741_824)
    }

    fn service(backend: Arc<dyn StorageBackend>, usage: AttachmentUsage) -> DefaultStorageService {
        service_with_bundles(backend, usage, 0)
    }

    #[tokio::test]
    async fn upload_and_download() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = service(Arc::clone(&backend), AttachmentUsage::default());

        let stored = svc
            .upload_file("test/hello.txt", b"hello world", "text/plain")
            .await
            .unwrap();
        assert_eq!(stored.key, "test/hello.txt");
        assert_eq!(stored.size_bytes, 11);

        let data = backend.get("test/hello.txt").await.unwrap();
        assert_eq!(data, b"hello world");
        assert_eq!(svc.download_file("test/hello.txt").await.unwrap(), data);
        assert!(matches!(
            svc.download_file("test/missing.txt").await,
            Err(ServiceError::NotFound)
        ));
    }

    #[tokio::test]
    async fn delete_file_works() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = service(Arc::clone(&backend), AttachmentUsage::default());

        svc.upload_file("to_delete.txt", b"bye", "text/plain")
            .await
            .unwrap();
        svc.delete_file("to_delete.txt").await.unwrap();

        let err = backend.get("to_delete.txt").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn quota_usage_reports_attachment_usage() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = service(
            backend,
            AttachmentUsage {
                total_bytes: 4096,
                count: 3,
            },
        );

        let usage = svc.get_quota_usage(ServerId::new()).await.unwrap();
        assert_eq!(usage.used_bytes, 4096);
        assert_eq!(usage.max_bytes, Some(1_073_741_824));
        assert_eq!(usage.file_count, 3);
    }

    #[tokio::test]
    async fn quota_usage_counts_plugin_bundles() {
        let tmp = TempDir::new().unwrap();
        let backend: Arc<dyn StorageBackend> =
            Arc::new(LocalFileStorage::new(tmp.path().to_path_buf()));
        let svc = service_with_bundles(
            backend,
            AttachmentUsage {
                total_bytes: 4096,
                count: 3,
            },
            1605,
        );

        let usage = svc.get_quota_usage(ServerId::new()).await.unwrap();
        assert_eq!(usage.used_bytes, 4096 + 1605);
    }
}
