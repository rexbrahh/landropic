use std::path::{Path, PathBuf};
use tokio::fs;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use bytes::Bytes;
use blake3::Hasher;
use tempfile::NamedTempFile;
use tracing::{debug, trace};

use landro_chunker::ContentHash;
use crate::errors::{CasError, Result};

/// Reference to an object in the content store
#[derive(Debug, Clone, PartialEq)]
pub struct ObjectRef {
    pub hash: ContentHash,
    pub size: u64,
}

/// Content-addressed storage
pub struct ContentStore {
    root_path: PathBuf,
}

impl ContentStore {
    /// Create a new content store at the given path
    pub async fn new(root_path: impl AsRef<Path>) -> Result<Self> {
        let root_path = root_path.as_ref().to_path_buf();
        
        // Ensure root directory exists
        fs::create_dir_all(&root_path).await?;
        
        // Create objects directory
        let objects_dir = root_path.join("objects");
        fs::create_dir_all(&objects_dir).await?;
        
        debug!("Content store initialized at {:?}", root_path);
        
        Ok(Self { root_path })
    }
    
    /// Get the path for an object with sharding
    fn object_path(&self, hash: &ContentHash) -> PathBuf {
        let hex = hash.to_hex();
        let (shard1, rest) = hex.split_at(2);
        let (shard2, filename) = rest.split_at(2);
        
        self.root_path
            .join("objects")
            .join(shard1)
            .join(shard2)
            .join(filename)
    }
    
    /// Write an object to the store
    pub async fn write(&self, data: &[u8]) -> Result<ObjectRef> {
        // Calculate hash
        let mut hasher = Hasher::new();
        hasher.update(data);
        let hash = ContentHash::from_blake3(hasher.finalize());
        
        let object_path = self.object_path(&hash);
        
        // Check if object already exists
        if object_path.exists() {
            trace!("Object {} already exists", hash);
            return Ok(ObjectRef {
                hash,
                size: data.len() as u64,
            });
        }
        
        // Ensure shard directories exist
        if let Some(parent) = object_path.parent() {
            fs::create_dir_all(parent).await?;
        }
        
        // Write atomically using temp file
        let temp_dir = object_path.parent()
            .ok_or_else(|| CasError::StoragePath("Invalid object path".to_string()))?;
        
        let temp_file = NamedTempFile::new_in(temp_dir)
            .map_err(|e| CasError::AtomicWriteFailed(e.to_string()))?;
        
        let temp_path = temp_file.path().to_path_buf();
        
        // Write data to temp file
        let mut file = fs::File::create(&temp_path).await?;
        file.write_all(data).await?;
        file.sync_all().await?;
        
        // Atomic rename
        fs::rename(&temp_path, &object_path).await
            .map_err(|e| CasError::AtomicWriteFailed(e.to_string()))?;
        
        debug!("Wrote object {} ({} bytes)", hash, data.len());
        
        Ok(ObjectRef {
            hash,
            size: data.len() as u64,
        })
    }
    
    /// Read an object from the store
    pub async fn read(&self, hash: &ContentHash) -> Result<Bytes> {
        let object_path = self.object_path(hash);
        
        if !object_path.exists() {
            return Err(CasError::ObjectNotFound(*hash));
        }
        
        let data = fs::read(&object_path).await?;
        
        // Verify hash
        let mut hasher = Hasher::new();
        hasher.update(&data);
        let actual_hash = ContentHash::from_blake3(hasher.finalize());
        
        if actual_hash != *hash {
            return Err(CasError::HashMismatch {
                expected: *hash,
                actual: actual_hash,
            });
        }
        
        trace!("Read object {} ({} bytes)", hash, data.len());
        
        Ok(Bytes::from(data))
    }
    
    /// Check if an object exists
    pub async fn exists(&self, hash: &ContentHash) -> bool {
        self.object_path(hash).exists()
    }
    
    /// Verify an object's integrity
    pub async fn verify(&self, hash: &ContentHash) -> Result<bool> {
        match self.read(hash).await {
            Ok(_) => Ok(true),
            Err(CasError::ObjectNotFound(_)) => Ok(false),
            Err(CasError::HashMismatch { .. }) => Ok(false),
            Err(e) => Err(e),
        }
    }
    
    /// Delete an object (use with caution)
    pub async fn delete(&self, hash: &ContentHash) -> Result<()> {
        let object_path = self.object_path(hash);
        
        if object_path.exists() {
            fs::remove_file(&object_path).await?;
            debug!("Deleted object {}", hash);
        }
        
        Ok(())
    }
    
    /// Get storage statistics
    pub async fn stats(&self) -> Result<StorageStats> {
        // TODO: Implement actual stats collection
        Ok(StorageStats {
            object_count: 0,
            total_size: 0,
        })
    }
}

/// Storage statistics
#[derive(Debug, Clone)]
pub struct StorageStats {
    pub object_count: u64,
    pub total_size: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;
    
    #[tokio::test]
    async fn test_write_and_read() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();
        
        let data = b"Hello, Landropic!";
        let obj_ref = store.write(data).await.unwrap();
        
        let read_data = store.read(&obj_ref.hash).await.unwrap();
        assert_eq!(&read_data[..], data);
    }
    
    #[tokio::test]
    async fn test_exists() {
        let dir = tempdir().unwrap();
        let store = ContentStore::new(dir.path()).await.unwrap();
        
        let data = b"Test data";
        let obj_ref = store.write(data).await.unwrap();
        
        assert!(store.exists(&obj_ref.hash).await);
        
        let fake_hash = ContentHash::from_bytes([0u8; 32]);
        assert!(!store.exists(&fake_hash).await);
    }
}