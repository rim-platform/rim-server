use async_trait::async_trait;

#[derive(Debug, Clone)]
pub struct StoredFile {
    /// Storage key (relative path or object key).
    pub key: String,
    pub size_bytes: u64,
}

#[async_trait]
pub trait StorageBackend: Send + Sync {
    /// Store data at the given key. Returns metadata about the stored file.
    async fn put(
        &self,
        key: &str,
        data: &[u8],
        content_type: &str,
    ) -> Result<StoredFile, StorageError>;

    /// Retrieve file contents by key.
    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError>;

    /// Delete a file by key.
    async fn delete(&self, key: &str) -> Result<(), StorageError>;
}

#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    #[error("File not found: {0}")]
    NotFound(String),
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("File too large: {size} bytes (max {max} bytes)")]
    TooLarge { size: u64, max: u64 },
    #[error("Quota exceeded: used {used} of {max} bytes")]
    QuotaExceeded { used: u64, max: u64 },
    #[error("Backend not supported: {0}")]
    Unsupported(String),
}
