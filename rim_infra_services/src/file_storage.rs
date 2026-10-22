use std::path::PathBuf;

use async_trait::async_trait;

use rim_core::storage::{StorageBackend, StorageError, StoredFile};

pub struct LocalFileStorage {
    root_dir: PathBuf,
}

impl LocalFileStorage {
    pub fn new(root_dir: PathBuf) -> Self {
        Self { root_dir }
    }

    /// Validate that the resolved path is inside root_dir to prevent path traversal.
    fn validate_path(&self, path: &str) -> Result<PathBuf, StorageError> {
        // Reject obvious traversal components before touching the filesystem.
        if path.contains('\0') || path.contains("..") {
            tracing::warn!(path = %path, "rejected: contains null or ..");
            return Err(StorageError::NotFound(path.to_string()));
        }

        let full_path = self.root_dir.join(path);
        tracing::debug!(root_dir = %self.root_dir.display(), full_path = %full_path.display(), exists = full_path.exists(), "validate_path");

        // Canonicalize root_dir for comparison.
        let canonical_root = self.root_dir.canonicalize().map_err(StorageError::Io)?;

        // For existing paths, canonicalize and verify containment.
        // For new paths (store), verify by normalizing without symlink resolution.
        let canonical = if full_path.exists() {
            full_path.canonicalize().map_err(StorageError::Io)?
        } else {
            // Path doesn't exist yet — this is a store operation.
            // ".." and null bytes are already rejected above.
            // Verify the joined path starts with canonical root by canonicalizing
            // the deepest existing ancestor.
            // Walk up to find the deepest existing ancestor, canonicalize it,
            // then re-append the remaining path components to get an absolute path.
            let mut ancestor = full_path.clone();
            let mut stripped_parts = Vec::new();
            loop {
                if ancestor.exists() {
                    let canonical_ancestor = ancestor.canonicalize().map_err(StorageError::Io)?;
                    if !canonical_ancestor.starts_with(&canonical_root) {
                        return Err(StorageError::NotFound(path.to_string()));
                    }
                    // Re-build the full canonical path by appending stripped parts
                    let mut result = canonical_ancestor;
                    for part in stripped_parts.into_iter().rev() {
                        result = result.join(part);
                    }
                    break result;
                }
                if let Some(file_name) = ancestor.file_name() {
                    stripped_parts.push(file_name.to_os_string());
                }
                if !ancestor.pop() {
                    return Err(StorageError::NotFound(path.to_string()));
                }
            }
        };

        if !canonical.starts_with(&canonical_root) {
            return Err(StorageError::NotFound(path.to_string()));
        }

        Ok(full_path)
    }
}

#[async_trait]
impl StorageBackend for LocalFileStorage {
    async fn put(
        &self,
        key: &str,
        data: &[u8],
        _content_type: &str,
    ) -> Result<StoredFile, StorageError> {
        let full_path = self.validate_path(key)?;

        if let Some(parent) = full_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        tokio::fs::write(&full_path, data).await?;

        Ok(StoredFile {
            key: key.to_string(),
            size_bytes: data.len() as u64,
        })
    }

    async fn get(&self, key: &str) -> Result<Vec<u8>, StorageError> {
        let full_path = self.validate_path(key)?;

        if !full_path.exists() {
            return Err(StorageError::NotFound(key.to_string()));
        }

        let data = tokio::fs::read(&full_path).await?;
        Ok(data)
    }

    async fn delete(&self, key: &str) -> Result<(), StorageError> {
        let full_path = self.validate_path(key)?;

        if !full_path.exists() {
            return Err(StorageError::NotFound(key.to_string()));
        }

        tokio::fs::remove_file(&full_path).await?;
        Ok(())
    }

    async fn list(&self, dir: &str) -> Result<Vec<String>, StorageError> {
        let full_path = self.validate_path(dir)?;

        if !full_path.is_dir() {
            return Ok(Vec::new());
        }

        let mut keys = Vec::new();
        let mut entries = tokio::fs::read_dir(&full_path).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await?.is_file() {
                continue;
            }
            // A name that is not UTF-8 cannot have been stored under a key.
            if let Some(name) = entry.file_name().to_str() {
                keys.push(format!("{dir}/{name}"));
            }
        }
        keys.sort();
        Ok(keys)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[tokio::test]
    async fn store_and_retrieve() {
        let tmp = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(tmp.path().to_path_buf());

        let stored = storage
            .put("test/hello.txt", b"hello world", "text/plain")
            .await
            .unwrap();
        assert_eq!(stored.key, "test/hello.txt");
        assert_eq!(stored.size_bytes, 11);

        let data = storage.get("test/hello.txt").await.unwrap();
        assert_eq!(data, b"hello world");
    }

    #[tokio::test]
    async fn delete_file() {
        let tmp = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(tmp.path().to_path_buf());

        storage
            .put("to_delete.txt", b"bye", "text/plain")
            .await
            .unwrap();
        storage.delete("to_delete.txt").await.unwrap();

        let err = storage.get("to_delete.txt").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn path_traversal_rejected() {
        let tmp = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(tmp.path().to_path_buf());

        let err = storage.put("../escape.txt", b"bad", "text/plain").await;
        assert!(err.is_err());

        let err = storage.get("../../etc/passwd").await;
        assert!(err.is_err());
    }

    #[tokio::test]
    async fn list_names_the_files_directly_in_a_directory() {
        let tmp = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(tmp.path().to_path_buf());
        for key in ["a/x.png", "a/b.gif", "a/sub/c.txt", "ab/d.txt"] {
            storage.put(key, b"x", "text/plain").await.unwrap();
        }

        assert_eq!(storage.list("a").await.unwrap(), vec!["a/b.gif", "a/x.png"]);
        assert_eq!(storage.list("a/sub").await.unwrap(), vec!["a/sub/c.txt"]);
        assert!(storage.list("missing").await.unwrap().is_empty());
        assert!(storage.list("a/x.png").await.unwrap().is_empty());
        assert!(storage.list("../escape").await.is_err());
    }

    #[tokio::test]
    async fn retrieve_not_found() {
        let tmp = TempDir::new().unwrap();
        let storage = LocalFileStorage::new(tmp.path().to_path_buf());

        let err = storage.get("nonexistent.txt").await;
        assert!(err.is_err());
    }
}
